//! Filesystem path tables for sandbox profiles.
//!
//! Collects device files, temp directories, and essential writable paths.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ── Grok state directory ────────────────────────────────────────────────────

/// Grok state directory — always writable (`$GROK_HOME` or `~/.grok`).
pub(crate) fn grok_home() -> PathBuf {
    xai_grok_config::grok_home()
}

// ── Device files & directories ──────────────────────────────────────────────

/// Device files that need write access for normal tool operation.
///
/// Without write access to these, common programs (git, curl, ssh, compilers)
/// break because they can't open `/dev/null` as an output sink, allocate PTYs,
/// or seed RNGs.
///
/// These are individual files (use `allow_file`, not `allow_path`).
/// Directory nodes under `/dev` belong in [`DEVICE_DIRS`].
#[cfg(all(feature = "enforce", unix))]
pub(crate) const DEVICE_FILES: &[&str] = &[
    "/dev/null",    // output sink — used by virtually every CLI tool
    "/dev/zero",    // zero source — used by memory allocators
    "/dev/random",  // entropy — used by crypto/TLS
    "/dev/urandom", // entropy — used by crypto/TLS
    "/dev/tty",     // controlling terminal — used by git, ssh, gpg
    "/dev/ptmx",    // PTY allocation — used by terminal spawning
];

/// Device directories that need write access (use `allow_path`, not `allow_file`).
#[cfg(all(feature = "enforce", unix))]
pub(crate) const DEVICE_DIRS: &[&str] = &[
    "/dev/pts", // PTY slaves (Linux)
    "/dev/fd",  // fd table (symlink to /proc/self/fd on Linux; a directory)
];

// ── Temporary directories ───────────────────────────────────────────────────

/// Temporary directories that need write access.
///
/// On Linux, `/tmp` is the standard temp directory.
/// On macOS, programs use both `/tmp` (symlink to `/private/tmp`) and
/// `/private/var/folders/` (the real `TMPDIR` / `NSTemporaryDirectory()`).
/// git, compilers, and other tools write temp files to `$TMPDIR` which
/// resolves to `/private/var/folders/xx/.../T/` on macOS.
pub(crate) fn temp_writable_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")];

    // macOS: /tmp → /private/tmp, but the real TMPDIR is under /private/var/folders.
    // Also include /private/tmp since Seatbelt may resolve the symlink.
    if cfg!(target_os = "macos") {
        for p in ["/private/tmp", "/private/var/tmp", "/private/var/folders"] {
            let pb = PathBuf::from(p);
            if pb.exists() && pb.is_dir() {
                paths.push(pb);
            }
        }
    }

    // Respect $TMPDIR if it points somewhere else (e.g. custom Linux setups).
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        let pb = PathBuf::from(&tmpdir);
        if pb.exists() && pb.is_dir() && !paths.contains(&pb) {
            paths.push(pb);
        }
    }

    paths
}

// ── Essential writable paths ────────────────────────────────────────────────

/// Writable directory paths for profiles that allow workspace writes (workspace, devbox, strict).
/// Device files are handled separately via `allow_file` in `to_capability_set_with_config`.
pub(crate) fn essential_writable_paths(workspace: &Path) -> Vec<PathBuf> {
    let mut paths = vec![workspace.to_path_buf(), grok_home()];
    paths.extend(temp_writable_paths());
    paths
}

/// Writable directory paths for the read-only profile (minimal: just ~/.grok + temp).
/// Device files are handled separately via `allow_file` in `to_capability_set_with_config`.
pub(crate) fn essential_writable_paths_minimal() -> Vec<PathBuf> {
    let mut paths = vec![grok_home()];
    paths.extend(temp_writable_paths());
    paths
}

// ── Grok-home credential write-deny ─────────────────────────────────────────

/// Known credential basenames at `$GROK_HOME` (write-denied, still readable).
pub(crate) const GROK_HOME_CREDENTIAL_BASENAMES: &[&str] = &[
    "auth.json",
    "auth.json.lock",
    "credentials.json",
    "credentials.json.lock",
    "secrets.json",
    "token.json",
    "tokens.json",
];

/// Filename suffixes treated as credentials under `$GROK_HOME`.
pub(crate) const GROK_HOME_CREDENTIAL_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".crt"];

