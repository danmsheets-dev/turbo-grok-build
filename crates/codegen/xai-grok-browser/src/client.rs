//! JSON-RPC client for the Agent WebView host.
//!
//! Talks to a [`BrowserTransport`]: the in-process [`crate::mock::MockBrowserHost`]
//! or a named-pipe client for `\\.\pipe\turbo-browser-{id}`.
//!
//! Policy (`check_url` / `check_fill` / `check_eval_result`) is applied in this
//! client **before** a request is sent. The mock host applies the same checks
//! again and fails closed before mutating state.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::profile::pipe_name;
use crate::protocol::{
    ClickResult, DownloadsResult, EvalPolicyError, FillPolicyError, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, JsonRpcVersion, METHOD_CLICK, METHOD_CLOSE_TAB, METHOD_DOWNLOADS, METHOD_EVAL,
    METHOD_FILL, METHOD_HOVER, METHOD_NAVIGATE, METHOD_NEW_TAB, METHOD_PRESS_KEY, METHOD_RAISE,
    METHOD_SCREENSHOT, METHOD_SCROLL, METHOD_SELECT, METHOD_SELECT_TAB, METHOD_SET_FILE,
    METHOD_SHUTDOWN, METHOD_SNAPSHOT, METHOD_TABS, METHOD_WAIT, NavigateResult, ProtocolError,
    ScreenshotResult, SnapshotResult, TabsResult, UrlPolicyError, WaitResult, check_eval_confirm,
    check_eval_result, check_fill, check_url_in_session, single_tab_v1_error,
};

