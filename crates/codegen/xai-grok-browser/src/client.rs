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
    EvalPolicyError, FillPolicyError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, JsonRpcVersion,
    METHOD_CLICK, METHOD_CLOSE_TAB, METHOD_EVAL, METHOD_FILL, METHOD_NAVIGATE, METHOD_NEW_TAB,
    METHOD_RAISE, METHOD_SCREENSHOT, METHOD_SELECT_TAB, METHOD_SHUTDOWN, METHOD_SNAPSHOT,
    METHOD_TABS, NavigateResult, ProtocolError, ScreenshotResult, SnapshotResult, TabsResult,
    UrlPolicyError, check_eval_result, check_fill, check_url_in_session,
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

#[cfg(windows)]
async fn call_named_pipe(pipe_name: &str, request_line: &str) -> Result<Value, BrowserClientError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;

    let mut client = ClientOptions::new()
        .open(pipe_name)
        .map_err(|e| BrowserClientError::Transport(format!("{pipe_name}: {e}")))?;
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

    /// `browser.new_tab`. Optional URL is checked before send.
    pub async fn new_tab(&self, url: Option<String>) -> Result<TabsResult, BrowserClientError> {
        if let Some(url) = url.as_deref() {
            check_url_in_session(url, self.session_folder.as_deref())?;
        }
        self.roundtrip(METHOD_NEW_TAB, serde_json::json!({ "url": url }))
            .await
    }

    /// `browser.select_tab`.
    pub async fn select_tab(&self, tab_id: u32) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_SELECT_TAB, serde_json::json!({ "tab_id": tab_id }))
            .await
    }

    /// `browser.close_tab`.
    pub async fn close_tab(&self, tab_id: u32) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_CLOSE_TAB, serde_json::json!({ "tab_id": tab_id }))
            .await
    }

    /// `browser.snapshot`.
    pub async fn snapshot(&self, verbose: bool) -> Result<SnapshotResult, BrowserClientError> {
        self.roundtrip(METHOD_SNAPSHOT, serde_json::json!({ "verbose": verbose }))
            .await
    }

    /// `browser.click`.
    pub async fn click(&self, uid: impl Into<String>) -> Result<(), BrowserClientError> {
        self.send_ok(METHOD_CLICK, serde_json::json!({ "uid": uid.into() }))
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
        let value = self
            .transport
            .call(
                METHOD_EVAL,
                serde_json::json!({ "function": function.into() }),
            )
            .await?;
        let serialized = serde_json::to_string(&value).map_err(BrowserClientError::from_json)?;
        check_eval_result(&serialized)?;
        Ok(value)
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
        assert_eq!(snap.nodes[0].uid, "1");
        assert_eq!(snap.nodes[0].role, "link");
        assert_eq!(snap.nodes[1].uid, "2");
        assert_eq!(snap.nodes[1].role, "textbox");

        client.click("1").await.unwrap();
        assert_eq!(
            client.transport().last_action(),
            Some(MockAction::Click { uid: "1".into() })
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
        let err = client.fill("2", "123456").await.unwrap_err();
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
