//! Agent WebView protocol, profile paths, and JSON-RPC client.
//!
//! Task 1 is protocol + policy only. The host (`turbo browser-host`) and
//! named-pipe transport land in later tasks. This crate must stay free of
//! WebView2 / Win32 dependencies.

pub mod client;
pub mod profile;
pub mod protocol;

pub use client::BrowserClient;
pub use profile::{agent_browser_user_data_dir, pipe_name};
pub use protocol::{
    AxNode, BrowserEvent, BrowserMethod, BrowserRequest, EVAL_RESULT_MAX_BYTES, EvalPolicyError,
    FillPolicyError, JSONRPC_VERSION, JsonRpcError, JsonRpcEvent, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, JsonRpcVersion, METHOD_CLICK, METHOD_CLOSE_TAB, METHOD_EVAL, METHOD_EVENT,
    METHOD_FILL, METHOD_NAVIGATE, METHOD_NEW_TAB, METHOD_RAISE, METHOD_SCREENSHOT,
    METHOD_SELECT_TAB, METHOD_SHUTDOWN, METHOD_SNAPSHOT, METHOD_TABS, NavigateResult,
    ProtocolError, ScreenshotResult, SnapshotResult, TabInfo, TabsResult, UrlPolicyError,
    check_eval_result, check_eval_result_len, check_fill, check_fill_value, check_url,
    check_url_in_session,
};
