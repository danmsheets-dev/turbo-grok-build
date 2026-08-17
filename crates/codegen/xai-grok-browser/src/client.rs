//! JSON-RPC client for the Agent WebView host.
//!
//! Task 2 adds the named-pipe transport. This module only exposes a stable
//! handle so later tools and the TUI can depend on `BrowserClient` now.

/// Client handle for a `turbo browser-host` sidecar.
///
/// The async JSON-RPC transport is implemented in a later task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserClient {
    session_id: String,
}

impl BrowserClient {
    /// Bind a client to a pager/session id (same segment used in the pipe name).
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
        }
    }

    /// Pager/session id this client will talk to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Named-pipe path for this session (`\\.\pipe\turbo-browser-<id>`).
    pub fn pipe_name(&self) -> String {
        crate::profile::pipe_name(&self.session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_pipe_name_matches_profile() {
        let client = BrowserClient::new("abc");
        assert_eq!(client.session_id(), "abc");
        assert_eq!(client.pipe_name(), crate::profile::pipe_name("abc"));
    }
}
