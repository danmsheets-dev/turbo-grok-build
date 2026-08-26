//! Named secrets vault: env `GROK_SECRET_*` or `$GROK_HOME/secrets/<name>` (0600).
//!
//! Raw values never appear in [`SecretHandle`], [`VaultError`] Display, or
//! `Debug` of [`SecretValue`]. Callers that need bytes for MCP env injection
//! go through [`SecretValue::expose`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Once, RwLock};

use serde::Serialize;

use crate::sanitizer::MIN_VAULT_REDACT_LEN;

const SECRETS_SUBDIR: &str = "secrets";
const ENV_PREFIX: &str = "GROK_SECRET_";
const MAX_NAME_LEN: usize = 64;
const MAX_SECRET_BYTES: u64 = 64 * 1024;

static PROCESS_VAULT_VALUES: RwLock<Vec<String>> = RwLock::new(Vec::new());
static ENSURE_VAULT_ONCE: Once = Once::new();

/// Source of a named secret. Never carries the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretSource {
    Env,
    File,
}

/// Opaque handle returned to CLI / agent tools. Never contains secret bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretHandle {
    pub handle: String,
    pub name: String,
    pub source: SecretSource,
}

impl fmt::Display for SecretHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.handle)
    }
}

impl SecretHandle {
    fn new(name: &str, source: SecretSource) -> Self {
        Self {
            handle: format!("vault:{name}"),
            name: name.to_owned(),
            source,
        }
    }
}

/// Secret bytes. `Debug` is always `[redacted]`.
#[derive(Clone)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn expose(&self) -> &str {
        &self.0
    }

    fn from_raw(raw: String) -> Result<Self, VaultError> {
        if raw.is_empty() {
            return Err(VaultError::Empty);
        }
        Ok(Self(raw))
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue([redacted])")
    }
}

#[derive(Clone)]
struct Entry {
    value: SecretValue,
    source: SecretSource,
}

/// In-memory named vault.
#[derive(Clone, Default)]
pub struct Vault {
    entries: BTreeMap<String, Entry>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("names", &self.names())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("invalid secret name")]
    InvalidName,
    #[error("secret not found")]
    NotFound,
    #[error("secret file is empty")]
    Empty,
    #[error("secret file permissions are not owner-only (0600)")]
    InsecurePermissions,
    #[error("secret path is not a regular file")]
    NotAFile,
    #[error("secret file is too large")]
    TooLarge,
    #[error("secret directory is not a directory")]
    NotADirectory,
    #[error("io error loading secrets")]
    Io,
}

