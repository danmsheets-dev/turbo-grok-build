//! Turbo community updater.
//!
//! This module is deliberately separate from the official Grok updater. It
//! only reads releases from `danmsheets-dev/turbo-grok-build` and only activates
//! binaries below `~/.turbo` (or `TURBO_SHARE_DIR` in debug/test builds).
//! Nothing here calls the x.ai/npm updater or writes `~/.grok/bin/grok`.
//!
//! Release archives ship more than the executable: the release workflow copies
//! the whole `bundled/` tree (skills, agents, prompts) next to `turbo` /
//! `turbo.exe`. The archive reader therefore accepts a `bundled/**` subtree at
//! arbitrary depth and activates it at `$GROK_HOME/bundled` (default
//! `~/.grok/bundled`) — staged into a sibling directory first, then swapped in
//! with renames so a crash can never leave a half-written bundle live. Binaries
//! and update state stay under `TURBO_SHARE_DIR` / `~/.turbo`; only the bundle
//! is shared with the agent/skill loaders that read `$GROK_HOME/bundled`.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::auto_update::{
    BackgroundUpdateCheck, EnsureLatestOutcome, UpdateAvailable, UpdateRunMode, UpdateStatus,
};

const RELEASE_REPO: &str = "danmsheets-dev/turbo-grok-build";
const RELEASE_API_BASE: &str =
    "https://api.github.com/repos/danmsheets-dev/turbo-grok-build/releases";
const CHECK_TTL: Duration = Duration::from_secs(30 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const SMOKE_TEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_AUXILIARY_BYTES: u64 = 16 * 1024 * 1024;
/// A real Turbo archive is not just the binary plus a licence: it carries the
/// whole `bundled/` tree, which is thousands of small markdown files. The old
/// limit of 32 was sized for the binary-only layout and rejected every real
/// release.
const MAX_ARCHIVE_ENTRIES: usize = 4096;
const MAX_BUNDLE_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BUNDLE_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BUNDLE_FILES: usize = 4096;
/// Depth cap for archive entries. Deep nesting is never legitimate here and is
/// the cheapest way to blow past Windows' 260-char `MAX_PATH` once the staging
/// prefix is prepended.
const MAX_PATH_DEPTH: usize = 32;
const BUNDLE_DIR_NAME: &str = "bundled";
const INSTALLER_NAME: &str = "community-github";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UpdateState {
    #[serde(default)]
    installed_version: Option<String>,
    #[serde(default)]
    installed_asset: Option<String>,
    #[serde(default)]
    installed_sha256: Option<String>,
    #[serde(default)]
    installed_binary: Option<String>,
    #[serde(default)]
    checked_at_unix: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseMetadata {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone)]
struct Candidate {
    version: String,
    asset_name: String,
    archive_url: String,
    sha256: String,
}

#[derive(Debug, Clone, Copy)]
struct Platform {
    asset_triple: &'static str,
    local_os: &'static str,
    local_arch: &'static str,
    archive_suffix: &'static str,
    binary_entry: &'static str,
}

#[derive(Debug, Clone)]
struct ActiveDeployment {
    version: String,
    binary_name: String,
    sha256: Option<String>,
}

#[derive(Debug)]
struct ConvergeOutcome {
    target: String,
    installed: bool,
}

#[derive(Debug, Clone)]
struct UpdateSource {
    api_base: String,
    allow_insecure_local: bool,
}

/// OS-level lock guard. The lock is released automatically on process exit,
/// including crashes, so there is no stale-PID recovery protocol to get wrong.
struct UpdateLock(File);

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

/// Removes a temporary artifact unless ownership was explicitly consumed.
/// Staging a `bundled/` tree means the guard must be able to clean up whole
/// directories, not just files, so the kind is recorded at construction.
struct TempArtifact {
    path: PathBuf,
    is_dir: bool,
    keep: bool,
}

impl TempArtifact {
    fn new_file(path: PathBuf) -> Self {
        Self {
            path,
            is_dir: false,
            keep: false,
        }
    }

    fn new_dir(path: PathBuf) -> Self {
        Self {
            path,
            is_dir: true,
            keep: false,
        }
    }

    fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for TempArtifact {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if self.is_dir {
            let _ = std::fs::remove_dir_all(&self.path);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || path.is_symlink()
}

/// Joins a rollback failure onto the failure that triggered it. Both matter to
/// the operator: the first says what broke, the second says what state the
/// install was left in.
fn combine_errors(primary: anyhow::Error, secondary: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("{primary:#}\n\nalso: {secondary:#}")
}

/// Moves whatever currently occupies `path` out of the way so a restore can
/// rename its replacement in. Returns the aside path when something moved.
fn move_active_aside(path: &Path, suffix: &str) -> Result<Option<PathBuf>> {
    if !path_exists_or_symlink(path) {
        return Ok(None);
    }
    let doomed = unique_sibling(path, suffix);
    std::fs::rename(path, &doomed).with_context(|| {
        format!(
            "moving active path {} aside to {} for rollback",
            path.display(),
            doomed.display()
        )
    })?;
    Ok(Some(doomed))
}

pub(crate) fn turbo_home() -> PathBuf {
    // Prefer Turbo env; accept legacy Hyper env during migration.
    if let Some(path) =
        std::env::var_os("TURBO_SHARE_DIR").or_else(|| std::env::var_os("HYPER_SHARE_DIR"))
    {
        return PathBuf::from(path);
    }
    #[allow(deprecated)]
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".turbo")
}

/// Back-compat alias for call sites still using the old name.
#[inline]
pub(crate) fn hyper_home() -> PathBuf {
    turbo_home()
}

pub(crate) fn managed_application() -> PathBuf {
    let name = if cfg!(windows) { "turbo.exe" } else { "turbo" };
    hyper_home().join("bin").join(name)
}

/// Shared Grok config home (`$GROK_HOME`, default `~/.grok`).
///
/// Deliberately *not* [`turbo_home`]: binaries and update state are Turbo's own
/// (`~/.turbo`), but the bundle is runtime content and the agent/skill loaders
/// read it from the Grok home (`xai_grok_agent` discovery and skill lookup both
/// resolve `<grok home>/bundled/...`). `install.sh` activates the bundle at
/// `${GROK_HOME:-$HOME/.grok}/bundled` for the same reason; the in-app updater
/// must land in exactly the same place or an update would silently orphan the
/// skills the installer put there.
fn community_grok_home() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home()
}

/// Activation target for the archive's `bundled/` tree.
fn managed_bundle_path() -> PathBuf {
    community_grok_home().join(BUNDLE_DIR_NAME)
}

fn state_path() -> PathBuf {
    hyper_home().join("update-state.json")
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn load_state() -> UpdateState {
    std::fs::read(state_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn state_is_fresh(state: &UpdateState) -> bool {
    let Some(checked) = state.checked_at_unix else {
        return false;
    };
    let now = now_unix();
    checked <= now && now - checked < CHECK_TTL.as_secs()
}

fn reject_symlink(path: &Path, label: &str) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        bail!(
            "refusing to use symlinked Turbo {label}: {}",
            path.display()
        );
    }
    Ok(())
}

fn ensure_safe_layout() -> Result<()> {
    let home = hyper_home();
    if home.as_os_str().is_empty() {
        bail!("Turbo install root is empty");
    }
    if home.exists() {
        reject_symlink(&home, "install root")?;
    } else {
        std::fs::create_dir_all(&home)
            .with_context(|| format!("creating Turbo install root {}", home.display()))?;
    }
    for (name, label) in [
        ("bin", "bin directory"),
        ("downloads", "downloads directory"),
    ] {
        let dir = home.join(name);
        if dir.exists() {
            reject_symlink(&dir, label)?;
        } else {
            std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        if !std::fs::metadata(&dir)?.is_dir() {
            bail!("Turbo {label} is not a directory: {}", dir.display());
        }
    }
    Ok(())
}

async fn acquire_update_lock() -> Result<UpdateLock> {
    ensure_safe_layout()?;
    let lock_path = hyper_home().join("update.lock");
    reject_symlink(&lock_path, "update lock")?;
    tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening Turbo update lock {}", lock_path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("locking {}", lock_path.display()))?;
        Ok(UpdateLock(file))
    })
    .await
    .map_err(|e| anyhow::anyhow!("Turbo update lock task failed: {e}"))?
}

fn unique_sibling(base: &Path, suffix: &str) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut name = base
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(
        ".{}-{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
        suffix
    ));
    base.with_file_name(name)
}

fn write_state_atomic(state: &UpdateState) -> Result<()> {
    ensure_safe_layout()?;
    let mut bytes = serde_json::to_vec_pretty(state)?;
    bytes.push(b'\n');
    write_state_bytes_atomic(&state_path(), &bytes)
}

/// Byte-level state publish. Split out from [`write_state_atomic`] so install
/// rollback can put the *previous* state file back verbatim instead of
/// re-deriving it from a struct it may no longer be able to reconstruct.
fn write_state_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_symlink(path, "update state")?;
    let tmp = unique_sibling(path, "tmp");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;
    let tmp_guard = TempArtifact::new_file(tmp.clone());
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    #[cfg(windows)]
    {
        // Windows `rename` cannot replace an existing file, so publish is a
        // move-aside/rename/cleanup sequence with restore on failure.
        let backup = unique_sibling(path, "old");
        let had_old = path.exists();
        if had_old {
            std::fs::rename(path, &backup).with_context(|| {
                format!("moving existing Turbo update state {}", path.display())
            })?;
        }
        if let Err(error) = std::fs::rename(&tmp, path) {
            let activation_error =
                anyhow::Error::new(error).context(format!("activating {}", path.display()));
            if had_old && let Err(restore_error) = std::fs::rename(&backup, path) {
                return Err(combine_errors(
                    activation_error,
                    anyhow::Error::new(restore_error).context(format!(
                        "restoring previous Turbo update state from {} (backup preserved)",
                        backup.display()
                    )),
                ));
            }
            return Err(activation_error);
        }
        let _ = std::fs::remove_file(backup);
    }
    #[cfg(not(windows))]
    std::fs::rename(&tmp, path).with_context(|| format!("activating {}", path.display()))?;

    let _ = tmp_guard.keep();
    Ok(())
}

/// Captures the exact previous update-state bytes *before* any activation
/// mutation. Anything other than "missing" or "readable regular file" fails
/// closed so a symlinked or directory state path cannot be silently clobbered.
fn capture_previous_state_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    reject_symlink(path, "update state")?;
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.is_file() {
                bail!(
                    "Turbo update state is not a regular file: {}",
                    path.display()
                );
            }
            let bytes = std::fs::read(path)
                .with_context(|| format!("reading Turbo update state {}", path.display()))?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("inspecting Turbo update state {}", path.display()))
        }
    }
}

fn restore_state_bytes(path: &Path, previous: Option<&[u8]>) -> Result<()> {
    match previous {
        Some(bytes) => write_state_bytes_atomic(path, bytes),
        None => {
            if path_exists_or_symlink(path) {
                std::fs::remove_file(path)
                    .with_context(|| format!("removing {}", path.display()))?;
            }
            Ok(())
        }
    }
}

#[allow(unreachable_code)]
fn platform() -> Result<Platform> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok(Platform {
        asset_triple: "aarch64-apple-darwin",
        local_os: "macos",
        local_arch: "aarch64",
        archive_suffix: "tar.gz",
        binary_entry: "turbo",
    });
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok(Platform {
        asset_triple: "x86_64-apple-darwin",
        local_os: "macos",
        local_arch: "x86_64",
        archive_suffix: "tar.gz",
        binary_entry: "turbo",
    });
    #[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
    return Ok(Platform {
        asset_triple: "aarch64-unknown-linux-gnu",
        local_os: "linux",
        local_arch: "aarch64",
        archive_suffix: "tar.gz",
        binary_entry: "turbo",
    });
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    return Ok(Platform {
        asset_triple: "x86_64-unknown-linux-gnu",
        local_os: "linux",
        local_arch: "x86_64",
        archive_suffix: "tar.gz",
        binary_entry: "turbo",
    });
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Ok(Platform {
        asset_triple: "x86_64-pc-windows-msvc",
        local_os: "windows",
        local_arch: "x86_64",
        archive_suffix: "zip",
        binary_entry: "turbo.exe",
    });
    bail!("this platform does not have a published Turbo community artifact")
}

