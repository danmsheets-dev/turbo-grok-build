//! Dedicated Agent WebView profile paths (never the user's daily Chrome).

use std::path::{Path, PathBuf};

/// Env var that selects persisted vs per-session isolation.
pub const GROK_BROWSER_PROFILE_ENV: &str = "GROK_BROWSER_PROFILE";

/// Env var that forces a throwaway temp profile (`1` / `true` / `yes` / `on`).
pub const GROK_BROWSER_FRESH_PROFILE_ENV: &str = "GROK_BROWSER_FRESH_PROFILE";

/// Root user-data directory (`$GROK_HOME/agent-browser`).
///
/// Default host profile: cookies persist across pager sessions (grok.com /
/// Imagine web login). Distinct from Chrome MCP `~/.grok/browser-profile`.
pub fn agent_browser_user_data_dir() -> PathBuf {
    xai_grok_config::grok_home().join("agent-browser")
}

/// Profile directory for a sidecar session.
///
/// Default (and `durable` / `shared` / `1`): `$GROK_HOME/agent-browser`.
/// `GROK_BROWSER_PROFILE=session|ephemeral|private`: per-session isolation
/// under `sessions/<session_id>`.
/// `GROK_BROWSER_FRESH_PROFILE=1` (true/yes/on): a unique temp dir.
pub fn agent_browser_profile_dir(session_id: &str) -> PathBuf {
    profile_dir_for(
        session_id,
        std::env::var(GROK_BROWSER_PROFILE_ENV).ok().as_deref(),
        std::env::var(GROK_BROWSER_FRESH_PROFILE_ENV)
            .ok()
            .as_deref(),
    )
}