/// Client / transport failure (policy, RPC, or I/O).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrowserClientError {
    /// Navigation URL rejected (fail closed, before send).
    #[error(transparent)]
    Url(#[from] UrlPolicyError),
    /// Fill value rejected (fail closed, before send).
    #[error(transparent)]
    Fill(#[from] FillPolicyError),
    /// Eval result rejected.
    #[error(transparent)]
    Eval(#[from] EvalPolicyError),
    /// Method / params decode failure.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// Snapshot uid is not in the current AX tree.
    #[error("unknown snapshot uid: {0}")]
    UnknownUid(String),
    /// Host returned a JSON-RPC error object.
    #[error("browser host error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code.
        code: i64,
        /// Host message.
        message: String,
    },
    /// Transport / I/O failure (missing pipe, disconnect, …).
    #[error("browser transport: {0}")]
    Transport(String),
    /// Result JSON did not match the expected type.
    #[error("invalid browser result: {0}")]
    InvalidResult(String),
}

impl BrowserClientError {
    pub(crate) fn from_json(err: serde_json::Error) -> Self {
        Self::InvalidResult(err.to_string())
    }
}

/// JSON-RPC transport: `call(method, params) -> result`.
///
/// The pipe impl wraps a standard JSON-RPC 2.0 envelope. The mock impl
/// dispatches in-process (no wire bytes).
pub trait BrowserTransport: Send + Sync {
    /// Invoke `method` with JSON `params` and return the result value.
    fn call(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, BrowserClientError>> + Send;
}

/// Named-pipe transport for `\\.\pipe\turbo-browser-{id}`.
///
/// Framing is newline-delimited [`JsonRpcRequest`] / [`JsonRpcResponse`].
/// Opening the pipe requires a running `turbo browser-host` (later task).
#[derive(Debug, Clone)]
pub struct NamedPipeTransport {
    pipe_name: String,
    next_id: Arc<AtomicI64>,
}

impl NamedPipeTransport {
    /// Bind to an explicit pipe path.
    pub fn new(pipe_name: impl Into<String>) -> Self {
        Self {
            pipe_name: pipe_name.into(),
            next_id: Arc::new(AtomicI64::new(1)),
        }
    }

    /// Bind to `\\.\pipe\turbo-browser-{session_id}`.
    pub fn for_session(session_id: &str) -> Self {
        Self::new(pipe_name(session_id))
    }

    /// Named-pipe path this transport will open.
    pub fn pipe_name(&self) -> &str {
        &self.pipe_name
    }

    fn next_id(&self) -> JsonRpcId {
        JsonRpcId::Number(self.next_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl BrowserTransport for NamedPipeTransport {
    async fn call(&self, method: &str, params: Value) -> Result<Value, BrowserClientError> {
        let id = self.next_id();
        let line = encode_rpc_request(id, method, params)?;
        #[cfg(windows)]
        {
            call_named_pipe(&self.pipe_name, &line).await
        }
        #[cfg(not(windows))]
        {
            let _ = line;
            Err(BrowserClientError::Transport(
                "named-pipe browser host is Windows-only".into(),
            ))
        }
    }
}

/// Encode a newline-delimited JSON-RPC 2.0 request (trailing `\n`).
pub fn encode_rpc_request(
    id: JsonRpcId,
    method: &str,
    params: Value,
) -> Result<String, BrowserClientError> {
    let req = JsonRpcRequest {
        jsonrpc: JsonRpcVersion,
        id,
        method: method.to_owned(),
        params,
    };
    let mut line = serde_json::to_string(&req).map_err(BrowserClientError::from_json)?;
    line.push('\n');
    Ok(line)
}

/// Decode one JSON-RPC 2.0 response line into a result value.
pub fn decode_rpc_response(line: &str) -> Result<Value, BrowserClientError> {
    let resp: JsonRpcResponse =
        serde_json::from_str(line.trim_end()).map_err(BrowserClientError::from_json)?;
    if let Some(err) = resp.error {
        return Err(BrowserClientError::Rpc {
            code: err.code,
            message: err.message,
        });
    }
    resp.result
        .ok_or_else(|| BrowserClientError::InvalidResult("response missing result".into()))
}

/// Ceiling on one host round trip.
///
/// Slightly above the host's own navigation ceiling so a host-side timeout
/// surfaces as its real error rather than a bare client disconnect.
pub const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);

/// How long to keep retrying `ERROR_PIPE_BUSY` before giving up.
const BUSY_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Open the pipe, retrying while every instance is busy.
///
/// The host serves one connection per instance, so a burst of calls can
/// momentarily find them all taken. `ERROR_PIPE_BUSY` means "try again", not
/// "no host" — treating it as fatal produced spurious transport errors.
#[cfg(windows)]
async fn open_pipe_client(
    pipe_name: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, BrowserClientError> {
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys_pipe::ERROR_PIPE_BUSY;

    let deadline = std::time::Instant::now() + BUSY_RETRY_BUDGET;
    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => return Ok(client),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                if std::time::Instant::now() >= deadline {
                    return Err(BrowserClientError::Transport(format!(
                        "{pipe_name}: all pipe instances busy"
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => {
                return Err(BrowserClientError::Transport(format!("{pipe_name}: {e}")));
            }
        }
    }
}

/// `ERROR_PIPE_BUSY` (231). Avoids pulling in a `windows-sys` dependency for
/// one constant.
#[cfg(windows)]
mod windows_sys_pipe {
    pub const ERROR_PIPE_BUSY: i32 = 231;
}

#[cfg(windows)]
async fn call_named_pipe(pipe_name: &str, request_line: &str) -> Result<Value, BrowserClientError> {
    match tokio::time::timeout(CALL_TIMEOUT, call_named_pipe_inner(pipe_name, request_line)).await {
        Ok(result) => result,
        Err(_) => Err(BrowserClientError::Transport(format!(
            "{pipe_name}: host did not respond within {}s",
            CALL_TIMEOUT.as_secs()
        ))),
    }
}

#[cfg(windows)]
async fn call_named_pipe_inner(
    pipe_name: &str,
    request_line: &str,
) -> Result<Value, BrowserClientError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut client = open_pipe_client(pipe_name).await?;
    client
        .write_all(request_line.as_bytes())
        .await
        .map_err(|e| BrowserClientError::Transport(e.to_string()))?;
    let mut reader = BufReader::new(client);
    let mut response = String::new();
    let n = reader
        .read_line(&mut response)
        .await
        .map_err(|e| BrowserClientError::Transport(e.to_string()))?;
    if n == 0 {
        return Err(BrowserClientError::Transport(format!(
            "{pipe_name}: host closed the pipe"
        )));
    }
    decode_rpc_response(&response)
}

/// Client handle for a `turbo browser-host` sidecar (or in-process mock).
#[derive(Debug, Clone)]
pub struct BrowserClient<T = NamedPipeTransport> {
    session_id: String,
    session_folder: Option<PathBuf>,
    transport: T,
}

impl BrowserClient<NamedPipeTransport> {
    /// Bind a client to a pager/session id (same segment used in the pipe name).
    pub fn new(session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        let transport = NamedPipeTransport::for_session(&session_id);
        Self {
            session_id,
            session_folder: None,
            transport,
        }
    }
}

impl<T> BrowserClient<T> {
    /// Wrap an existing transport (mock host in tests, pipe in production).
    pub fn with_transport(session_id: impl Into<String>, transport: T) -> Self {
        Self {
            session_id: session_id.into(),
            session_folder: None,
            transport,
        }
    }

    /// Allow `file:` only under this session folder (see [`check_url_in_session`]).
    pub fn with_session_folder(mut self, folder: impl Into<PathBuf>) -> Self {
        self.session_folder = Some(folder.into());
        self
    }

    /// Pager/session id this client will talk to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Named-pipe path for this session (`\\.\pipe\turbo-browser-<id>`).
    pub fn pipe_name(&self) -> String {
        pipe_name(&self.session_id)
    }

    /// Session folder used for `file:` exceptions, if any.
    pub fn session_folder(&self) -> Option<&Path> {
        self.session_folder.as_deref()
    }

    /// Borrow the underlying transport (inspect mock state in tests).
    pub fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T: BrowserTransport> BrowserClient<T> {
    /// `browser.navigate`. URL policy is enforced before send.
    pub async fn navigate(
        &self,
        url: impl Into<String>,
    ) -> Result<NavigateResult, BrowserClientError> {
        let url = url.into();
        check_url_in_session(&url, self.session_folder.as_deref())?;
        self.roundtrip(METHOD_NAVIGATE, serde_json::json!({ "url": url }))
            .await
    }

    /// `browser.tabs`.
    pub async fn tabs(&self) -> Result<TabsResult, BrowserClientError> {
        self.roundtrip(METHOD_TABS, serde_json::json!({})).await
    }

    /// `browser.downloads`. List files in the session-scoped broker directory.
    pub async fn downloads(&self) -> Result<DownloadsResult, BrowserClientError> {
        self.roundtrip(METHOD_DOWNLOADS, serde_json::json!({}))
            .await
    }

    /// `browser.new_tab`. Arbitrary extra tabs are still unimplemented.
    pub async fn new_tab(&self, url: Option<String>) -> Result<TabsResult, BrowserClientError> {
        let _ = url;
        Err(BrowserClientError::Rpc {
            code: -32601,
            message: single_tab_v1_error(METHOD_NEW_TAB),
        })
    }

    /// `browser.select_tab`. Main tab is `1`; host-owned OAuth popups are `2+`.
    pub async fn select_tab(&self, tab_id: u32) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_SELECT_TAB, serde_json::json!({ "tab_id": tab_id }))
            .await
    }

    /// `browser.close_tab`. Closes a host-owned OAuth popup; the main tab cannot close.
    pub async fn close_tab(&self, tab_id: u32) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_CLOSE_TAB, serde_json::json!({ "tab_id": tab_id }))
            .await
    }

    /// `browser.snapshot`.
    pub async fn snapshot(&self, verbose: bool) -> Result<SnapshotResult, BrowserClientError> {
        self.snapshot_ex(verbose, false).await
    }

    /// `browser.snapshot` with optional main-landmark text.
    pub async fn snapshot_ex(
        &self,
        verbose: bool,
        include_text: bool,
    ) -> Result<SnapshotResult, BrowserClientError> {
        self.snapshot_on(None, verbose, include_text).await
    }

    /// `browser.snapshot` of a specific tab (`1` = main, `2+` = OAuth popup).
    pub async fn snapshot_on(
        &self,
        tab_id: Option<u32>,
        verbose: bool,
        include_text: bool,
    ) -> Result<SnapshotResult, BrowserClientError> {
        let mut params = serde_json::json!({ "verbose": verbose, "include_text": include_text });
        if let Some(tab_id) = tab_id {
            params["tab_id"] = tab_id.into();
        }
        self.roundtrip(METHOD_SNAPSHOT, params).await
    }

    /// `browser.click`.
    pub async fn click(&self, uid: impl Into<String>) -> Result<ClickResult, BrowserClientError> {
        self.roundtrip(METHOD_CLICK, serde_json::json!({ "uid": uid.into() }))
            .await
    }

    /// `browser.fill`. Fill policy is enforced before send.
    pub async fn fill(
        &self,
        uid: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), BrowserClientError> {
        let uid = uid.into();
        let value = value.into();
        check_fill(&value, None)?;
        self.send_ok(
            METHOD_FILL,
            serde_json::json!({ "uid": uid, "value": value }),
        )
        .await
    }

    /// `browser.eval`. Result size is capped after the host returns.
    pub async fn eval(&self, function: impl Into<String>) -> Result<Value, BrowserClientError> {
        self.eval_ex(function, false).await
    }

    /// `browser.eval` with an explicit confirm flag for mutating script.
    pub async fn eval_ex(
        &self,
        function: impl Into<String>,
        confirm: bool,
    ) -> Result<Value, BrowserClientError> {
        let function = function.into();
        check_eval_confirm(&function, confirm)?;
        let value = self
            .transport
            .call(
                METHOD_EVAL,
                serde_json::json!({ "function": function, "confirm": confirm }),
            )
            .await?;
        let serialized = serde_json::to_string(&value).map_err(BrowserClientError::from_json)?;
        check_eval_result(&serialized)?;
        Ok(value)
    }

    /// `browser.wait`.
    pub async fn wait(
        &self,
        text: Option<String>,
        url_substring: Option<String>,
        timeout_ms: Option<u64>,
    ) -> Result<WaitResult, BrowserClientError> {
        self.roundtrip(
            METHOD_WAIT,
            serde_json::json!({
                "text": text,
                "url_substring": url_substring,
                "timeout_ms": timeout_ms,
            }),
        )
        .await
    }

    /// `browser.scroll`.
    pub async fn scroll(
        &self,
        uid: Option<String>,
        dx: Option<i32>,
        dy: Option<i32>,
    ) -> Result<(), BrowserClientError> {
        self.send_ok(
            METHOD_SCROLL,
            serde_json::json!({ "uid": uid, "dx": dx, "dy": dy }),
        )
        .await
    }

    /// `browser.press_key`.
    pub async fn press_key(
        &self,
        key: impl Into<String>,
        uid: Option<String>,
    ) -> Result<(), BrowserClientError> {
        self.send_ok(
            METHOD_PRESS_KEY,
            serde_json::json!({ "key": key.into(), "uid": uid }),
        )
        .await
    }

    /// `browser.select`.
    pub async fn select(
        &self,
        uid: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), BrowserClientError> {
        self.send_ok(
            METHOD_SELECT,
            serde_json::json!({ "uid": uid.into(), "value": value.into() }),
        )
        .await
    }

    /// `browser.hover`.
    pub async fn hover(&self, uid: impl Into<String>) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_HOVER, serde_json::json!({ "uid": uid.into() }))
            .await
    }

    /// `browser.set_file`.
    pub async fn set_file(
        &self,
        uid: impl Into<String>,
        path: impl Into<String>,
    ) -> Result<(), BrowserClientError> {
        self.send_ok(
            METHOD_SET_FILE,
            serde_json::json!({ "uid": uid.into(), "path": path.into() }),
        )
        .await
    }

    /// `browser.screenshot`.
    pub async fn screenshot(&self) -> Result<ScreenshotResult, BrowserClientError> {
        self.roundtrip(METHOD_SCREENSHOT, serde_json::json!({}))
            .await
    }

    /// `browser.raise`.
    pub async fn raise(&self) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_RAISE, serde_json::json!({})).await
    }

    /// `browser.shutdown`.
    pub async fn shutdown(&self) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_SHUTDOWN, serde_json::json!({})).await
    }

    async fn roundtrip<R: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
    ) -> Result<R, BrowserClientError> {
        let value = self.transport.call(method, params).await?;
        serde_json::from_value(value).map_err(BrowserClientError::from_json)
    }

    async fn send_ok(&self, method: &str, params: Value) -> Result<(), BrowserClientError> {
        let _ = self.transport.call(method, params).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockAction, MockBrowserHost};

    #[test]
    fn client_pipe_name_matches_profile() {
        let client = BrowserClient::new("abc");
        assert_eq!(client.session_id(), "abc");
        assert_eq!(client.pipe_name(), crate::profile::pipe_name("abc"));
        assert_eq!(
            client.transport().pipe_name(),
            crate::profile::pipe_name("abc")
        );
    }

    #[test]
    fn encode_decode_jsonrpc_roundtrip() {
        let line = encode_rpc_request(
            JsonRpcId::Number(1),
            METHOD_NAVIGATE,
            serde_json::json!({ "url": "https://example.com/" }),
        )
        .unwrap();
        assert!(line.ends_with('\n'));
        assert!(!line.ends_with("\r\n"));
        let req: JsonRpcRequest = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(req.method, METHOD_NAVIGATE);

        let ok = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "url": "https://example.com/", "title": "example.com" }
        });
        let got = decode_rpc_response(&ok.to_string()).unwrap();
        assert_eq!(got["url"], "https://example.com/");

        let err_line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32601, "message": "unknown" }
        })
        .to_string();
        match decode_rpc_response(&err_line).unwrap_err() {
            BrowserClientError::Rpc { code, message } => {
                assert_eq!(code, -32601);
                assert_eq!(message, "unknown");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn navigate_snapshot_click_uid_1() {
        let client = BrowserClient::with_transport("sess", MockBrowserHost::new());
        let nav = client.navigate("https://example.com/").await.unwrap();
        assert_eq!(nav.url, "https://example.com/");

        let snap = client.snapshot(false).await.unwrap();
        assert_eq!(snap.url, "https://example.com/");
        assert_eq!(snap.nodes.len(), 2);
        assert_eq!(snap.nodes[0].uid, "1-1");
        assert_eq!(snap.nodes[0].role, "link");
        assert_eq!(snap.nodes[1].uid, "1-2");
        assert_eq!(snap.nodes[1].role, "textbox");

        client.click("1-1").await.unwrap();
        assert_eq!(
            client.transport().last_action(),
            Some(MockAction::Click { uid: "1-1".into() })
        );
        assert_eq!(
            client.transport().call_log(),
            vec![
                METHOD_NAVIGATE.to_owned(),
                METHOD_SNAPSHOT.to_owned(),
                METHOD_CLICK.to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn fill_rejects_otp_in_client_before_send() {
        let client = BrowserClient::with_transport("sess", MockBrowserHost::new());
        let err = client.fill("1-2", "123456").await.unwrap_err();
        assert!(
            matches!(err, BrowserClientError::Fill(FillPolicyError::OtpShaped)),
            "{err:?}"
        );
        assert!(
            client.transport().call_log().is_empty(),
            "OTP fill must not be sent to the host"
        );
        assert_eq!(client.transport().last_action(), None);
    }

    #[tokio::test]
    async fn file_url_denied_in_client_before_send() {
        let client = BrowserClient::with_transport("sess", MockBrowserHost::new());
        let err = client
            .navigate("file:///C:/Windows/notepad.exe")
            .await
            .unwrap_err();
        assert!(
            matches!(err, BrowserClientError::Url(UrlPolicyError::FileDenied)),
            "{err:?}"
        );
        assert!(
            client.transport().call_log().is_empty(),
            "denied file: URL must not be sent to the host"
        );
        assert_eq!(client.transport().url(), "about:blank");
    }

    #[tokio::test]
    async fn mutating_eval_is_not_sent_without_confirm() {
        let client = BrowserClient::with_transport("sess", MockBrowserHost::new());
        let err = client
            .eval_ex("() => document.querySelector('button').click()", false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, BrowserClientError::Eval(EvalPolicyError::NeedsConfirm)),
            "{err:?}"
        );
        assert!(
            client.transport().call_log().is_empty(),
            "mutating eval must not reach the host without confirm"
        );
        let err = client
            .eval_ex("() => document.querySelector('button').click()", true)
            .await
            .unwrap_err();
        assert!(
            matches!(err, BrowserClientError::Eval(EvalPolicyError::NeedsConfirm)),
            "confirm must not unlock mutating eval: {err:?}"
        );
        assert!(
            client.transport().call_log().is_empty(),
            "mutating eval must not reach the host even with confirm"
        );
        client
            .eval_ex("() => document.title", false)
            .await
            .expect("read eval");
        assert_eq!(client.transport().call_log(), vec![METHOD_EVAL]);
    }

    #[tokio::test]
    async fn new_tab_fails_closed_without_send() {
        let client = BrowserClient::with_transport("sess", MockBrowserHost::new());
        let err = client
            .new_tab(Some("https://example.com/".into()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("v1 is a single tab"), "{err}");
        assert!(
            client.transport().call_log().is_empty(),
            "new_tab RPC must not be sent"
        );
    }

    #[tokio::test]
    async fn oauth_popup_tab_can_be_snapshotted_and_closed() {
        let host = MockBrowserHost::new();
        let tab_id = host
            .open_oauth_popup("https://accounts.google.com/gsi")
            .unwrap();
        let client = BrowserClient::with_transport("sess", host);
        let tabs = client.tabs().await.unwrap();
        assert_eq!(tabs.tabs.len(), 2);
        assert_eq!(tabs.tabs[1].tab_id, tab_id);
        let snap = client
            .snapshot_on(Some(tab_id), false, false)
            .await
            .unwrap();
        assert!(snap.url.contains("accounts.google.com"), "{}", snap.url);
        client.select_tab(tab_id).await.unwrap();
        client.close_tab(tab_id).await.unwrap();
        let err = client.close_tab(1).await.unwrap_err();
        assert!(err.to_string().contains("cannot close the main"), "{err}");
        assert_eq!(client.tabs().await.unwrap().tabs.len(), 1);
    }

    #[tokio::test]
    async fn downloads_roundtrip_returns_session_broker_state() {
        let client = BrowserClient::with_transport("sess", MockBrowserHost::new());
        let result = client.downloads().await.unwrap();
        assert!(result.downloads.is_empty());
        assert_eq!(client.transport().call_log(), vec![METHOD_DOWNLOADS]);
    }

    #[tokio::test]
    async fn named_pipe_missing_host_errors() {
        let t = NamedPipeTransport::new(r"\\.\pipe\turbo-browser-does-not-exist-task2");
        let err = t
            .call(METHOD_TABS, serde_json::json!({}))
            .await
            .unwrap_err();
        match err {
            BrowserClientError::Transport(msg) => {
                assert!(!msg.is_empty(), "{msg}");
            }
            other => panic!("expected transport error, got {other:?}"),
        }
    }
}