fn update_source() -> Result<UpdateSource> {
    let Some(override_base) = std::env::var_os("TURBO_UPDATE_BASE_URL")
        .or_else(|| std::env::var_os("HYPER_UPDATE_BASE_URL"))
    else {
        return Ok(UpdateSource {
            api_base: RELEASE_API_BASE.to_string(),
            allow_insecure_local: false,
        });
    };

    // A release build must never inherit an arbitrary update origin. The
    // override exists only for hermetic debug/integration tests and requires a
    // second, explicit opt-in so an accidental environment leak fails closed.
    if !cfg!(debug_assertions)
        || std::env::var_os("TURBO_ALLOW_INSECURE_UPDATE_BASE")
            .or_else(|| std::env::var_os("HYPER_ALLOW_INSECURE_UPDATE_BASE"))
            .as_deref()
            != Some(std::ffi::OsStr::new("1"))
    {
        bail!(
            "TURBO_UPDATE_BASE_URL is disabled in production Turbo builds; updates are pinned to {RELEASE_REPO}"
        );
    }
    let api_base = override_base
        .to_string_lossy()
        .trim_end_matches('/')
        .to_string();
    let url = reqwest::Url::parse(&api_base).context("invalid TURBO_UPDATE_BASE_URL")?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if !local {
        bail!("debug update-base overrides are restricted to localhost");
    }
    Ok(UpdateSource {
        api_base,
        allow_insecure_local: true,
    })
}

fn allowed_github_redirect(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    matches!(
        url.host_str(),
        Some(
            "api.github.com"
                | "github.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
        )
    )
}

fn http_client(source: &UpdateSource) -> Result<reqwest::Client> {
    let allow_local = source.allow_insecure_local;
    let local_origin = reqwest::Url::parse(&source.api_base).ok().and_then(|u| {
        Some((
            u.scheme().to_string(),
            u.host_str()?.to_string(),
            u.port_or_known_default(),
        ))
    });
    let redirect = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects while updating Turbo");
        }
        let url = attempt.url();
        if allow_local {
            let same_local_origin = local_origin.as_ref().is_some_and(|(scheme, host, port)| {
                url.scheme() == scheme
                    && url.host_str() == Some(host.as_str())
                    && url.port_or_known_default() == *port
            });
            if same_local_origin {
                attempt.follow()
            } else {
                attempt.stop()
            }
        } else if allowed_github_redirect(url) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });
    Ok(reqwest::Client::builder()
        .user_agent("turbo-community-updater")
        .timeout(REQUEST_TIMEOUT)
        .redirect(redirect)
        .build()?)
}

async fn response_bytes_limited(response: reqwest::Response, max: u64) -> Result<Vec<u8>> {
    if let Some(length) = response.content_length()
        && length > max
    {
        bail!("update response is too large ({length} bytes; limit {max})");
    }
    let mut out = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if out.len() as u64 + chunk.len() as u64 > max {
            bail!("update response exceeded the {max}-byte limit");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

async fn checked_response(response: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response_bytes_limited(response, 4096)
        .await
        .unwrap_or_default();
    let detail = String::from_utf8_lossy(&body);
    bail!("{what} failed with HTTP {status}: {}", detail.trim());
}

fn validate_release_asset_url(source: &UpdateSource, url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("release contains an invalid asset URL")?;
    if source.allow_insecure_local {
        let base = reqwest::Url::parse(&source.api_base)?;
        if parsed.scheme() != base.scheme()
            || parsed.host_str() != base.host_str()
            || parsed.port_or_known_default() != base.port_or_known_default()
        {
            bail!("debug release asset URL escaped the localhost update origin");
        }
        return Ok(());
    }
    let expected_prefix = format!("/{RELEASE_REPO}/releases/download/");
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || !parsed.path().starts_with(&expected_prefix)
    {
        bail!("release asset URL is outside the pinned Turbo GitHub repository");
    }
    Ok(())
}

fn one_asset<'a>(release: &'a ReleaseMetadata, name: &str) -> Result<&'a ReleaseAsset> {
    let mut matching = release.assets.iter().filter(|asset| asset.name == name);
    let Some(asset) = matching.next() else {
        bail!("release {} has no asset {name}", release.tag_name);
    };
    if matching.next().is_some() {
        bail!(
            "release {} contains duplicate asset {name}",
            release.tag_name
        );
    }
    Ok(asset)
}

fn parse_manifest_checksum(manifest: &str, asset_name: &str) -> Result<String> {
    let mut found: Option<String> = None;
    for line in manifest.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name.trim_start_matches('*') != asset_name {
            continue;
        }
        if parts.next().is_some() {
            bail!("SHA256SUMS contains a malformed entry for {asset_name}");
        }
        let normalized = hash.to_ascii_lowercase();
        if !valid_sha256(&normalized) {
            bail!("SHA256SUMS contains an invalid checksum for {asset_name}");
        }
        if found.replace(normalized).is_some() {
            bail!("SHA256SUMS contains duplicate entries for {asset_name}");
        }
    }
    found.ok_or_else(|| anyhow::anyhow!("SHA256SUMS has no entry for {asset_name}"))
}

/// Resolve only the release metadata version. Version checks must not depend
/// on a platform asset being present; a stale/missing asset is an installability
/// problem, not evidence that the release catalog has no latest version.
async fn resolve_latest_release_version() -> Result<String> {
    let source = update_source()?;
    let client = http_client(&source)?;
    let endpoint = format!("{}/latest", source.api_base);
    let mut request = client
        .get(&endpoint)
        .header("Accept", "application/vnd.github+json");
    if !source.allow_insecure_local
        && let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.trim().is_empty()
    {
        request = request.bearer_auth(token.trim());
    }
    let response = checked_response(
        request.send().await?,
        "Turbo latest release metadata request",
    )
    .await?;
    let release_bytes = response_bytes_limited(response, MAX_MANIFEST_BYTES).await?;
    let release: ReleaseMetadata =
        serde_json::from_slice(&release_bytes).context("invalid Turbo release metadata")?;
    if release.draft {
        bail!("latest Turbo release is a draft");
    }
    if release.prerelease {
        bail!("latest Turbo release endpoint returned a prerelease");
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    semver::Version::parse(version)
        .with_context(|| format!("release tag {} is not valid semver", release.tag_name))?;
    Ok(version.to_owned())
}

async fn resolve_candidate(pinned_version: Option<&str>) -> Result<Candidate> {
    let source = update_source()?;
    let client = http_client(&source)?;
    let endpoint = match pinned_version {
        Some(version) => format!("{}/tags/v{version}", source.api_base),
        None => format!("{}/latest", source.api_base),
    };
    let mut request = client
        .get(&endpoint)
        .header("Accept", "application/vnd.github+json");
    if !source.allow_insecure_local
        && let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.trim().is_empty()
    {
        // Only the fixed api.github.com request receives this token. Browser
        // download URLs and all debug overrides remain unauthenticated.
        request = request.bearer_auth(token.trim());
    }
    let response =
        checked_response(request.send().await?, "Turbo release metadata request").await?;
    let release_bytes = response_bytes_limited(response, MAX_MANIFEST_BYTES).await?;
    let release: ReleaseMetadata =
        serde_json::from_slice(&release_bytes).context("invalid Turbo release metadata")?;
    if release.draft {
        bail!("refusing to install draft release {}", release.tag_name);
    }
    if pinned_version.is_none() && release.prerelease {
        bail!("the latest Turbo release endpoint returned a prerelease");
    }
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    semver::Version::parse(version)
        .with_context(|| format!("release tag {} is not valid semver", release.tag_name))?;
    if let Some(requested) = pinned_version
        && requested != version
    {
        bail!("requested Turbo {requested}, but the release endpoint returned {version}");
    }

    let platform = platform()?;
    let asset_name = format!(
        "turbo-{version}-{}.{}",
        platform.asset_triple, platform.archive_suffix
    );
    let archive_asset = one_asset(&release, &asset_name)?;
    let sums_asset = one_asset(&release, "SHA256SUMS")?;
    validate_release_asset_url(&source, &archive_asset.browser_download_url)?;
    validate_release_asset_url(&source, &sums_asset.browser_download_url)?;

    let sums_response = checked_response(
        client.get(&sums_asset.browser_download_url).send().await?,
        "Turbo SHA256SUMS download",
    )
    .await?;
    let sums = response_bytes_limited(sums_response, MAX_MANIFEST_BYTES).await?;
    let sums = std::str::from_utf8(&sums).context("SHA256SUMS is not UTF-8")?;
    let sha256 = parse_manifest_checksum(sums, &asset_name)?;

    Ok(Candidate {
        version: version.to_string(),
        asset_name,
        archive_url: archive_asset.browser_download_url.clone(),
        sha256,
    })
}

fn version_from_managed_name(name: &str) -> Option<String> {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    let suffix = name.strip_prefix("turbo-")?;
    let marker = ["-macos-", "-linux-", "-windows-"]
        .into_iter()
        .find_map(|marker| suffix.find(marker).map(|index| (marker, index)))?;
    let version = &suffix[..marker.1];
    semver::Version::parse(version).ok()?;
    Some(version.to_string())
}

fn digest_from_managed_name(name: &str) -> Option<String> {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    let digest = name.rsplit_once("-sha256-")?.1.to_ascii_lowercase();
    valid_sha256(&digest).then_some(digest)
}

fn active_deployment() -> Option<ActiveDeployment> {
    let app = managed_application();
    let metadata = std::fs::metadata(&app).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }

    #[cfg(unix)]
    let binary_name = {
        let target = std::fs::read_link(&app).ok()?;
        target.file_name()?.to_string_lossy().to_string()
    };
    #[cfg(windows)]
    let binary_name = load_state().installed_binary?;
    #[cfg(not(any(unix, windows)))]
    return None;

    let version = version_from_managed_name(&binary_name).or_else(|| {
        let state = load_state();
        (state.installed_binary.as_deref() == Some(binary_name.as_str()))
            .then_some(state.installed_version)
            .flatten()
    })?;
    let state = load_state();
    let state_sha = (state.installed_version.as_deref() == Some(version.as_str())
        && state.installed_binary.as_deref() == Some(binary_name.as_str()))
    .then_some(state.installed_sha256)
    .flatten()
    .filter(|sha| valid_sha256(sha));
    Some(ActiveDeployment {
        version,
        sha256: digest_from_managed_name(&binary_name).or(state_sha),
        binary_name,
    })
}

fn current_exe_belongs_to_turbo_home() -> bool {
    let Ok(exe) = std::env::current_exe().and_then(dunce::canonicalize) else {
        return false;
    };
    let home = dunce::canonicalize(hyper_home()).unwrap_or_else(|_| hyper_home());
    exe.starts_with(home.join("downloads")) || exe.starts_with(home.join("bin"))
}

fn current_process_is_managed() -> bool {
    match (
        std::env::current_exe()
            .ok()
            .and_then(|p| dunce::canonicalize(p).ok()),
        dunce::canonicalize(managed_application()).ok(),
    ) {
        (Some(exe), Some(active)) => exe == active,
        _ => false,
    }
}

pub(crate) fn running_differs_from_active() -> bool {
    if !current_exe_belongs_to_turbo_home() {
        return false;
    }
    match (
        std::env::current_exe()
            .ok()
            .and_then(|p| dunce::canonicalize(p).ok()),
        dunce::canonicalize(managed_application()).ok(),
    ) {
        (Some(exe), Some(active)) => exe != active,
        _ => false,
    }
}

fn automatic_entry_allowed() -> bool {
    current_process_is_managed() || running_differs_from_active()
}

fn deployed_digest(active: &ActiveDeployment, state: &UpdateState) -> Option<String> {
    active.sha256.clone().or_else(|| {
        (state.installed_version.as_deref() == Some(active.version.as_str())
            && state.installed_binary.as_deref() == Some(active.binary_name.as_str()))
        .then(|| state.installed_sha256.clone())
        .flatten()
        .filter(|sha| valid_sha256(sha))
    })
}

fn candidate_requires_install(
    candidate: &Candidate,
    active: Option<&ActiveDeployment>,
    state: &UpdateState,
) -> Result<bool> {
    let Some(active) = active else {
        return Ok(true);
    };
    let target = semver::Version::parse(&candidate.version)?;
    let current = semver::Version::parse(&active.version)?;
    if target > current {
        return Ok(true);
    }
    if target < current {
        return Ok(false);
    }
    // A release tag may be republished in this community repository. Once an
    // install has an archive identity, equal semver but different digest is a
    // real update. Old installer layouts without state are adopted once rather
    // than forcing a multi-hundred-MiB reinstall on first launch.
    Ok(deployed_digest(active, state).is_some_and(|sha| sha != candidate.sha256))
}