impl From<io::Error> for VaultError {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

impl VaultError {
    /// Fail-closed CLI / tool mapping: missing vs other.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }
}

/// `$GROK_HOME/secrets`.
pub fn secrets_dir(grok_home: &Path) -> PathBuf {
    grok_home.join(SECRETS_SUBDIR)
}

/// `GROK_SECRET_<NAME>` with `-` folded to `_` and ASCII uppercased.
pub fn env_key_for_secret_name(name: &str) -> String {
    let mut out = String::with_capacity(ENV_PREFIX.len() + name.len());
    out.push_str(ENV_PREFIX);
    for c in name.chars() {
        let mapped = if c == '-' { '_' } else { c };
        out.extend(mapped.to_uppercase());
    }
    out
}

pub fn is_valid_secret_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Vault {
    pub fn load() -> Result<Self, VaultError> {
        Self::load_from_home(&xai_grok_config::grok_home())
    }

    /// Files under `$home/secrets/` then env overlay (`GROK_SECRET_*` wins).
    pub fn load_from_home(home: &Path) -> Result<Self, VaultError> {
        let mut vault = Self::load_from_dir(&secrets_dir(home))?;
        vault.merge_env();
        Ok(vault)
    }

    /// Load named files from a secrets directory. Missing dir → empty vault.
    pub fn load_from_dir(dir: &Path) -> Result<Self, VaultError> {
        let mut vault = Self::default();
        match fs::metadata(dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vault),
            Err(_) => return Err(VaultError::Io),
            Ok(meta) if !meta.is_dir() => return Err(VaultError::NotADirectory),
            Ok(_) => {}
        }
        let rd = fs::read_dir(dir).map_err(|_| VaultError::Io)?;
        for ent in rd {
            let ent = ent.map_err(|_| VaultError::Io)?;
            let path = ent.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if name.starts_with('.') {
                continue;
            }
            if !is_valid_secret_name(name) {
                continue;
            }
            let file_type = ent.file_type().map_err(|_| VaultError::Io)?;
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(VaultError::NotAFile);
            }
            vault.insert_file(name, &path)?;
        }
        Ok(vault)
    }

    fn insert_file(&mut self, name: &str, path: &Path) -> Result<(), VaultError> {
        check_secure_permissions(path)?;
        let meta = fs::metadata(path).map_err(|_| VaultError::Io)?;
        if meta.len() > MAX_SECRET_BYTES {
            return Err(VaultError::TooLarge);
        }
        let raw = fs::read_to_string(path).map_err(|_| VaultError::Io)?;
        let trimmed = trim_single_trailing_newline(&raw);
        let value = SecretValue::from_raw(trimmed.to_owned())?;
        self.entries.insert(
            name.to_owned(),
            Entry {
                value,
                source: SecretSource::File,
            },
        );
        Ok(())
    }

    /// Overlay `GROK_SECRET_*` from the process environment. Env wins.
    pub fn merge_env(&mut self) {
        for name in self.names() {
            let key = env_key_for_secret_name(&name);
            if let Ok(val) = std::env::var(&key) {
                let trimmed = trim_single_trailing_newline(&val);
                if let Ok(value) = SecretValue::from_raw(trimmed.to_owned()) {
                    self.entries.insert(
                        name,
                        Entry {
                            value,
                            source: SecretSource::Env,
                        },
                    );
                }
            }
        }
        for (key, val) in std::env::vars() {
            let Some(suffix) = key.strip_prefix(ENV_PREFIX) else {
                continue;
            };
            if suffix.is_empty() || !is_valid_secret_name(suffix) {
                continue;
            }
            if self
                .entries
                .keys()
                .any(|n| env_key_for_secret_name(n).eq_ignore_ascii_case(&key))
            {
                continue;
            }
            let trimmed = trim_single_trailing_newline(&val);
            let Ok(value) = SecretValue::from_raw(trimmed.to_owned()) else {
                continue;
            };
            self.entries.insert(
                suffix.to_owned(),
                Entry {
                    value,
                    source: SecretSource::Env,
                },
            );
        }
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Handle only — never the raw secret. Missing → [`VaultError::NotFound`].
    pub fn get_handle(&self, name: &str) -> Result<SecretHandle, VaultError> {
        if !is_valid_secret_name(name) {
            return Err(VaultError::InvalidName);
        }
        let entry = self.entries.get(name).ok_or(VaultError::NotFound)?;
        Ok(SecretHandle::new(name, entry.source))
    }

    /// Bytes for MCP env injection only. Missing → fail closed.
    pub fn get_value(&self, name: &str) -> Result<&SecretValue, VaultError> {
        if !is_valid_secret_name(name) {
            return Err(VaultError::InvalidName);
        }
        self.entries
            .get(name)
            .map(|e| &e.value)
            .ok_or(VaultError::NotFound)
    }

    pub fn values_for_redaction(&self) -> Vec<String> {
        let mut values: Vec<String> = self
            .entries
            .values()
            .map(|e| e.value.expose().to_owned())
            .filter(|v| v.len() >= MIN_VAULT_REDACT_LEN)
            .collect();
        values.sort_by_key(|s| std::cmp::Reverse(s.len()));
        values.dedup();
        values
    }

    pub fn install_for_redaction(&self) {
        install_vault_redaction_values(self.values_for_redaction());
    }

    /// Resolve `secret:<name>`, `vault:<name>`, and `${secret:<name>}` placeholders.
    /// Fail closed if a well-formed ref is missing.
    pub fn resolve_placeholders(&self, input: &str) -> Result<String, VaultError> {
        if let Some(name) = whole_value_secret_ref(input) {
            return Ok(self.get_value(name)?.expose().to_owned());
        }
        interpolate_secret_placeholders(self, input)
    }
}

