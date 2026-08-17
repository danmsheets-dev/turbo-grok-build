//! Agent WebView protocol, profile paths, JSON-RPC client, mock, and host stub.
//!
//! `turbo browser-host` is the product-owned sidecar. This crate must stay
//! free of WebView2 / Win32 dependencies (those land in a later task).

pub mod client;
pub mod host;
pub mod mock;
pub mod profile;
pub mod protocol;

pub use client::{
    BrowserClient, BrowserClientError, BrowserTransport, NamedPipeTransport, decode_rpc_response,
    encode_rpc_request,
};
pub use mock::{MockAction, MockBrowserHost};
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