fn state_matches_active(state: &UpdateState, active: &ActiveDeployment) -> bool {
    state.installed_version.as_deref() == Some(active.version.as_str())
        && state.installed_binary.as_deref() == Some(active.binary_name.as_str())
        && state.installed_sha256.as_deref().is_some_and(valid_sha256)
        && deployed_digest(active, state).as_deref() == state.installed_sha256.as_deref()
}

pub(crate) fn is_version_cache_fresh() -> bool {
    let state = load_state();
    let Some(active) = active_deployment() else {
        return false;
    };
    state_is_fresh(&state) && state_matches_active(&state, &active)
}

pub(crate) fn installed_on_disk_version() -> Option<String> {
    active_deployment().map(|deployment| deployment.version)
}

fn reconcile_checked_state(candidate: &Candidate, active: Option<&ActiveDeployment>) -> Result<()> {
    let mut state = load_state();
    if let Some(active) = active {
        state.installed_version = Some(active.version.clone());
        state.installed_binary = Some(active.binary_name.clone());
        if active.version == candidate.version {
            state.installed_asset = Some(candidate.asset_name.clone());
            state.installed_sha256 = Some(candidate.sha256.clone());
        } else if let Some(sha) = &active.sha256 {
            state.installed_sha256 = Some(sha.clone());
        }
    }
    state.checked_at_unix = Some(now_unix());
    write_state_atomic(&state)
}

async fn record_no_update(candidate: &Candidate) -> Result<()> {
    let _lock = acquire_update_lock().await?;
    let state = load_state();
    let active = active_deployment();
    if candidate_requires_install(candidate, active.as_ref(), &state)? {
        // Another process changed the active deployment while this caller was
        // waiting for the lock. Do not cache a stale "no update" conclusion.
        return Ok(());
    }
    reconcile_checked_state(candidate, active.as_ref())
}

async fn download_archive(candidate: &Candidate, destination: &Path) -> Result<String> {
    let source = update_source()?;
    validate_release_asset_url(&source, &candidate.archive_url)?;
    let client = http_client(&source)?;
    let response = checked_response(
        client.get(&candidate.archive_url).send().await?,
        "Turbo archive download",
    )
    .await?;
    if let Some(length) = response.content_length()
        && length > MAX_ARCHIVE_BYTES
    {
        bail!("Turbo archive is too large ({length} bytes)");
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await
        .with_context(|| format!("creating {}", destination.display()))?;
    let mut size = 0u64;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        size = size.saturating_add(chunk.len() as u64);
        if size > MAX_ARCHIVE_BYTES {
            bail!("Turbo archive exceeded the {MAX_ARCHIVE_BYTES}-byte limit");
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    let digest = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(digest)
}

/// Windows refuses to create these names (with or without an extension) in any
/// directory, and some of them are devices rather than files. An archive that
/// contains one is either hostile or broken; either way it must never reach a
/// `create_new` call on a Turbo user's machine. Checked on every platform so a
/// Linux CI run catches a bad archive before Windows users do.
fn is_windows_reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn validate_path_component(name: &str, raw: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        bail!("archive entry has an invalid path component: {raw}");
    }
    if name.contains('\0') {
        bail!("archive entry has an invalid path component: {raw}");
    }
    // `:` would be an alternate-data-stream / drive separator on Windows.
    if name.contains(':') {
        bail!("archive entry component contains ':': {raw}");
    }
    // Windows silently strips trailing dots and spaces, so `evil.` and `evil `
    // both resolve to `evil` — a rename-time collision the checks above cannot
    // see.
    if name.ends_with('.') || name.ends_with(' ') {
        bail!("archive entry component has trailing '.' or space: {raw}");
    }
    if is_windows_reserved_device(name) {
        bail!("archive entry uses a Windows reserved device name: {raw}");
    }
    Ok(())
}

/// Safely normalizes an archive entry into relative path components.
///
/// Multiple `Normal` components are allowed (that is the whole point: a
/// `bundled/**` tree is nested), `.` is ignored, and absolute / rooted /
/// drive-prefixed / `..` paths, empty and reserved names, and excessive depth
/// are rejected. Returns `None` for the archive root (`.` or empty), which tar
/// producers emit as a directory placeholder.
///
/// `allow_backslash_as_separator`: zip producers (PowerShell's `Copy-Item` +
/// `Compress-Archive`, older tools) may emit `\` separators, so for zip we fold
/// them to `/` and then apply the same component rules. Tar entry names are
/// defined to use `/`, so a literal backslash there is a suspicious filename
/// and stays rejected outright.
fn normalize_archive_path(
    raw: &str,
    allow_backslash_as_separator: bool,
) -> Result<Option<Vec<String>>> {
    if raw.contains('\0') {
        bail!("archive entry contains a NUL byte");
    }
    let normalized = if allow_backslash_as_separator {
        raw.replace('\\', "/")
    } else {
        if raw.contains('\\') {
            bail!("archive entry uses a backslash path: {raw}");
        }
        raw.to_string()
    };
    // `Path::components` on Unix does not treat `C:` as a prefix, so a drive
    // spec would survive as a `Normal` component and only turn into an absolute
    // path once it reached Windows. Reject it explicitly on every platform.
    if normalized.chars().nth(1) == Some(':') {
        bail!("archive entry has a Windows drive prefix: {raw}");
    }
    let path = Path::new(&normalized);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                let name = part.to_str().ok_or_else(|| {
                    anyhow::anyhow!("archive entry has a non-UTF-8 component: {raw}")
                })?;
                validate_path_component(name, raw)?;
                if name.contains('/') || (!allow_backslash_as_separator && name.contains('\\')) {
                    bail!("archive entry has an invalid path component: {raw}");
                }
                parts.push(name.to_string());
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("archive entry escapes its root: {raw}");
            }
        }
    }
    if parts.len() > MAX_PATH_DEPTH {
        bail!("archive entry exceeds the maximum path depth ({MAX_PATH_DEPTH}): {raw}");
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts))
}