/// Inject vault secrets into an MCP stdio env map.
///
/// 1. Resolve `secret:` / `${secret:}` placeholders in existing values (fail closed).
/// 2. Add `GROK_SECRET_<NAME>` for every loaded secret not already present.
pub fn apply_vault_to_mcp_env(
    vault: &Vault,
    env: &mut Vec<(String, String)>,
) -> Result<(), VaultError> {
    for (_key, value) in env.iter_mut() {
        *value = vault.resolve_placeholders(value)?;
    }
    for name in vault.names() {
        let key = env_key_for_secret_name(&name);
        if env.iter().any(|(k, _)| k == &key) {
            continue;
        }
        let value = vault.get_value(&name)?.expose().to_owned();
        env.push((key, value));
    }
    Ok(())
}

pub fn install_vault_redaction_values<I: IntoIterator<Item = String>>(values: I) {
    let mut vals: Vec<String> = values
        .into_iter()
        .filter(|v| v.len() >= MIN_VAULT_REDACT_LEN)
        .collect();
    vals.sort_by_key(|s| std::cmp::Reverse(s.len()));
    vals.dedup();
    if let Ok(mut guard) = PROCESS_VAULT_VALUES.write() {
        *guard = vals;
    }
}

pub fn process_vault_redaction_values() -> Vec<String> {
    PROCESS_VAULT_VALUES
        .read()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Load `$GROK_HOME/secrets` once into the process redactor. Fail-open on I/O
/// (redaction still applies regex shapes). `get` / MCP inject stay fail-closed.
pub fn ensure_process_vault_redaction() {
    ENSURE_VAULT_ONCE.call_once(|| {
        if let Ok(vault) = Vault::load() {
            vault.install_for_redaction();
        }
    });
}

fn trim_single_trailing_newline(s: &str) -> &str {
    s.strip_suffix("\r\n")
        .or_else(|| s.strip_suffix('\n'))
        .unwrap_or(s)
}

fn whole_value_secret_ref(input: &str) -> Option<&str> {
    let name = input
        .strip_prefix("secret:")
        .or_else(|| input.strip_prefix("vault:"))?;
    if is_valid_secret_name(name) {
        Some(name)
    } else {
        None
    }
}

fn interpolate_secret_placeholders(vault: &Vault, input: &str) -> Result<String, VaultError> {
    const NEEDLE: &str = "${secret:";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find(NEEDLE) {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + NEEDLE.len()..];
        let Some(end) = after.find('}') else {
            return Err(VaultError::InvalidName);
        };
        let name = &after[..end];
        if !is_valid_secret_name(name) {
            return Err(VaultError::InvalidName);
        }
        out.push_str(vault.get_value(name)?.expose());
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn check_secure_permissions(path: &Path) -> Result<(), VaultError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|_| VaultError::Io)?
            .permissions()
            .mode();
        // Owner read/write only (0600) or owner-read (0400). Any group/other bit fails closed.
        if mode & 0o077 != 0 {
            return Err(VaultError::InsecurePermissions);
        }
        if mode & 0o400 == 0 {
            return Err(VaultError::InsecurePermissions);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Serialize a handle without risk of leaking values (tests / CLI `--json`).
pub fn handle_json(handle: &SecretHandle) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(handle)
}

/// `turbo secret get <name>` / `secret_get` tool entry.
///
/// Returns `vault:<name>` or pretty JSON of [`SecretHandle`]. Never the
/// secret bytes. Missing names fail closed.
pub fn secret_get(name: &str, json: bool) -> Result<String, VaultError> {
    secret_get_from(&Vault::load()?, name, json)
}

pub fn secret_get_from(vault: &Vault, name: &str, json: bool) -> Result<String, VaultError> {
    vault.install_for_redaction();
    let handle = vault.get_handle(name)?;
    if json {
        handle_json(&handle).map_err(|_| VaultError::Io)
    } else {
        Ok(handle.handle)
    }
}

/// Env-var tests mutate process env; serialize them.
#[cfg(test)]
pub(crate) fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{REDACTED, redact_secrets_with_values};
    use std::borrow::Cow;

    /// Join fragments at runtime so secret-shaped fixtures never sit
    /// contiguously in source (and never in snapshots).
    fn fixture(parts: &[&str]) -> String {
        parts.concat()
    }

    fn write_secret(dir: &Path, name: &str, value: &str) {
        let path = dir.join(name);
        fs::write(&path, value).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms).unwrap();
        }
    }

    #[test]
    fn missing_secret_fails_closed() {
        let vault = Vault::default();
        let err = vault.get_handle("github_token").unwrap_err();
        assert!(err.is_not_found(), "{err}");
        assert!(vault.get_value("github_token").is_err());
    }

    #[test]
    fn invalid_name_fails_closed() {
        let vault = Vault::default();
        assert!(matches!(
            vault.get_handle("../etc/passwd"),
            Err(VaultError::InvalidName)
        ));
        assert!(matches!(
            vault.get_handle("has space"),
            Err(VaultError::InvalidName)
        ));
        assert!(!is_valid_secret_name(""));
        assert!(!is_valid_secret_name("a/b"));
        assert!(!is_valid_secret_name(".hidden"));
    }

    #[test]
    fn get_returns_handle_never_value() {
        let canary = fixture(&["vaulttest", "CanaryValue99"]);
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "ci_token", &canary);
        let vault = Vault::load_from_dir(dir.path()).unwrap();
        let handle = vault.get_handle("ci_token").unwrap();
        assert_eq!(handle.handle, "vault:ci_token");
        assert_eq!(handle.name, "ci_token");
        assert_eq!(handle.source, SecretSource::File);
        let rendered = format!(
            "{handle:?}{handle}{}",
            serde_json::to_string(&handle).unwrap()
        );
        assert!(
            !rendered.contains(&canary),
            "handle/debug/json must not contain vault bytes"
        );
        assert!(format!("{:?}", vault.get_value("ci_token").unwrap()).contains("[redacted]"));
        assert!(!format!("{:?}", vault.get_value("ci_token").unwrap()).contains(&canary));
    }

    #[test]
    fn load_from_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::load_from_dir(&dir.path().join("nope")).unwrap();
        assert!(vault.names().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn insecure_permissions_fail_closed() {
        use std::os::unix::fs::PermissionsExt;
        let canary = fixture(&["openmode", "CanaryValue99"]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leaky");
        fs::write(&path, &canary).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();
        let err = Vault::load_from_dir(dir.path()).unwrap_err();
        assert!(matches!(err, VaultError::InsecurePermissions));
        assert!(!format!("{err}").contains(&canary));
    }

    #[test]
    fn empty_file_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "blank", "");
        let err = Vault::load_from_dir(dir.path()).unwrap_err();
        assert!(matches!(err, VaultError::Empty));
    }

    #[test]
    fn env_overrides_file() {
        let _guard = env_test_lock();
        let file_canary = fixture(&["filecanary", "ValueAAA11"]);
        let env_canary = fixture(&["envcanary", "ValueBBB22"]);
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "TOKEN_X", &file_canary);
        let key = env_key_for_secret_name("TOKEN_X");
        // SAFETY: serialized by env_test_lock; restored before guard drop.
        unsafe {
            std::env::set_var(&key, &env_canary);
        }
        let mut vault = Vault::load_from_dir(dir.path()).unwrap();
        vault.merge_env();
        unsafe {
            std::env::remove_var(&key);
        }
        assert_eq!(vault.get_value("TOKEN_X").unwrap().expose(), env_canary);
        assert_eq!(
            vault.get_handle("TOKEN_X").unwrap().source,
            SecretSource::Env
        );
        assert_ne!(vault.get_value("TOKEN_X").unwrap().expose(), file_canary);
    }

    #[test]
    fn missing_placeholder_fails_closed() {
        let vault = Vault::default();
        assert!(matches!(
            vault.resolve_placeholders("secret:nope_token"),
            Err(VaultError::NotFound)
        ));
        assert!(matches!(
            vault.resolve_placeholders("prefix-${secret:nope_token}-suffix"),
            Err(VaultError::NotFound)
        ));
    }

    #[test]
    fn resolve_and_mcp_inject_without_leaking_into_debug_vault() {
        let canary = fixture(&["mcpinject", "CanaryValue99"]);
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "svc_token", &canary);
        let vault = Vault::load_from_dir(dir.path()).unwrap();
        assert_eq!(
            vault.resolve_placeholders("secret:svc_token").unwrap(),
            canary
        );
        assert_eq!(
            vault
                .resolve_placeholders("Bearer ${secret:svc_token}")
                .unwrap(),
            format!("Bearer {canary}")
        );

        let mut env = vec![
            ("EXISTING".into(), "plain".into()),
            ("AUTH".into(), "secret:svc_token".into()),
        ];
        apply_vault_to_mcp_env(&vault, &mut env).unwrap();
        let auth = env
            .iter()
            .find(|(k, _)| k == "AUTH")
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(auth, canary);
        let injected = env
            .iter()
            .find(|(k, _)| k == &env_key_for_secret_name("svc_token"))
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_eq!(injected, canary);
        // Vault Debug / handle snapshots must never carry bytes.
        let snap = format!("{vault:?}");
        assert!(!snap.contains(&canary), "vault debug leaked bytes: {snap}");
        assert!(snap.contains("svc_token"));
    }

    #[test]
    fn redaction_of_vault_values_strips_canary() {
        let canary = fixture(&["redactme", "VaultValue88"]);
        let extra = vec![canary.clone()];
        let line = format!("log line with {canary} in the middle");
        let out = redact_secrets_with_values(&line, &extra);
        assert!(
            matches!(out, Cow::Owned(_)),
            "vault hit must own the redacted string"
        );
        assert!(!out.contains(&canary), "canary must not survive redaction");
        assert!(out.contains(REDACTED), "expected {REDACTED} in {out}");
        // Snapshot-shaped string: never the canary.
        let snapshot = out.into_owned();
        assert_eq!(snapshot, format!("log line with {REDACTED} in the middle"));
        assert!(!snapshot.contains("VaultValue"));
        assert!(!snapshot.contains("redactme"));
    }

    #[test]
    fn json_walk_redacts_installed_vault_values() {
        let canary = fixture(&["jsoncanary", "VaultValue88"]);
        install_vault_redaction_values([canary.clone()]);
        let mut value = serde_json::json!({
            "summary": format!("oops {canary}"),
            "nested": { "note": canary }
        });
        crate::redact_json_string_values(&mut value);
        let dumped = value.to_string();
        assert!(!dumped.contains(&canary));
        assert!(dumped.contains(REDACTED));
        install_vault_redaction_values(Vec::<String>::new());
    }

    #[test]
    fn env_key_folds_hyphen() {
        assert_eq!(
            env_key_for_secret_name("github-token"),
            "GROK_SECRET_GITHUB_TOKEN"
        );
        assert_eq!(env_key_for_secret_name("ci_token"), "GROK_SECRET_CI_TOKEN");
    }

    #[test]
    fn secret_get_emits_handle_not_bytes() {
        let canary = fixture(&["cliget", "CanaryValue99"]);
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "ci_token", &canary);
        let vault = Vault::load_from_dir(dir.path()).unwrap();
        let out = secret_get_from(&vault, "ci_token", false).unwrap();
        assert_eq!(out, "vault:ci_token");
        assert!(!out.contains(&canary));
        let json = secret_get_from(&vault, "ci_token", true).unwrap();
        assert!(json.contains("vault:ci_token"));
        assert!(!json.contains(&canary));
        assert!(
            secret_get_from(&vault, "missing", false)
                .unwrap_err()
                .is_not_found()
        );
    }
}
