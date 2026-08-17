//! Dedicated Agent WebView profile paths (never the user's daily Chrome).

use std::path::PathBuf;

/// User-data directory for the agent-owned WebView2 profile.
///
/// Always `$GROK_HOME/agent-browser` (typically `~/.grok/agent-browser`).
pub fn agent_browser_user_data_dir() -> PathBuf {
    xai_grok_config::grok_home().join("agent-browser")
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
}