/// Directories skipped while scanning `$GROK_HOME` for credential files.
const GROK_HOME_CREDENTIAL_SCAN_SKIP: &[&str] = &[
    "sessions",
    "logs",
    "worktrees",
    "agent-browser",
    "memtrace",
    "plugin-worktrees",
    "workspace-trees",
    "cache",
    "tmp",
    "developer-log",
    "feature-request-log",
];

/// True when `path`'s basename is a grok-home credential (auth, keys, certs).
pub(crate) fn is_grok_home_credential_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    GROK_HOME_CREDENTIAL_BASENAMES.iter().any(|b| lower == *b)
        || GROK_HOME_CREDENTIAL_SUFFIXES
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

/// Credential paths under `$GROK_HOME` that confining profiles must write-deny.
///
/// Always includes well-known basenames (even if they do not exist yet, so
/// Seatbelt can block create). Also scans a shallow tree for `*.pem` / `*.key`
/// and similar, skipping bulky session/log/worktree directories.
pub(crate) fn grok_home_credential_write_deny_paths() -> Vec<PathBuf> {
    grok_home_credential_write_deny_paths_in(&grok_home())
}

/// Same as [`grok_home_credential_write_deny_paths`] for an explicit home.
pub(crate) fn grok_home_credential_write_deny_paths_in(home: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for name in GROK_HOME_CREDENTIAL_BASENAMES {
        let path = home.join(name);
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    collect_existing_credential_files(home, &mut out, &mut seen, 0);
    out
}

fn collect_existing_credential_files(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 2;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let skip = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                GROK_HOME_CREDENTIAL_SCAN_SKIP
                    .iter()
                    .any(|s| n.eq_ignore_ascii_case(s))
            });
            if !skip {
                collect_existing_credential_files(&path, out, seen, depth + 1);
            }
            continue;
        }
        if file_type.is_file() && is_grok_home_credential_file(&path) && seen.insert(path.clone()) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_file_matcher_covers_auth_and_keys() {
        assert!(is_grok_home_credential_file(Path::new("auth.json")));
        assert!(is_grok_home_credential_file(Path::new("AUTH.JSON")));
        assert!(is_grok_home_credential_file(Path::new("credentials.json")));
        assert!(is_grok_home_credential_file(Path::new("id_rsa.key")));
        assert!(is_grok_home_credential_file(Path::new("tls.pem")));
        assert!(is_grok_home_credential_file(Path::new("client.p12")));
        assert!(!is_grok_home_credential_file(Path::new("config.toml")));
        assert!(!is_grok_home_credential_file(Path::new("sandbox.toml")));
        assert!(!is_grok_home_credential_file(Path::new("session.json")));
    }

    #[test]
    fn credential_write_deny_lists_known_names_and_existing_pem() {
        let tmp = std::env::temp_dir().join(format!(
            "grok-cred-deny-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("keys")).unwrap();
        std::fs::write(tmp.join("tls.pem"), b"pem").unwrap();
        std::fs::write(tmp.join("keys").join("id.key"), b"key").unwrap();
        std::fs::write(tmp.join("config.toml"), b"ok").unwrap();
        std::fs::create_dir_all(tmp.join("sessions")).unwrap();
        std::fs::write(tmp.join("sessions").join("ignore.pem"), b"no").unwrap();

        let denied = grok_home_credential_write_deny_paths_in(&tmp);
        assert!(
            denied.iter().any(|p| p == &tmp.join("auth.json")),
            "auth.json must be write-denied even when missing: {denied:?}"
        );
        assert!(
            denied.iter().any(|p| p == &tmp.join("credentials.json")),
            "credentials.json must be write-denied: {denied:?}"
        );
        assert!(
            denied.iter().any(|p| p == &tmp.join("tls.pem")),
            "existing pem must be write-denied: {denied:?}"
        );
        assert!(
            denied.iter().any(|p| p == &tmp.join("keys").join("id.key")),
            "nested key must be write-denied: {denied:?}"
        );
        assert!(
            !denied.iter().any(|p| p.ends_with("config.toml")),
            "config.toml must stay writable: {denied:?}"
        );
        assert!(
            !denied
                .iter()
                .any(|p| p == &tmp.join("sessions").join("ignore.pem")),
            "sessions/ must not be scanned: {denied:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