/// Duplicate detection key. Case-folded because NTFS and APFS are usually
/// case-insensitive: `bundled/A.md` and `bundled/a.md` are two archive entries
/// but one file on disk, and the second `create_new` would fail mid-extract.
fn path_key_casefold(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| p.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

fn display_parts(parts: &[String]) -> String {
    parts.join("/")
}

fn auxiliary_entry_allowed(name: &str) -> bool {
    matches!(
        name,
        "LICENSE" | "NOTICE" | "THIRD-PARTY-NOTICES" | "THIRD-PARTY-NOTICES.md"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveEntryClass {
    /// Archive root `.` / empty (tar directory placeholder).
    RootPlaceholder,
    /// Root-level managed binary (`turbo` / `turbo.exe`).
    Binary,
    /// Root-level licence/notice allowlist (drained, never deployed).
    Notice,
    /// The `bundled` directory entry itself.
    BundleRootDir,
    /// A directory under `bundled/`.
    BundleDir,
    /// A regular file under `bundled/`.
    BundleFile,
}

/// The archive layout contract, in one place: exactly one `turbo` binary at the
/// root, an allowlist of root notices, and an optional `bundled/**` subtree.
/// Anything else — including nested paths outside `bundled/` — is rejected.
fn classify_archive_entry(
    parts: Option<&[String]>,
    binary_entry: &str,
) -> Result<ArchiveEntryClass> {
    let Some(parts) = parts else {
        return Ok(ArchiveEntryClass::RootPlaceholder);
    };
    match parts.len() {
        0 => Ok(ArchiveEntryClass::RootPlaceholder),
        1 => {
            let name = &parts[0];
            if name == binary_entry {
                Ok(ArchiveEntryClass::Binary)
            } else if auxiliary_entry_allowed(name) {
                Ok(ArchiveEntryClass::Notice)
            } else if name == BUNDLE_DIR_NAME {
                Ok(ArchiveEntryClass::BundleRootDir)
            } else {
                bail!("Turbo archive contains unexpected root entry {name}");
            }
        }
        _ => {
            if parts[0] != BUNDLE_DIR_NAME {
                bail!(
                    "Turbo archive contains unexpected nested entry {}",
                    display_parts(parts)
                );
            }
            // Every remaining component was already validated by
            // `normalize_archive_path`, so the join below cannot escape.
            Ok(ArchiveEntryClass::BundleFile)
        }
    }
}

/// Result of a successful archive extraction.
struct ExtractedArchive {
    binary: PathBuf,
    /// Stage directory whose *contents* are the `bundled/` tree (the `bundled`
    /// component itself is not part of the path). `None` when the archive did
    /// not ship a bundle, e.g. older releases and the binary-only test fixture.
    bundle_stage: Option<PathBuf>,
    _binary_guard: TempArtifact,
    _bundle_guard: Option<TempArtifact>,
}

/// Running totals enforced across an extraction so a zip bomb cannot expand
/// into the Turbo home one small entry at a time.
struct ExtractLimits {
    entries: usize,
    bundle_files: usize,
    bundle_bytes: u64,
}

impl ExtractLimits {
    fn new() -> Self {
        Self {
            entries: 0,
            bundle_files: 0,
            bundle_bytes: 0,
        }
    }

    fn count_entry(&mut self) -> Result<()> {
        self.entries += 1;
        if self.entries > MAX_ARCHIVE_ENTRIES {
            bail!("Turbo archive contains too many entries (limit {MAX_ARCHIVE_ENTRIES})");
        }
        Ok(())
    }

    fn count_bundle_file(&mut self, size: u64) -> Result<()> {
        self.bundle_files += 1;
        if self.bundle_files > MAX_BUNDLE_FILES {
            bail!("Turbo archive bundle contains too many files (limit {MAX_BUNDLE_FILES})");
        }
        self.bundle_bytes = self.bundle_bytes.saturating_add(size);
        if self.bundle_bytes > MAX_BUNDLE_TOTAL_BYTES {
            bail!(
                "Turbo archive bundle exceeds the {MAX_BUNDLE_TOTAL_BYTES}-byte decompressed limit"
            );
        }
        Ok(())
    }
}

fn ensure_parent_dirs(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory {}", parent.display()))?;
    }
    Ok(())
}

/// Copies at most `max` bytes and fails if the source had more. The declared
/// header size is only a hint; this is the check that actually binds.
fn copy_limited<R: Read>(
    reader: &mut R,
    mut writer: impl Write,
    max: u64,
    label: &str,
) -> Result<u64> {
    let copied = std::io::copy(&mut reader.take(max.saturating_add(1)), &mut writer)?;
    if copied > max {
        bail!("{label} exceeds the decompressed size limit ({max} bytes)");
    }
    Ok(copied)
}

fn drain_limited<R: Read>(reader: &mut R, max: u64, label: &str) -> Result<u64> {
    copy_limited(reader, std::io::sink(), max, label)
}

fn insert_seen(seen: &mut HashSet<String>, parts: &[String]) -> Result<()> {
    let key = path_key_casefold(parts);
    if !seen.insert(key) {
        bail!(
            "Turbo archive contains duplicate or case-colliding entry {}",
            display_parts(parts)
        );
    }
    Ok(())
}

fn prepare_extract_destinations(
    stage_root: &Path,
    bundle_stage: PathBuf,
    binary_entry: &str,
) -> Result<(PathBuf, PathBuf, TempArtifact, TempArtifact)> {
    std::fs::create_dir_all(stage_root)
        .with_context(|| format!("creating extract stage {}", stage_root.display()))?;
    let binary_path = stage_root.join(binary_entry);
    // The bundle stage must already sit on the same filesystem as the final
    // `$GROK_HOME/bundled` target so activation is a rename, not a copy — a
    // copy would not be atomic and could be interrupted half-written.
    if path_exists_or_symlink(&bundle_stage) {
        bail!(
            "bundle stage path already exists: {}",
            bundle_stage.display()
        );
    }
    std::fs::create_dir_all(&bundle_stage)
        .with_context(|| format!("creating bundle stage {}", bundle_stage.display()))?;
    let binary_guard = TempArtifact::new_file(binary_path.clone());
    let bundle_guard = TempArtifact::new_dir(bundle_stage.clone());
    Ok((binary_path, bundle_stage, binary_guard, bundle_guard))
}

fn finish_extracted(
    binary_path: PathBuf,
    binary_guard: TempArtifact,
    bundle_stage: PathBuf,
    bundle_guard: TempArtifact,
    wrote_bundle: bool,
    found_binary: bool,
    binary_entry: &str,
) -> Result<ExtractedArchive> {
    if !found_binary {
        bail!("Turbo archive does not contain {binary_entry}");
    }
    if !binary_path.is_file() {
        bail!("Turbo binary stage is missing after extraction");
    }
    if wrote_bundle {
        Ok(ExtractedArchive {
            binary: binary_path,
            bundle_stage: Some(bundle_stage),
            _binary_guard: binary_guard,
            _bundle_guard: Some(bundle_guard),
        })
    } else {
        // No bundle in this archive: dropping the guard removes the empty stage
        // so activation never publishes an empty `bundled/` over a good one.
        drop(bundle_guard);
        Ok(ExtractedArchive {
            binary: binary_path,
            bundle_stage: None,
            _binary_guard: binary_guard,
            _bundle_guard: None,
        })
    }
}

#[cfg(unix)]
fn extract_tar_archive(
    archive_path: &Path,
    stage_root: &Path,
    bundle_stage: PathBuf,
    binary_entry: &str,
) -> Result<ExtractedArchive> {
    use std::os::unix::fs::PermissionsExt;
    use tar::EntryType;

    let (binary_path, bundle_stage, binary_guard, bundle_guard) =
        prepare_extract_destinations(stage_root, bundle_stage, binary_entry)?;

    let archive_file = File::open(archive_path)
        .with_context(|| format!("opening Turbo archive {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(decoder);
    let mut seen = HashSet::new();
    let mut limits = ExtractLimits::new();
    let mut found_binary = false;
    let mut wrote_bundle = false;

    for entry in archive.entries().context("reading Turbo tar archive")? {
        limits.count_entry()?;
        let mut entry = entry.context("reading Turbo tar entry")?;
        let kind = entry.header().entry_type();
        // Security decisions must not run on lossily decoded paths: a lossy
        // decode can turn an unrepresentable byte into a benign-looking name.
        let raw_os = entry
            .path()
            .context("reading Turbo tar entry path")?
            .into_owned();
        let raw = raw_os
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Turbo archive entry path is not valid UTF-8"))?
            .to_string();
        let parts = normalize_archive_path(&raw, false)?;

        // `tar -C staging .` emits a `.` root placeholder. Accept it only as a
        // directory so a zero-byte root *file* cannot skip classification.
        if parts.is_none() {
            match kind {
                EntryType::Directory => continue,
                _ => bail!("Turbo archive root entry has unsupported type: {raw}"),
            }
        }
        let parts = parts.expect("checked above");
        let mut class = classify_archive_entry(Some(&parts), binary_entry)?;
        if matches!(class, ArchiveEntryClass::BundleFile) && kind == EntryType::Directory {
            class = ArchiveEntryClass::BundleDir;
        }
        if matches!(class, ArchiveEntryClass::BundleRootDir) && kind != EntryType::Directory {
            bail!(
                "Turbo archive entry {} must be a directory",
                display_parts(&parts)
            );
        }

        match kind {
            EntryType::Directory => {
                insert_seen(&mut seen, &parts)?;
                match class {
                    ArchiveEntryClass::BundleRootDir | ArchiveEntryClass::BundleDir => {
                        let rel: PathBuf = parts[1..].iter().collect();
                        let dest = bundle_stage.join(&rel);
                        std::fs::create_dir_all(&dest).with_context(|| {
                            format!(
                                "creating bundle directory stage for {}",
                                display_parts(&parts)
                            )
                        })?;
                        wrote_bundle = true;
                    }
                    ArchiveEntryClass::RootPlaceholder => {}
                    _ => bail!(
                        "Turbo archive has an unexpected directory entry {}",
                        display_parts(&parts)
                    ),
                }
            }
            EntryType::Regular | EntryType::Continuous => {
                insert_seen(&mut seen, &parts)?;
                match class {
                    ArchiveEntryClass::Binary => {
                        if found_binary {
                            bail!("Turbo archive contains duplicate {binary_entry}");
                        }
                        if entry.size() > MAX_BINARY_BYTES {
                            bail!("Turbo binary exceeds the decompressed size limit");
                        }
                        let mut out = OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(&binary_path)
                            .with_context(|| {
                                format!("creating binary stage {}", binary_path.display())
                            })?;
                        copy_limited(&mut entry, &mut out, MAX_BINARY_BYTES, "Turbo binary")?;
                        out.sync_all()?;
                        std::fs::set_permissions(
                            &binary_path,
                            std::fs::Permissions::from_mode(0o755),
                        )?;
                        found_binary = true;
                    }
                    ArchiveEntryClass::Notice => {
                        if entry.size() > MAX_AUXILIARY_BYTES {
                            bail!(
                                "Turbo archive auxiliary entry {} is too large",
                                display_parts(&parts)
                            );
                        }
                        // Drained, not unpacked: validates the stream without
                        // putting release notices into the install tree.
                        drain_limited(
                            &mut entry,
                            MAX_AUXILIARY_BYTES,
                            &format!("Turbo archive auxiliary entry {}", display_parts(&parts)),
                        )?;
                    }
                    ArchiveEntryClass::BundleFile => {
                        if entry.size() > MAX_BUNDLE_FILE_BYTES {
                            bail!(
                                "Turbo archive bundle file {} exceeds the per-file size limit",
                                display_parts(&parts)
                            );
                        }
                        let rel: PathBuf = parts[1..].iter().collect();
                        if rel.as_os_str().is_empty() {
                            bail!("Turbo archive bundle file path is empty");
                        }
                        let dest = bundle_stage.join(&rel);
                        ensure_parent_dirs(&dest)?;
                        let mut out = OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(&dest)
                            .with_context(|| {
                                format!(
                                    "creating bundle stage file {} -> {}",
                                    display_parts(&parts),
                                    dest.display()
                                )
                            })?;
                        let copied = copy_limited(
                            &mut entry,
                            &mut out,
                            MAX_BUNDLE_FILE_BYTES,
                            &format!("Turbo archive bundle file {}", display_parts(&parts)),
                        )?;
                        out.sync_all()?;
                        limits.count_bundle_file(copied)?;
                        wrote_bundle = true;
                    }
                    ArchiveEntryClass::BundleRootDir | ArchiveEntryClass::BundleDir => bail!(
                        "Turbo archive directory entry {} is not a regular file",
                        display_parts(&parts)
                    ),
                    ArchiveEntryClass::RootPlaceholder => {
                        bail!("Turbo archive contains an unnamed regular entry")
                    }
                }
            }
            // Symlinks and hard links are how an archive reaches outside its
            // own tree; devices and sparse/extended headers have no legitimate
            // use in a Turbo release.
            _ => bail!(
                "Turbo archive contains unsupported entry type {:?} at {}",
                kind,
                display_parts(&parts)
            ),
        }
    }

    finish_extracted(
        binary_path,
        binary_guard,
        bundle_stage,
        bundle_guard,
        wrote_bundle,
        found_binary,
        binary_entry,
    )
}

/// Not `#[cfg(windows)]`: the Windows producer layout is the one most likely to
/// carry hostile separators, and gating this on Windows would mean the security
/// matrix for it never runs on Linux CI.
fn extract_zip_archive(
    archive_path: &Path,
    stage_root: &Path,
    bundle_stage: PathBuf,
    binary_entry: &str,
) -> Result<ExtractedArchive> {
    let (binary_path, bundle_stage, binary_guard, bundle_guard) =
        prepare_extract_destinations(stage_root, bundle_stage, binary_entry)?;

    let file = File::open(archive_path)
        .with_context(|| format!("opening Turbo archive {}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("reading Turbo zip archive")?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        bail!("Turbo archive contains too many entries (limit {MAX_ARCHIVE_ENTRIES})");
    }

    let mut seen = HashSet::new();
    let mut limits = ExtractLimits::new();
    let mut found_binary = false;
    let mut wrote_bundle = false;

    for index in 0..archive.len() {
        limits.count_entry()?;
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("reading Turbo zip entry #{index}"))?;
        let raw_name = entry.name().to_string();
        let trimmed = raw_name.trim_end_matches(['/', '\\']);
        let parts = normalize_archive_path(trimmed, true)?;
        let is_dir = entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\');

        // Unix mode S_IFLNK survives in zip entries produced on Unix; a symlink
        // extracted into the bundle would redirect writes outside the stage.
        if entry.is_symlink() {
            bail!("Turbo archive contains a symlink: {raw_name}");
        }

        if parts.is_none() {
            if is_dir {
                continue;
            }
            bail!("Turbo archive contains an unnamed regular entry: {raw_name}");
        }
        let parts = parts.expect("checked above");
        let mut class = classify_archive_entry(Some(&parts), binary_entry)?;

        if is_dir {
            if matches!(class, ArchiveEntryClass::BundleFile) {
                class = ArchiveEntryClass::BundleDir;
            }
            insert_seen(&mut seen, &parts)?;
            match class {
                ArchiveEntryClass::BundleRootDir | ArchiveEntryClass::BundleDir => {
                    let rel: PathBuf = parts[1..].iter().collect();
                    let dest = bundle_stage.join(&rel);
                    std::fs::create_dir_all(&dest).with_context(|| {
                        format!(
                            "creating bundle directory stage for {}",
                            display_parts(&parts)
                        )
                    })?;
                    wrote_bundle = true;
                }
                ArchiveEntryClass::RootPlaceholder => {}
                _ => bail!(
                    "Turbo archive has an unexpected directory entry {}",
                    display_parts(&parts)
                ),
            }
            continue;
        }

        insert_seen(&mut seen, &parts)?;
        match class {
            ArchiveEntryClass::Binary => {
                if found_binary {
                    bail!("Turbo archive contains duplicate {binary_entry}");
                }
                if entry.size() > MAX_BINARY_BYTES {
                    bail!("Turbo binary exceeds the decompressed size limit");
                }
                let mut out = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&binary_path)
                    .with_context(|| format!("creating binary stage {}", binary_path.display()))?;
                copy_limited(&mut entry, &mut out, MAX_BINARY_BYTES, "Turbo binary")?;
                out.sync_all()?;
                found_binary = true;
            }
            ArchiveEntryClass::Notice => {
                if entry.size() > MAX_AUXILIARY_BYTES {
                    bail!(
                        "Turbo archive auxiliary entry {} is too large",
                        display_parts(&parts)
                    );
                }
                drain_limited(
                    &mut entry,
                    MAX_AUXILIARY_BYTES,
                    &format!("Turbo archive auxiliary entry {}", display_parts(&parts)),
                )?;
            }
            ArchiveEntryClass::BundleFile => {
                if entry.size() > MAX_BUNDLE_FILE_BYTES {
                    bail!(
                        "Turbo archive bundle file {} exceeds the per-file size limit",
                        display_parts(&parts)
                    );
                }
                let rel: PathBuf = parts[1..].iter().collect();
                if rel.as_os_str().is_empty() {
                    bail!("Turbo archive bundle file path is empty");
                }
                let dest = bundle_stage.join(&rel);
                ensure_parent_dirs(&dest)?;
                let mut out = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&dest)
                    .with_context(|| {
                        format!(
                            "creating bundle stage file {} -> {}",
                            display_parts(&parts),
                            dest.display()
                        )
                    })?;
                let copied = copy_limited(
                    &mut entry,
                    &mut out,
                    MAX_BUNDLE_FILE_BYTES,
                    &format!("Turbo archive bundle file {}", display_parts(&parts)),
                )?;
                out.sync_all()?;
                limits.count_bundle_file(copied)?;
                wrote_bundle = true;
            }
            ArchiveEntryClass::BundleRootDir | ArchiveEntryClass::BundleDir => bail!(
                "Turbo archive directory entry {} is not a regular file",
                display_parts(&parts)
            ),
            ArchiveEntryClass::RootPlaceholder => {
                bail!("Turbo archive contains an unnamed regular entry")
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if found_binary {
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
        }
    }

    finish_extracted(
        binary_path,
        binary_guard,
        bundle_stage,
        bundle_guard,
        wrote_bundle,
        found_binary,
        binary_entry,
    )
}

/// Extraction entry point. Returns both the staged binary and, when the
/// release shipped one, the staged `bundled/` tree ready for activation.
async fn extract_archive(
    archive: &Path,
    stage_root: &Path,
    bundle_stage: PathBuf,
    platform: Platform,
) -> Result<ExtractedArchive> {
    let archive = archive.to_owned();
    let stage_root = stage_root.to_owned();
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            // Unix releases ship tar.gz; dispatch on the extension anyway so a
            // zip fixture can exercise the Windows producer layout on Unix CI.
            let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".zip") {
                extract_zip_archive(&archive, &stage_root, bundle_stage, platform.binary_entry)
            } else {
                extract_tar_archive(&archive, &stage_root, bundle_stage, platform.binary_entry)
            }
        }
        #[cfg(windows)]
        {
            extract_zip_archive(&archive, &stage_root, bundle_stage, platform.binary_entry)
        }
        #[cfg(not(any(unix, windows)))]
        {
            bail!("unsupported Turbo archive format")
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("Turbo archive extraction task failed: {e}"))?
}

async fn smoke_test(binary: &Path) -> Result<()> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = tokio::time::timeout(SMOKE_TEST_TIMEOUT, command.status())
        .await
        .context("downloaded Turbo binary smoke test timed out")??;
    if !status.success() {
        bail!("downloaded Turbo binary failed its --version smoke test ({status})");
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn publish_versioned_binary(stage: &Path, destination: &Path) -> Result<()> {
    match std::fs::hard_link(stage, destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if sha256_file(stage)? != sha256_file(destination)? {
                bail!(
                    "existing checksum-addressed Turbo binary does not match the verified download: {}",
                    destination.display()
                );
            }
        }
        Err(_) => {
            // Some Windows/filesystem configurations disallow hard links. The
            // process lock means a create_new copy is still never observed by
            // another cooperating updater before it is complete.
            let mut src = File::open(stage)?;
            let mut dst = match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(destination)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if sha256_file(stage)? != sha256_file(destination)? {
                        bail!("existing Turbo binary conflicts with verified download");
                    }
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            if let Err(error) = std::io::copy(&mut src, &mut dst).and_then(|_| dst.sync_all()) {
                let _ = std::fs::remove_file(destination);
                return Err(error.into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    Ok(())
}

/// What the active application path looked like before activation, i.e. what a
/// rollback has to put back.
enum PreviousBinary {
    /// Nothing was installed yet — rollback means removing what we published.
    Missing,
    /// Unix: `bin/turbo` was a symlink; the target is enough to recreate it.
    #[cfg(unix)]
    Symlink { target: PathBuf },
    /// Unix: `bin/turbo` was a real file (older installer layouts). It was
    /// moved aside because a symlink rename would otherwise destroy it.
    #[cfg(unix)]
    RegularAside { aside: PathBuf },
    /// Windows: the previous executable, moved aside so the new one can take
    /// its name.
    #[cfg(windows)]
    ExeAside { aside: PathBuf },
}

struct BinaryActivation {
    previous: PreviousBinary,
    /// Aside path kept alive until the state write commits, so a late failure
    /// can still restore the previous executable. Deleted on success.
    pending_aside: Option<PathBuf>,
}

fn relative_versioned_link_target(versioned: &Path) -> Result<PathBuf> {
    let downloads = hyper_home().join("downloads");
    let name = versioned
        .file_name()
        .context("versioned Turbo binary has no filename")?;
    let relative = Path::new("..").join(
        downloads
            .file_name()
            .context("Turbo downloads directory has no filename")?,
    );
    Ok(relative.join(name))
}

fn restore_previous_binary(app: &Path, previous: &PreviousBinary) -> Result<()> {
    match previous {
        PreviousBinary::Missing => {
            // A first install failed after publishing the active path: remove
            // it so a broken deployment is not left reachable on PATH.
            if path_exists_or_symlink(app)
                && let Some(doomed) = move_active_aside(app, "failed-new")?
            {
                let _ = std::fs::remove_file(&doomed);
                let _ = std::fs::remove_dir_all(&doomed);
            }
            Ok(())
        }
        #[cfg(unix)]
        PreviousBinary::Symlink { target } => {
            // Prefer an atomic replace: stage the restore link, rename over.
            let tmp = unique_sibling(app, "restore-link");
            let _ = std::fs::remove_file(&tmp);
            std::os::unix::fs::symlink(target, &tmp).with_context(|| {
                format!(
                    "staging restore symlink for {} -> {}",
                    app.display(),
                    target.display()
                )
            })?;
            if let Err(error) = std::fs::rename(&tmp, app) {
                let _ = std::fs::remove_file(&tmp);
                return Err(anyhow::Error::new(error).context(format!(
                    "restoring previous Turbo symlink at {}",
                    app.display()
                )));
            }
            Ok(())
        }
        #[cfg(unix)]
        PreviousBinary::RegularAside { aside } => restore_aside_over_active(app, aside),
        #[cfg(windows)]
        PreviousBinary::ExeAside { aside } => restore_aside_over_active(app, aside),
    }
}

/// Puts `aside` back at `app`. Neither Windows nor a non-empty Unix target can
/// be renamed over, so the failed-new artifact is moved out of the way first —
/// and put back if the restore itself fails, so the active path is never left
/// missing entirely.
#[cfg(any(unix, windows))]
fn restore_aside_over_active(app: &Path, aside: &Path) -> Result<()> {
    let doomed = move_active_aside(app, "failed-new").map_err(|move_err| {
        move_err.context(format!(
            "cannot clear active Turbo at {} before restoring {}",
            app.display(),
            aside.display()
        ))
    })?;
    if let Err(error) = std::fs::rename(aside, app) {
        let restore_error = anyhow::Error::new(error).context(format!(
            "restoring previous Turbo executable from {} (aside preserved)",
            aside.display()
        ));
        // Better a broken-but-present active path than none at all: put the
        // failed-new artifact back if the real restore could not happen.
        if let Some(doomed) = doomed.as_ref()
            && let Err(republish_error) = std::fs::rename(doomed, app)
        {
            return Err(combine_errors(
                restore_error,
                anyhow::Error::new(republish_error).context(format!(
                    "republishing failed-new Turbo executable from {} to {}",
                    doomed.display(),
                    app.display()
                )),
            ));
        }
        return Err(restore_error);
    }
    if let Some(doomed) = doomed {
        let _ = std::fs::remove_file(doomed);
    }
    Ok(())
}

#[cfg(unix)]
fn activate_binary_transactional(versioned: &Path) -> Result<BinaryActivation> {
    let app = managed_application();
    let bin_dir = app.parent().context("Turbo application has no parent")?;

    // Inspect before mutating so an unsupported shape (directory, socket, …)
    // fails closed rather than half-way through activation.
    let meta = match std::fs::symlink_metadata(&app) {
        Ok(meta) => Some(meta),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", app.display()));
        }
    };

    let relative = relative_versioned_link_target(versioned)?;
    let tmp = unique_sibling(&app, "tmp-link");
    let tmp_guard = TempArtifact::new_file(tmp.clone());
    std::os::unix::fs::symlink(&relative, &tmp).context("staging Turbo activation symlink")?;

    let mut pending_aside = None;
    let previous = match meta {
        Some(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(&app)
                .with_context(|| format!("reading active Turbo symlink {}", app.display()))?;
            PreviousBinary::Symlink { target }
        }
        Some(meta) if meta.is_file() => {
            // Renaming a symlink over a regular file would destroy the previous
            // install outright; move it aside so rollback can restore it.
            let aside = unique_sibling(&app, "old-regular");
            std::fs::rename(&app, &aside).with_context(|| {
                format!(
                    "preserving existing Turbo regular file {} before activation",
                    app.display()
                )
            })?;
            pending_aside = Some(aside.clone());
            PreviousBinary::RegularAside { aside }
        }
        Some(_) => {
            bail!(
                "Turbo application path is not a regular file or symlink: {}",
                app.display()
            );
        }
        None => PreviousBinary::Missing,
    };

    if let Err(error) = std::fs::rename(&tmp, &app) {
        let activation_err = anyhow::Error::new(error).context(format!(
            "atomically activating Turbo at {} (bin dir {})",
            app.display(),
            bin_dir.display()
        ));
        // A failed rename leaves an existing symlink untouched; only the
        // moved-aside regular file needs putting back.
        if matches!(previous, PreviousBinary::RegularAside { .. })
            && let Err(restore_error) = restore_previous_binary(&app, &previous)
        {
            return Err(combine_errors(activation_err, restore_error));
        }
        return Err(activation_err);
    }
    let _ = tmp_guard.keep();
    Ok(BinaryActivation {
        previous,
        pending_aside,
    })
}

#[cfg(windows)]
fn activate_binary_transactional(versioned: &Path) -> Result<BinaryActivation> {
    let app = managed_application();
    reject_symlink(&app, "application")?;
    let staged = unique_sibling(&app, "new.exe");
    std::fs::copy(versioned, &staged)?;
    let staged_guard = TempArtifact::new_file(staged.clone());
    if sha256_file(versioned)? != sha256_file(&staged)? {
        bail!("copied Turbo executable failed activation integrity check");
    }
    let aside = unique_sibling(&app, "old.exe");
    let had_old = app.exists();
    if had_old {
        // Windows cannot rename *over* a running image, but it can rename that
        // image out of the way while it runs.
        std::fs::rename(&app, &aside).with_context(|| {
            format!(
                "cannot replace running {}; close all Turbo sessions and retry",
                app.display()
            )
        })?;
    }
    if let Err(error) = std::fs::rename(&staged, &app) {
        let activation_err =
            anyhow::Error::new(error).context("activating downloaded Turbo executable");
        if had_old && let Err(restore_error) = std::fs::rename(&aside, &app) {
            return Err(combine_errors(
                activation_err,
                anyhow::Error::new(restore_error).context(format!(
                    "failed to restore previous Turbo executable from {} (aside preserved)",
                    aside.display()
                )),
            ));
        }
        return Err(activation_err);
    }
    let _ = staged_guard.keep();
    // The aside survives until the install commits. A still-running old image
    // may keep it locked; that is harmless and a later update cleans it up.
    Ok(BinaryActivation {
        previous: if had_old {
            PreviousBinary::ExeAside {
                aside: aside.clone(),
            }
        } else {
            PreviousBinary::Missing
        },
        pending_aside: had_old.then_some(aside),
    })
}

#[cfg(not(any(unix, windows)))]
fn activate_binary_transactional(_versioned: &Path) -> Result<BinaryActivation> {
    bail!("unsupported platform for Turbo binary activation")
}

/// The bundle's parent must exist and be a real directory, and the bundle path
/// itself must not be a symlink — otherwise activation would rename into
/// whatever directory that link points at.
fn ensure_bundle_parent_ready(bundle_path: &Path) -> Result<()> {
    let parent = bundle_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("bundled runtime path has no parent directory")?;
    if parent.exists() {
        reject_symlink(parent, "bundled runtime parent")?;
        if !std::fs::metadata(parent)?.is_dir() {
            bail!(
                "bundled runtime parent is not a directory: {}",
                parent.display()
            );
        }
    } else {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating bundled runtime parent {}", parent.display()))?;
    }
    if path_exists_or_symlink(bundle_path) {
        reject_symlink(bundle_path, "bundled runtime directory")?;
    }
    Ok(())
}

/// Activates a staged bundle tree at `bundle_path` (`$GROK_HOME/bundled` in
/// production) using same-volume renames only. Returns the aside path of the
/// previous bundle, if any, so the caller can roll it back on a later failure
/// or delete it on commit.
///
/// Rename is the unit of work precisely because it is atomic: a crash at any
/// point leaves either the old bundle or the new one live, never a merge of the
/// two and never a partially written tree.
///
/// `bundle_path` is a parameter rather than a call to [`managed_bundle_path`]
/// so the transaction can be exercised against a scratch directory without
/// mutating process-global `GROK_HOME` state.
fn activate_bundle_transactional(bundle_path: &Path, stage: &Path) -> Result<Option<PathBuf>> {
    ensure_bundle_parent_ready(bundle_path)?;
    if !stage.is_dir() {
        bail!("bundle stage is not a directory: {}", stage.display());
    }
    reject_symlink(stage, "bundle stage")?;

    let aside = unique_sibling(bundle_path, "old");
    let had_old = path_exists_or_symlink(bundle_path);
    if had_old {
        reject_symlink(bundle_path, "bundled runtime directory")?;
        std::fs::rename(bundle_path, &aside).with_context(|| {
            format!(
                "moving existing bundled runtime {} aside",
                bundle_path.display()
            )
        })?;
    }
    if let Err(error) = std::fs::rename(stage, bundle_path) {
        let activation_err = anyhow::Error::new(error).context(format!(
            "activating bundled runtime at {} from stage {}",
            bundle_path.display(),
            stage.display()
        ));
        if had_old && let Err(restore_error) = std::fs::rename(&aside, bundle_path) {
            return Err(combine_errors(
                activation_err,
                anyhow::Error::new(restore_error).context(format!(
                    "failed to restore previous bundled runtime from {} (aside preserved)",
                    aside.display()
                )),
            ));
        }
        return Err(activation_err);
    }
    Ok(had_old.then_some(aside))
}

/// Undoes [`activate_bundle_transactional`]: the just-published tree is moved
/// out of the way and the previous one renamed back.
fn restore_bundle(bundle_path: &Path, aside: Option<&Path>) -> Result<()> {
    let doomed = if path_exists_or_symlink(bundle_path) {
        move_active_aside(bundle_path, "failed")?
    } else {
        None
    };

    if let Some(aside) = aside
        && let Err(error) = std::fs::rename(aside, bundle_path)
    {
        let restore_error = anyhow::Error::new(error).context(format!(
            "restoring previous bundled runtime from {} (aside preserved)",
            aside.display()
        ));
        // Better a stale-but-complete bundle than none at all: put the
        // failed-new tree back so the runtime still has skills to load.
        if let Some(doomed) = doomed.as_ref()
            && let Err(republish_error) = std::fs::rename(doomed, bundle_path)
        {
            return Err(combine_errors(
                restore_error,
                anyhow::Error::new(republish_error).context(format!(
                    "republishing failed-new bundled runtime from {} to {}",
                    doomed.display(),
                    bundle_path.display()
                )),
            ));
        }
        return Err(restore_error);
    }
    if let Some(doomed) = doomed {
        let _ = std::fs::remove_dir_all(&doomed);
        let _ = std::fs::remove_file(&doomed);
    }
    Ok(())
}

fn format_rollback_failure(
    commit_error: anyhow::Error,
    rollback_errors: Vec<anyhow::Error>,
) -> anyhow::Error {
    if rollback_errors.is_empty() {
        return commit_error;
    }
    let mut msg = format!(
        "Turbo community update failed and rollback was incomplete; \
         installation may be inconsistent.\n\ncommit error: {commit_error:#}"
    );
    for (index, error) in rollback_errors.iter().enumerate() {
        msg.push_str(&format!("\n\nrollback error {}: {error:#}", index + 1));
    }
    anyhow::anyhow!(msg)
}

async fn install_candidate(candidate: &Candidate) -> Result<()> {
    ensure_safe_layout()?;
    let platform = platform()?;
    let downloads = hyper_home().join("downloads");
    let archive_tmp = unique_sibling(&downloads.join(&candidate.asset_name), "download");
    let archive_guard = TempArtifact::new_file(archive_tmp.clone());
    eprintln!(
        "  Downloading Turbo v{} ({}) from community releases...",
        candidate.version, platform.asset_triple
    );
    let actual_sha = download_archive(candidate, &archive_tmp).await?;
    // Non-negotiable: nothing is unpacked before the published SHA256SUMS entry
    // matches the bytes that were actually downloaded.
    if actual_sha != candidate.sha256 {
        bail!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            candidate.asset_name,
            candidate.sha256,
            actual_sha
        );
    }

    // The binary stages under `downloads`; the bundle stages as a sibling of
    // its final home (the Grok home, not the Turbo one) so activation is a
    // same-volume rename.
    let extract_root = unique_sibling(&downloads.join("turbo-extracted"), "dir");
    std::fs::create_dir_all(&extract_root)
        .with_context(|| format!("creating extract root {}", extract_root.display()))?;
    let extract_root_guard = TempArtifact::new_dir(extract_root.clone());

    let bundle_path = managed_bundle_path();
    ensure_bundle_parent_ready(&bundle_path)?;
    let bundle_stage_path = unique_sibling(&bundle_path, "install");

    let extracted =
        extract_archive(&archive_tmp, &extract_root, bundle_stage_path, platform).await?;
    smoke_test(&extracted.binary).await?;

    let extension = if cfg!(windows) { ".exe" } else { "" };
    let binary_name = format!(
        "turbo-{}-{}-{}-sha256-{}{}",
        candidate.version, platform.local_os, platform.local_arch, candidate.sha256, extension
    );
    let versioned = downloads.join(&binary_name);
    publish_versioned_binary(&extracted.binary, &versioned)?;
    smoke_test(&versioned).await?;

    // Read the previous state before anything mutates: a symlinked or otherwise
    // unusable state file must fail closed while the deployment is untouched.
    let state_file = state_path();
    let previous_state_bytes = capture_previous_state_bytes(&state_file)?;

    // --- Compensating transaction: bundle -> binary -> state ---
    // The state write is the sole commit point. Any failure before it restores
    // the whole previous deployment, so users never end up running a new binary
    // against an old bundle (or the reverse).
    let mut bundle_aside: Option<PathBuf> = None;
    let mut bundle_activated = false;
    let mut binary_activation: Option<BinaryActivation> = None;
    let mut state_write_attempted = false;

    let commit_result: Result<()> = (|| {
        if let Some(stage) = extracted.bundle_stage.as_ref() {
            bundle_aside = activate_bundle_transactional(&bundle_path, stage)?;
            bundle_activated = true;
        }

        binary_activation = Some(activate_binary_transactional(&versioned)?);

        let state = UpdateState {
            installed_version: Some(candidate.version.clone()),
            installed_asset: Some(candidate.asset_name.clone()),
            installed_sha256: Some(candidate.sha256.clone()),
            installed_binary: Some(binary_name.clone()),
            checked_at_unix: Some(now_unix()),
        };
        state_write_attempted = true;
        write_state_atomic(&state)?;
        Ok(())
    })();

    if let Err(error) = commit_result {
        let mut rollback_errors = Vec::new();

        if let Some(activation) = binary_activation.as_ref()
            && let Err(restore_error) =
                restore_previous_binary(&managed_application(), &activation.previous)
        {
            rollback_errors.push(
                restore_error
                    .context("binary rollback failed; previous/active paths may be inconsistent"),
            );
        }

        if bundle_activated
            && let Err(restore_error) = restore_bundle(&bundle_path, bundle_aside.as_deref())
        {
            let aside_note = bundle_aside
                .as_ref()
                .map(|path| format!(" (aside preserved at {})", path.display()))
                .unwrap_or_default();
            rollback_errors.push(restore_error.context(format!(
                "bundle rollback failed{aside_note}; installation may be inconsistent"
            )));
        }

        if state_write_attempted
            && let Err(restore_error) =
                restore_state_bytes(&state_file, previous_state_bytes.as_deref())
        {
            rollback_errors.push(
                restore_error
                    .context("update-state rollback failed; installation may be inconsistent"),
            );
        }

        return Err(format_rollback_failure(error, rollback_errors));
    }

    // Committed. Asides are best-effort cleanup from here on; the versioned
    // binaries under `downloads` are kept on purpose so a pinned reinstall can
    // relink without re-downloading.
    if let Some(aside) = bundle_aside {
        let _ = std::fs::remove_dir_all(&aside);
        let _ = std::fs::remove_file(&aside);
    }
    if let Some(activation) = binary_activation
        && let Some(aside) = activation.pending_aside
    {
        let _ = std::fs::remove_file(aside);
    }
    drop(extracted);
    drop(extract_root_guard);
    drop(archive_guard);
    Ok(())
}

async fn converge(force: bool, pinned_version: Option<&str>) -> Result<ConvergeOutcome> {
    let _lock = acquire_update_lock().await?;
    let mut candidate = resolve_candidate(pinned_version).await?;
    let state = load_state();
    let active = active_deployment();

    // `--force-reinstall` without a pin should not downgrade a locally newer
    // build merely because the latest pointer rolled back. Reinstall the
    // active version's release instead.
    if force
        && pinned_version.is_none()
        && let Some(active) = &active
        && semver::Version::parse(&active.version)? > semver::Version::parse(&candidate.version)?
    {
        candidate = resolve_candidate(Some(&active.version)).await?;
    }

    let need_install = if force || pinned_version.is_some() {
        true
    } else {
        candidate_requires_install(&candidate, active.as_ref(), &state)?
    };
    if !need_install {
        reconcile_checked_state(&candidate, active.as_ref())?;
        return Ok(ConvergeOutcome {
            target: candidate.version,
            installed: false,
        });
    }

    install_candidate(&candidate).await?;
    Ok(ConvergeOutcome {
        target: candidate.version,
        installed: true,
    })
}

pub(crate) async fn latest_version() -> Result<String> {
    resolve_latest_release_version().await
}

pub(crate) async fn check_update_status() -> UpdateStatus {
    let current_version = xai_grok_version::installed();
    let current_config = xai_grok_shell::util::config::load_config().await;
    let latest_version = match resolve_latest_release_version().await {
        Ok(version) => version,
        Err(error) => {
            return UpdateStatus {
                current_version,
                latest_version: None,
                update_available: false,
                installer: Some(INSTALLER_NAME.to_string()),
                channel: "stable".to_string(),
                auto_update: current_config.cli.auto_update,
                error: Some(error.to_string()),
            };
        }
    };
    match resolve_candidate(None).await {
        Ok(candidate) => {
            let state = load_state();
            let active = active_deployment();
            let update_available =
                candidate_requires_install(&candidate, active.as_ref(), &state).unwrap_or(false);
            UpdateStatus {
                current_version,
                latest_version: Some(latest_version),
                update_available,
                installer: Some(INSTALLER_NAME.to_string()),
                channel: "stable".to_string(),
                auto_update: current_config.cli.auto_update,
                error: None,
            }
        }
        Err(error) => UpdateStatus {
            current_version,
            latest_version: Some(latest_version),
            update_available: false,
            installer: Some(INSTALLER_NAME.to_string()),
            channel: "stable".to_string(),
            auto_update: current_config.cli.auto_update,
            error: Some(format!(
                "latest release is known but not installable on this platform: {error}"
            )),
        },
    }
}

pub(crate) async fn auto_update_target() -> Option<(&'static str, String)> {
    if !automatic_entry_allowed() {
        return None;
    }
    let candidate = resolve_candidate(None).await.ok()?;
    let state = load_state();
    let active = active_deployment();
    candidate_requires_install(&candidate, active.as_ref(), &state)
        .ok()?
        .then_some((INSTALLER_NAME, candidate.version))
}

pub(crate) async fn ensure_latest_on_disk() -> Result<EnsureLatestOutcome> {
    let relaunch_before = running_differs_from_active();
    if !automatic_entry_allowed() {
        return Ok(EnsureLatestOutcome {
            installed: None,
            relaunch_needed: relaunch_before,
        });
    }
    let config = xai_grok_shell::util::config::load_config().await;
    if config.cli.auto_update == Some(false) {
        return Ok(EnsureLatestOutcome {
            installed: None,
            relaunch_needed: relaunch_before,
        });
    }
    if is_version_cache_fresh() {
        return Ok(EnsureLatestOutcome {
            installed: None,
            relaunch_needed: relaunch_before,
        });
    }
    let outcome = converge(false, None).await?;
    Ok(EnsureLatestOutcome {
        installed: outcome.installed.then_some(outcome.target),
        relaunch_needed: running_differs_from_active(),
    })
}

async fn spawn_update_subcommand(run_mode: UpdateRunMode) -> Result<Option<tokio::process::Child>> {
    let exe = std::env::current_exe()?;
    let mut command = tokio::process::Command::new(exe);
    command.arg("update");
    match run_mode {
        UpdateRunMode::Blocking => {
            let status = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .status()
                .await?;
            if !status.success() {
                bail!("turbo update failed with {status}");
            }
            Ok(None)
        }
        UpdateRunMode::NonBlocking => {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            xai_grok_tools::util::detach_command(&mut command);
            Ok(Some(command.spawn()?))
        }
    }
}

pub(crate) async fn check_update_background() -> BackgroundUpdateCheck {
    if !automatic_entry_allowed() {
        return BackgroundUpdateCheck {
            update: None,
            download: None,
        };
    }
    let config = xai_grok_shell::util::config::load_config().await;
    if config.cli.auto_update == Some(false) {
        return BackgroundUpdateCheck {
            update: None,
            download: None,
        };
    }
    if running_differs_from_active() {
        return BackgroundUpdateCheck {
            update: active_deployment().map(|active| UpdateAvailable {
                latest_version: active.version,
            }),
            download: None,
        };
    }
    if is_version_cache_fresh() {
        return BackgroundUpdateCheck {
            update: None,
            download: None,
        };
    }

    let candidate = match resolve_candidate(None).await {
        Ok(candidate) => candidate,
        Err(error) => {
            tracing::warn!("Turbo community update check failed: {error:#}");
            return BackgroundUpdateCheck {
                update: None,
                download: None,
            };
        }
    };
    let state = load_state();
    let active = active_deployment();
    let needs_install =
        candidate_requires_install(&candidate, active.as_ref(), &state).unwrap_or(false);
    if !needs_install {
        if let Err(error) = record_no_update(&candidate).await {
            tracing::debug!("failed to cache Turbo update check: {error:#}");
        }
        return BackgroundUpdateCheck {
            update: None,
            download: None,
        };
    }

    let download = match spawn_update_subcommand(UpdateRunMode::NonBlocking).await {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!("Turbo background update failed to start: {error:#}");
            None
        }
    };
    BackgroundUpdateCheck {
        update: Some(UpdateAvailable {
            latest_version: candidate.version,
        }),
        download,
    }
}

pub(crate) async fn run_update_if_available(
    run_mode: UpdateRunMode,
    _interactive: bool,
) -> Result<bool> {
    if !automatic_entry_allowed() || is_version_cache_fresh() {
        return Ok(false);
    }
    let config = xai_grok_shell::util::config::load_config().await;
    if config.cli.auto_update == Some(false) {
        return Ok(false);
    }
    if config.cli.auto_update.is_none()
        && let Err(error) = xai_grok_shell::util::config::update_config(|state| {
            if state.cli.auto_update.is_none() {
                state.cli.auto_update = Some(true);
            }
        })
        .await
    {
        tracing::warn!("failed to save Turbo auto-update setting: {error}");
    }

    let candidate = match resolve_candidate(None).await {
        Ok(candidate) => candidate,
        Err(error) => {
            tracing::debug!("Turbo community update check failed: {error:#}");
            return Ok(false);
        }
    };
    let state = load_state();
    let active = active_deployment();
    if !candidate_requires_install(&candidate, active.as_ref(), &state)? {
        record_no_update(&candidate).await?;
        return Ok(false);
    }
    let current = active
        .as_ref()
        .map(|active| active.version.as_str())
        .unwrap_or(xai_grok_version::VERSION);
    eprintln!(
        "A new Turbo community release is available: {} -> {} [stable]",
        current, candidate.version
    );
    let child = spawn_update_subcommand(run_mode).await?;
    drop(child);
    Ok(matches!(run_mode, UpdateRunMode::Blocking))
}

pub(crate) async fn run_update(
    force: bool,
    pinned_version: Option<&str>,
    channel_switch: Option<&str>,
) -> Result<Option<String>> {
    if let Some(channel) = channel_switch
        && channel != "stable"
    {
        bail!("Turbo community releases support only the stable channel");
    }
    if let Some(version) = pinned_version {
        semver::Version::parse(version)
            .with_context(|| format!("'{version}' is not a valid Turbo release version"))?;
    }

    let before = active_deployment();
    let before_version = before
        .as_ref()
        .map(|active| active.version.as_str())
        .unwrap_or(xai_grok_version::VERSION);
    eprintln!(
        "Checking Turbo community releases (installed: {before_version}, destination: {})...",
        managed_application().display()
    );
    let outcome = converge(force, pinned_version).await.map_err(|error| {
        anyhow::anyhow!(
            "Turbo community update failed: {error:#}\n\nReinstall with:\n  {}",
            if cfg!(windows) {
                "irm https://github.com/danmsheets-dev/turbo-grok-build/releases/latest/download/install.ps1 | iex"
            } else {
                "curl -fsSL https://github.com/danmsheets-dev/turbo-grok-build/releases/latest/download/install.sh | bash"
            }
        )
    })?;

    if pinned_version.is_some()
        && let Err(error) = xai_grok_shell::util::config::update_config(|state| {
            state.cli.auto_update = Some(false);
        })
        .await
    {
        tracing::warn!("failed to disable auto-update after pinned Turbo install: {error}");
    }

    if outcome.installed {
        eprintln!("  ✓ Turbo v{} installed successfully.", outcome.target);
        eprintln!("  Restart Turbo to use the new community build.");
    } else {
        eprintln!("Already up to date (Turbo {}).", outcome.target);
    }
    Ok(Some(outcome.target))
}

pub(crate) async fn run_install_target(target: Option<&str>) -> Result<()> {
    converge(true, target).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parser_accepts_gnu_and_star_formats() {
        let hash = "a".repeat(64);
        let asset = "turbo-0.2.113-x86_64-unknown-linux-gnu.tar.gz";
        assert_eq!(
            parse_manifest_checksum(&format!("{hash}  *{asset}\n"), asset).unwrap(),
            hash
        );
    }

    #[test]
    fn manifest_parser_rejects_duplicate_or_invalid_entries() {
        let asset = "turbo-0.2.113-x86_64-unknown-linux-gnu.tar.gz";
        let hash = "b".repeat(64);
        assert!(parse_manifest_checksum(&format!("bad  {asset}\n"), asset).is_err());
        assert!(
            parse_manifest_checksum(&format!("{hash}  {asset}\n{hash}  {asset}\n"), asset).is_err()
        );
    }

    #[test]
    fn managed_name_round_trips_version_and_digest() {
        let digest = "c".repeat(64);
        let name = format!("turbo-0.2.113-linux-x86_64-sha256-{digest}");
        assert_eq!(version_from_managed_name(&name).as_deref(), Some("0.2.113"));
        assert_eq!(
            digest_from_managed_name(&name).as_deref(),
            Some(digest.as_str())
        );
        assert_eq!(
            version_from_managed_name("turbo-0.2.113-linux-x86_64").as_deref(),
            Some("0.2.113")
        );
    }

    #[test]
    fn same_semver_uses_archive_digest_as_deployment_identity() {
        let old = "d".repeat(64);
        let new = "e".repeat(64);
        let active = ActiveDeployment {
            version: "0.2.113".to_string(),
            binary_name: format!("turbo-0.2.113-linux-x86_64-sha256-{old}"),
            sha256: Some(old.clone()),
        };
        let candidate = Candidate {
            version: "0.2.113".to_string(),
            asset_name: "asset".to_string(),
            archive_url: "https://example.invalid/asset".to_string(),
            sha256: new,
        };
        assert!(
            candidate_requires_install(&candidate, Some(&active), &UpdateState::default()).unwrap()
        );
        let same = Candidate {
            sha256: old,
            ..candidate
        };
        assert!(
            !candidate_requires_install(&same, Some(&active), &UpdateState::default()).unwrap()
        );
    }

    #[test]
    fn archive_paths_accept_bundled_subtree_and_never_escape() {
        // Tar mode (`allow_backslash_as_separator = false`).
        assert_eq!(
            normalize_archive_path("./turbo", false).unwrap().as_deref(),
            Some(["turbo".to_string()].as_slice())
        );
        // The whole point of the fix: a real release ships a nested bundle.
        assert_eq!(
            normalize_archive_path("bundled/skills/demo/SKILL.md", false)
                .unwrap()
                .as_deref(),
            Some(
                [
                    "bundled".to_string(),
                    "skills".to_string(),
                    "demo".to_string(),
                    "SKILL.md".to_string(),
                ]
                .as_slice()
            )
        );
        // Tar root placeholders.
        assert_eq!(normalize_archive_path(".", false).unwrap(), None);
        assert_eq!(normalize_archive_path("./", false).unwrap(), None);
        assert_eq!(normalize_archive_path("", false).unwrap(), None);

        // Escapes and absolute forms.
        assert!(normalize_archive_path("../turbo", false).is_err());
        assert!(normalize_archive_path("/turbo", false).is_err());
        assert!(normalize_archive_path("nested\\turbo", false).is_err());
        assert!(normalize_archive_path("bundled/../../evil", false).is_err());
        assert!(normalize_archive_path("C:/turbo", false).is_err());

        // Zip mode: `\` is a separator, but `..` is still an escape.
        assert_eq!(
            normalize_archive_path("bundled\\skills\\x.md", true)
                .unwrap()
                .as_deref(),
            Some(
                [
                    "bundled".to_string(),
                    "skills".to_string(),
                    "x.md".to_string(),
                ]
                .as_slice()
            )
        );
        assert!(normalize_archive_path("..\\evil", true).is_err());
        assert!(normalize_archive_path("bundled\\..\\..\\evil", true).is_err());

        // Windows-hostile component shapes, rejected on every platform.
        assert!(normalize_archive_path("bundled/foo:bar", false).is_err());
        assert!(normalize_archive_path("bundled/foo.", false).is_err());
        assert!(normalize_archive_path("bundled/foo ", false).is_err());
        assert!(normalize_archive_path("bundled/CON", false).is_err());
        assert!(normalize_archive_path("bundled/nul.txt", false).is_err());
        assert!(normalize_archive_path("bundled/COM1", false).is_err());
        assert!(normalize_archive_path("bundled/lpt9.md", false).is_err());

        // Depth overflow: `bundled` + 32 nested dirs + leaf = 34 components.
        let deep = format!("bundled/{}x.md", "a/".repeat(MAX_PATH_DEPTH));
        assert!(normalize_archive_path(&deep, false).is_err());
        let at_limit = format!("bundled/{}x.md", "a/".repeat(MAX_PATH_DEPTH - 2));
        assert!(normalize_archive_path(&at_limit, false).is_ok());
    }

    #[test]
    fn classify_accepts_bundle_files_but_not_other_nesting() {
        assert_eq!(
            classify_archive_entry(Some(&["turbo".into()]), "turbo").unwrap(),
            ArchiveEntryClass::Binary
        );
        assert_eq!(
            classify_archive_entry(Some(&["LICENSE".into()]), "turbo").unwrap(),
            ArchiveEntryClass::Notice
        );
        assert_eq!(
            classify_archive_entry(Some(&["bundled".into()]), "turbo").unwrap(),
            ArchiveEntryClass::BundleRootDir
        );
        assert_eq!(
            classify_archive_entry(
                Some(&["bundled".into(), "a".into(), "b.md".into()]),
                "turbo"
            )
            .unwrap(),
            ArchiveEntryClass::BundleFile
        );
        assert_eq!(
            classify_archive_entry(None, "turbo").unwrap(),
            ArchiveEntryClass::RootPlaceholder
        );
        // Nesting outside `bundled/` stays rejected — that is the guarantee the
        // old root-only contract provided, and it must survive the fix.
        assert!(classify_archive_entry(Some(&["nested".into(), "turbo".into()]), "turbo").is_err());
        assert!(classify_archive_entry(Some(&["README".into()]), "turbo").is_err());
    }

    /// Extraction stages: `stage` holds the binary, `bundle` is the (not yet
    /// created) bundle stage directory.
    fn extract_dirs(root: &Path, label: &str) -> (PathBuf, PathBuf) {
        let stage = root.join(format!("{label}-stage"));
        let bundle = root.join(format!("{label}-bundle"));
        std::fs::create_dir_all(&stage).unwrap();
        (stage, bundle)
    }

    fn write_test_zip(entries: &[(&str, bool, &[u8])], path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, is_dir, body) in entries {
            if *is_dir {
                let dir_name = if name.ends_with('/') {
                    (*name).to_string()
                } else {
                    format!("{name}/")
                };
                zip.add_directory(dir_name, options).unwrap();
            } else {
                zip.start_file(*name, options).unwrap();
                zip.write_all(body).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn real_layout_zip_extracts_binary_and_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("good.zip");
        write_test_zip(
            &[
                ("turbo.exe", false, b"MZ-fake-binary"),
                ("LICENSE", false, b"lic\n"),
                ("bundled/", true, b""),
                ("bundled/skills/", true, b""),
                ("bundled/skills/demo/", true, b""),
                ("bundled/skills/demo/SKILL.md", false, b"# skill\n"),
                ("bundled/agents/helper.md", false, b"# agent\n"),
            ],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "good");
        let extracted = extract_zip_archive(&archive, &stage, bundle_stage, "turbo.exe").unwrap();
        assert_eq!(std::fs::read(&extracted.binary).unwrap(), b"MZ-fake-binary");
        let bundle = extracted.bundle_stage.clone().expect("bundle present");
        assert_eq!(
            std::fs::read(bundle.join("skills/demo/SKILL.md")).unwrap(),
            b"# skill\n"
        );
        assert_eq!(
            std::fs::read(bundle.join("agents/helper.md")).unwrap(),
            b"# agent\n"
        );
        // Notices are drained, never staged next to the binary.
        assert!(!stage.join("LICENSE").exists());
    }

    #[test]
    fn binary_only_zip_leaves_no_bundle_stage() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("binary-only.zip");
        write_test_zip(
            &[
                ("turbo.exe", false, b"MZ"),
                ("THIRD-PARTY-NOTICES", false, b"tpn\n"),
            ],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "binonly");
        let extracted =
            extract_zip_archive(&archive, &stage, bundle_stage.clone(), "turbo.exe").unwrap();
        assert!(extracted.binary.is_file());
        assert!(extracted.bundle_stage.is_none());
        // An empty stage must not survive; activating it would wipe a good
        // bundle from a previous install.
        assert!(!bundle_stage.exists());
    }

    #[test]
    fn zip_extraction_rejects_unsafe_bundle_entries() {
        let dir = tempfile::tempdir().unwrap();
        let deep = format!("bundled/{}x.md", "a/".repeat(MAX_PATH_DEPTH));
        #[allow(clippy::type_complexity)]
        let cases: &[(&str, &[(&str, bool, &[u8])])] = &[
            // Zip-slip: the bundle prefix must not license traversal.
            (
                "zip_slip",
                &[
                    ("turbo.exe", false, b"MZ"),
                    ("bundled/../../evil", false, b"pwned"),
                ],
            ),
            (
                "backslash_parent",
                &[("turbo.exe", false, b"MZ"), ("..\\evil", false, b"pwned")],
            ),
            ("absolute", &[("/turbo.exe", false, b"MZ")]),
            // Windows reserved device name inside the bundle.
            (
                "reserved_device",
                &[("turbo.exe", false, b"MZ"), ("bundled/CON", false, b"x")],
            ),
            (
                "reserved_device_with_extension",
                &[
                    ("turbo.exe", false, b"MZ"),
                    ("bundled/skills/nul.md", false, b"x"),
                ],
            ),
            (
                "trailing_space",
                &[("turbo.exe", false, b"MZ"), ("bundled/foo ", false, b"x")],
            ),
            // Nesting outside the bundle is still forbidden.
            (
                "nested_outside_bundle",
                &[("turbo.exe", false, b"MZ"), ("nested/turbo", false, b"x")],
            ),
            (
                "unexpected_root",
                &[("turbo.exe", false, b"MZ"), ("README", false, b"nope")],
            ),
            // Case-folding collision would be one file on NTFS/APFS.
            (
                "case_collision",
                &[
                    ("turbo.exe", false, b"MZ"),
                    ("bundled/A.md", false, b"a"),
                    ("bundled/a.md", false, b"b"),
                ],
            ),
            (
                "duplicate_binary",
                &[("turbo.exe", false, b"one"), ("TURBO.EXE", false, b"two")],
            ),
            ("missing_binary", &[("LICENSE", false, b"lic\n")]),
        ];
        for (label, entries) in cases {
            let archive = dir.path().join(format!("{label}.zip"));
            write_test_zip(entries, &archive);
            let (stage, bundle_stage) = extract_dirs(dir.path(), label);
            let result = extract_zip_archive(&archive, &stage, bundle_stage, "turbo.exe");
            assert!(
                result.is_err(),
                "case {label} must be rejected, got {:?}",
                result.err()
            );
        }

        // Depth overflow, built separately because the name is generated.
        let archive = dir.path().join("depth.zip");
        write_test_zip(
            &[("turbo.exe", false, b"MZ"), (deep.as_str(), false, b"x")],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "depth");
        assert!(extract_zip_archive(&archive, &stage, bundle_stage, "turbo.exe").is_err());
    }

    #[test]
    fn zip_accepts_backslash_separated_bundle_paths() {
        // PowerShell's Compress-Archive is a real producer for the Windows
        // asset and emits `\` separators.
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("backslash.zip");
        write_test_zip(
            &[
                ("turbo.exe", false, b"MZ"),
                ("bundled\\skills\\demo\\SKILL.md", false, b"# skill\n"),
            ],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "backslash");
        let extracted = extract_zip_archive(&archive, &stage, bundle_stage, "turbo.exe").unwrap();
        let bundle = extracted.bundle_stage.clone().expect("bundle present");
        assert_eq!(
            std::fs::read(bundle.join("skills/demo/SKILL.md")).unwrap(),
            b"# skill\n"
        );
    }

    #[cfg(unix)]
    fn write_test_tar(entries: &[(&str, tar::EntryType, &[u8])], path: &Path) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, kind, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_mode(if kind.is_dir() { 0o755 } else { 0o644 });
            if *kind == tar::EntryType::Symlink {
                header.set_size(0);
                header.set_link_name("outside").unwrap();
                header.set_cksum();
                builder
                    .append_data(&mut header, *name, &[] as &[u8])
                    .unwrap();
            } else if kind.is_dir() {
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, *name, &[] as &[u8])
                    .unwrap();
            } else {
                header.set_size(body.len() as u64);
                header.set_cksum();
                builder.append_data(&mut header, *name, *body).unwrap();
            }
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn real_layout_tar_extracts_binary_licenses_and_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("good.tar.gz");
        write_test_tar(
            &[
                (".", tar::EntryType::Directory, b"".as_slice()),
                ("turbo", tar::EntryType::Regular, b"#!/bin/sh\nexit 0\n"),
                ("LICENSE", tar::EntryType::Regular, b"lic\n"),
                ("THIRD-PARTY-NOTICES", tar::EntryType::Regular, b"tpn\n"),
                ("bundled", tar::EntryType::Directory, b""),
                ("bundled/skills", tar::EntryType::Directory, b""),
                ("bundled/skills/demo", tar::EntryType::Directory, b""),
                (
                    "bundled/skills/demo/SKILL.md",
                    tar::EntryType::Regular,
                    b"# skill\n",
                ),
                (
                    "bundled/agents/helper.md",
                    tar::EntryType::Regular,
                    b"# agent\n",
                ),
            ],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "tar-good");
        let extracted = extract_tar_archive(&archive, &stage, bundle_stage, "turbo").unwrap();
        assert_eq!(
            std::fs::read(&extracted.binary).unwrap(),
            b"#!/bin/sh\nexit 0\n"
        );
        let bundle = extracted.bundle_stage.clone().expect("bundle present");
        assert_eq!(
            std::fs::read(bundle.join("skills/demo/SKILL.md")).unwrap(),
            b"# skill\n"
        );
        assert_eq!(
            std::fs::read(bundle.join("agents/helper.md")).unwrap(),
            b"# agent\n"
        );
        assert!(!stage.join("LICENSE").exists());
    }

    #[cfg(unix)]
    #[test]
    fn strict_tar_extraction_rejects_links_and_duplicate_binary() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad.tar.gz");
        write_test_tar(
            &[("turbo", tar::EntryType::Symlink, b"".as_slice())],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "tar-link");
        assert!(extract_tar_archive(&archive, &stage, bundle_stage, "turbo").is_err());
        assert!(!stage.join("turbo").exists());

        std::fs::remove_file(&archive).unwrap();
        write_test_tar(
            &[
                ("turbo", tar::EntryType::Regular, b"one".as_slice()),
                ("turbo", tar::EntryType::Regular, b"two".as_slice()),
            ],
            &archive,
        );
        let (stage, bundle_stage) = extract_dirs(dir.path(), "tar-dup");
        assert!(extract_tar_archive(&archive, &stage, bundle_stage, "turbo").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn tar_extraction_rejects_bundle_traversal_and_reserved_names() {
        let dir = tempfile::tempdir().unwrap();
        for (label, entry) in [
            ("slip", "bundled/../../evil"),
            ("device", "bundled/skills/COM1.md"),
            ("outside", "nested/turbo"),
        ] {
            let archive = dir.path().join(format!("{label}.tar.gz"));
            write_test_tar(
                &[
                    ("turbo", tar::EntryType::Regular, b"one".as_slice()),
                    (entry, tar::EntryType::Regular, b"x".as_slice()),
                ],
                &archive,
            );
            let (stage, bundle_stage) = extract_dirs(dir.path(), label);
            let result = extract_tar_archive(&archive, &stage, bundle_stage, "turbo");
            assert!(
                result.is_err(),
                "case {label} must be rejected, got {:?}",
                result.err()
            );
        }
    }

    /// Bundle activation is exercised against a scratch directory: the real
    /// `GROK_HOME` resolution is a process-wide `OnceLock`, so pointing it at a
    /// temp dir from a unit test would be order-dependent and flaky.
    fn scratch_bundle(root: &Path) -> PathBuf {
        let home = root.join("grok-home");
        std::fs::create_dir_all(&home).unwrap();
        home.join(BUNDLE_DIR_NAME)
    }

    #[test]
    fn bundle_activation_publishes_stage_and_rollback_restores_previous() {
        let dir = tempfile::tempdir().unwrap();
        let live = scratch_bundle(dir.path());

        // A previous install's bundle is already live.
        std::fs::create_dir_all(live.join("skills")).unwrap();
        std::fs::write(live.join("skills/old.md"), b"old").unwrap();

        // Stage the replacement as a same-volume sibling and activate it.
        let stage = unique_sibling(&live, "install");
        std::fs::create_dir_all(stage.join("skills")).unwrap();
        std::fs::write(stage.join("skills/new.md"), b"new").unwrap();

        let aside = activate_bundle_transactional(&live, &stage)
            .unwrap()
            .expect("previous bundle should be moved aside");
        assert_eq!(std::fs::read(live.join("skills/new.md")).unwrap(), b"new");
        assert!(!live.join("skills/old.md").exists());
        assert!(!stage.exists(), "stage must be consumed by the rename");

        // A later step failing must put the previous tree back verbatim.
        restore_bundle(&live, Some(&aside)).unwrap();
        assert_eq!(std::fs::read(live.join("skills/old.md")).unwrap(), b"old");
        assert!(!live.join("skills/new.md").exists());
    }

    #[test]
    fn bundle_activation_from_clean_home_rolls_back_to_no_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let live = scratch_bundle(dir.path());
        let stage = unique_sibling(&live, "install");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("only.md"), b"new").unwrap();

        // No previous bundle: nothing to move aside.
        assert!(
            activate_bundle_transactional(&live, &stage)
                .unwrap()
                .is_none()
        );
        assert!(live.join("only.md").is_file());

        restore_bundle(&live, None).unwrap();
        assert!(
            !live.exists(),
            "rollback must not leave a partial bundle live"
        );
    }

    #[test]
    fn bundle_activation_refuses_a_symlinked_target() {
        // A symlinked `bundled` would make the rename land wherever the link
        // points — outside the Grok home entirely.
        let dir = tempfile::tempdir().unwrap();
        let live = scratch_bundle(dir.path());
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&elsewhere, &live).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&elsewhere, &live).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if !linked {
            // Windows without Developer Mode cannot create symlinks; nothing to
            // assert on such a host.
            return;
        }

        let stage = unique_sibling(&live, "install");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("only.md"), b"new").unwrap();
        assert!(activate_bundle_transactional(&live, &stage).is_err());
    }

    #[test]
    fn state_bytes_round_trip_and_restore_removes_a_state_that_did_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("update-state.json");

        // Missing state is captured as `None`, not an error.
        assert!(capture_previous_state_bytes(&state).unwrap().is_none());

        write_state_bytes_atomic(&state, b"{\"installed_version\":\"0.2.113\"}\n").unwrap();
        let captured = capture_previous_state_bytes(&state).unwrap().unwrap();
        assert_eq!(captured, b"{\"installed_version\":\"0.2.113\"}\n");

        // Rollback to "there was no state" removes the file again.
        restore_state_bytes(&state, None).unwrap();
        assert!(!state.exists());

        // Rollback to captured bytes restores them verbatim.
        restore_state_bytes(&state, Some(&captured)).unwrap();
        assert_eq!(std::fs::read(&state).unwrap(), captured);
    }

    #[test]
    fn future_cache_timestamp_is_not_fresh() {
        let state = UpdateState {
            checked_at_unix: Some(now_unix() + 60),
            ..UpdateState::default()
        };
        assert!(!state_is_fresh(&state));
    }
}
