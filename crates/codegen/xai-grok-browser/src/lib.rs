//! Agent WebView protocol, profile paths, JSON-RPC client, mock, and host.
//!
//! `turbo browser-host` is the product-owned sidecar. WebView2 / Win32
//! bindings are Windows-only (`host::{window,webview,rpc}`).

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
pub use profile::{
    GROK_BROWSER_PROFILE_ENV, agent_browser_profile_dir, agent_browser_user_data_dir, pipe_name,
};
pub use protocol::{
    AxNode, BrowserEvent, BrowserMethod, BrowserRequest, ClickResult, DownloadInfo,
    DownloadsResult, EVAL_RESULT_MAX_BYTES, EvalPolicyError, FillPolicyError, FillTarget,
    GROK_BROWSER_ALLOW_ENV, JSONRPC_VERSION, JsonRpcError, JsonRpcEvent, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, JsonRpcVersion, METHOD_CLICK, METHOD_CLOSE_TAB, METHOD_DOWNLOADS, METHOD_EVAL,
    METHOD_EVENT, METHOD_FILL, METHOD_HOVER, METHOD_NAVIGATE, METHOD_NEW_TAB, METHOD_PRESS_KEY,
    METHOD_RAISE, METHOD_SCREENSHOT, METHOD_SCROLL, METHOD_SELECT, METHOD_SELECT_TAB,
    METHOD_SET_FILE, METHOD_SHUTDOWN, METHOD_SNAPSHOT, METHOD_TABS, METHOD_WAIT, NavigateResult,
    ProtocolError, ScreenshotResult, SnapshotResult, SnapshotSource, TabInfo, TabsResult,
    UrlPolicyError, WaitResult, check_eval_result, check_eval_result_len, check_fill,
    check_fill_target, check_fill_value, check_url, check_url_in_session, eval_looks_mutating,
    is_oauth_popup_url,
};
