//! Dedicated Agent WebView profile paths (never the user's daily Chrome).

use std::path::PathBuf;

/// Env var that opts the host into a shared durable profile (`durable`).
pub const GROK_BROWSER_PROFILE_ENV: &str = "GROK_BROWSER_PROFILE";

/// Root user-data directory (`$GROK_HOME/agent-browser`).
///
/// This is the *parent* of per-session profiles. A host that writes here
/// directly shares cookies across every pager session — the rc.2 default,
/// and the privacy incident that rc.3 fixes.
pub fn agent_browser_user_data_dir() -> PathBuf {
    xai_grok_config::grok_home().join("agent-browser")
}

/// Profile directory for a sidecar session.
///
/// Default: `$GROK_HOME/agent-browser/sessions/<session_id>` so a later
/// session cannot inherit this session's LinkedIn/Indeed cookies.
/// `GROK_BROWSER_PROFILE=durable` (or `shared`) uses
/// `$GROK_HOME/agent-browser/durable` instead, for job-hunt continuity.
pub fn agent_browser_profile_dir(session_id: &str) -> PathBuf {
    let root = agent_browser_user_data_dir();
    match std::env::var(GROK_BROWSER_PROFILE_ENV) {
        Ok(value) => {
            let kind = value.trim().to_ascii_lowercase();
            if kind == "durable" || kind == "shared" || kind == "1" {
                return root.join("durable");
            }
        }
        Err(_) => {}
    }
    let sid = session_id.trim();
    if sid.is_empty() {
        root.join("sessions").join("_unnamed")
    } else {
        root.join("sessions").join(sid)
    }
}

/// Session-private named pipe for JSON-RPC between Turbo and `browser-host`.
pub fn pipe_name(session_id: &str) -> String {
    format!(r"\\.\pipe\turbo-browser-{session_id}")
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
    fn default_profile_is_session_scoped() {
        let dir = agent_browser_profile_dir("abc-session");
        assert_eq!(
            dir,
            agent_browser_user_data_dir()
                .join("sessions")
                .join("abc-session")
        );
    }
}
