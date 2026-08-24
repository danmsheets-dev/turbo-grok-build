//! Error type for the CDP client.

/// Anything that can go wrong talking to a Chromium DevTools endpoint.
#[derive(Debug, thiserror::Error)]
pub enum CdpError {
    /// No Chromium-family browser was found to launch.
    #[error(
        "no Chromium browser found (looked for Edge and Chrome). Install Microsoft Edge, or set {env}"
    )]
    BrowserNotFound {
        /// Env var that overrides browser discovery.
        env: &'static str,
    },

    /// The browser process could not be spawned.
    #[error("could not launch {path}: {source}")]
    Spawn {
        /// Executable we tried to run.
        path: String,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// The browser started but never printed its DevTools endpoint.
    #[error("browser did not report a DevTools endpoint within {secs}s")]
    NoEndpoint {
        /// How long we waited.
        secs: u64,
    },

    /// The WebSocket transport failed.
    #[error("devtools websocket: {0}")]
    WebSocket(String),

    /// The connection closed while a command was in flight.
    #[error("devtools connection closed")]
    ConnectionClosed,

    /// A command exceeded its deadline.
    #[error("devtools command `{method}` timed out after {secs}s")]
    Timeout {
        /// CDP method that timed out.
        method: String,
        /// Deadline in seconds.
        secs: u64,
    },

    /// The browser returned an error object for a command.
    #[error("devtools `{method}` failed: {message}")]
    Protocol {
        /// CDP method that failed.
        method: String,
        /// Message reported by the browser.
        message: String,
    },

    /// A response was missing a field we require.
    #[error("devtools `{method}` response missing `{field}`")]
    MalformedResponse {
        /// CDP method whose response was malformed.
        method: String,
        /// Field we expected.
        field: &'static str,
    },

    /// Page-level JavaScript threw.
    #[error("page script threw: {0}")]
    JavaScript(String),

    /// I/O around the browser process or its profile directory.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, CdpError>;