fn env_flag_true(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn profile_dir_for(
    session_id: &str,
    profile_env: Option<&str>,
    fresh_env: Option<&str>,
) -> PathBuf {
    if env_flag_true(fresh_env) {
        return fresh_profile_temp_dir();
    }
    let root = agent_browser_user_data_dir();
    if let Some(value) = profile_env {
        let kind = value.trim().to_ascii_lowercase();
        if matches!(kind.as_str(), "session" | "ephemeral" | "private") {
            return session_profile_dir(&root, session_id);
        }
        // durable|shared|1 (and unset / other values) alias the persisted root.
    }
    root
}

fn session_profile_dir(root: &Path, session_id: &str) -> PathBuf {
    let sid = session_id.trim();
    if sid.is_empty() {
        root.join("sessions").join("_unnamed")
    } else {
        root.join("sessions").join(sid)
    }
}

fn fresh_profile_temp_dir() -> PathBuf {
    let unique = format!(
        "grok-agent-browser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    std::env::temp_dir().join(unique)
}

/// Session-private named pipe for JSON-RPC between Turbo and `browser-host`.
pub fn pipe_name(session_id: &str) -> String {
    format!(r"\\.\pipe\turbo-browser-{session_id}")
}

/// Delete `dir` if it exists. Does not read or log profile contents.
///
/// Returns `true` when a directory was removed, `false` when it was absent.
pub fn reset_profile_dir(dir: &Path) -> std::io::Result<bool> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Delete the persisted Agent WebView profile (`$GROK_HOME/agent-browser`).
pub fn reset_agent_browser_profile() -> std::io::Result<bool> {
    reset_profile_dir(&agent_browser_user_data_dir())
}

#[cfg(test)]
pub(crate) struct ProfileEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_profile: Option<std::ffi::OsString>,
    prev_fresh: Option<std::ffi::OsString>,
}

#[cfg(test)]
static PROFILE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
impl ProfileEnvGuard {
    pub(crate) fn lock_cleared() -> Self {
        let lock = PROFILE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_profile = std::env::var_os(GROK_BROWSER_PROFILE_ENV);
        let prev_fresh = std::env::var_os(GROK_BROWSER_FRESH_PROFILE_ENV);
        // SAFETY: tests serialize env mutations through PROFILE_ENV_LOCK.
        unsafe {
            std::env::remove_var(GROK_BROWSER_PROFILE_ENV);
            std::env::remove_var(GROK_BROWSER_FRESH_PROFILE_ENV);
        }
        Self {
            _lock: lock,
            prev_profile,
            prev_fresh,
        }
    }

    pub(crate) fn set_fresh(&self, value: &str) {
        // SAFETY: held together with PROFILE_ENV_LOCK via `_lock`.
        unsafe {
            std::env::set_var(GROK_BROWSER_FRESH_PROFILE_ENV, value);
        }
    }

    pub(crate) fn set_profile(&self, value: &str) {
        // SAFETY: held together with PROFILE_ENV_LOCK via `_lock`.
        unsafe {
            std::env::set_var(GROK_BROWSER_PROFILE_ENV, value);
        }
    }
}

#[cfg(test)]
impl Drop for ProfileEnvGuard {
    fn drop(&mut self) {
        // SAFETY: restores the values captured under PROFILE_ENV_LOCK.
        unsafe {
            match &self.prev_profile {
                Some(v) => std::env::set_var(GROK_BROWSER_PROFILE_ENV, v),
                None => std::env::remove_var(GROK_BROWSER_PROFILE_ENV),
            }
            match &self.prev_fresh {
                Some(v) => std::env::set_var(GROK_BROWSER_FRESH_PROFILE_ENV, v),
                None => std::env::remove_var(GROK_BROWSER_FRESH_PROFILE_ENV),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_dir_is_agent_browser_under_grok_home() {
        let dir = agent_browser_user_data_dir();
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some("agent-browser")
        );
        assert!(dir.starts_with(xai_grok_config::grok_home()));
    }

    #[test]
    fn pipe_name_uses_session_id() {
        assert_eq!(pipe_name("sess-1"), r"\\.\pipe\turbo-browser-sess-1");
    }

    #[test]
    fn default_profile_is_persisted_root() {
        let dir = profile_dir_for("abc-session", None, None);
        assert_eq!(dir, agent_browser_user_data_dir());
    }

    #[test]
    fn durable_aliases_persisted_root() {
        let root = agent_browser_user_data_dir();
        assert_eq!(profile_dir_for("abc-session", Some("durable"), None), root);
        assert_eq!(profile_dir_for("abc-session", Some("shared"), None), root);
        assert_eq!(profile_dir_for("abc-session", Some("1"), None), root);
    }

    #[test]
    fn session_ephemeral_private_are_per_session() {
        let expected = agent_browser_user_data_dir()
            .join("sessions")
            .join("abc-session");
        assert_eq!(
            profile_dir_for("abc-session", Some("session"), None),
            expected
        );
        assert_eq!(
            profile_dir_for("abc-session", Some("ephemeral"), None),
            expected
        );
        assert_eq!(
            profile_dir_for("abc-session", Some("private"), None),
            expected
        );
    }

    #[test]
    fn fresh_profile_flag_uses_temp_dir() {
        for flag in ["1", "true", "yes", "on", "TRUE", " Yes "] {
            let dir = profile_dir_for("abc-session", None, Some(flag));
            assert!(
                dir.starts_with(std::env::temp_dir()),
                "flag {flag:?} -> {dir:?}"
            );
            assert_ne!(dir, agent_browser_user_data_dir());
        }
        assert_eq!(
            profile_dir_for("abc-session", None, Some("0")),
            agent_browser_user_data_dir()
        );
    }

    #[test]
    fn fresh_profile_wins_over_session_isolation() {
        let dir = profile_dir_for("abc-session", Some("session"), Some("1"));
        assert!(dir.starts_with(std::env::temp_dir()));
        assert!(!dir.ends_with("abc-session"));
    }

    #[test]
    fn reset_profile_dir_deletes_tree_without_reading_contents() {
        let dir = std::env::temp_dir().join(format!(
            "grok-profile-reset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp profile dir");
        std::fs::write(dir.join("Cookies"), b"not-logged").expect("dummy cookie file");
        assert!(reset_profile_dir(&dir).expect("reset deletes"));
        assert!(!dir.exists());
        assert!(!reset_profile_dir(&dir).expect("missing is ok"));
    }
}
