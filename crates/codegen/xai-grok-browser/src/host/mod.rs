//! Sidecar host entry (`turbo browser-host`).
//!
//! Windows builds own a WS_OVERLAPPEDWINDOW + WebView2 controller and serve
//! newline-delimited JSON-RPC on the session named pipe. Non-Windows builds
//! return [`HostError::WindowsOnly`] (CLI maps to exit 2).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;

use crate::profile::{agent_browser_user_data_dir, pipe_name};
use crate::protocol::{
    BrowserRequest, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, JsonRpcVersion,
    check_fill, check_url,
};

mod ax;
#[cfg(windows)]
mod rpc;
#[cfg(windows)]
mod webview;
#[cfg(windows)]
mod window;

pub use ax::{
    SNAPSHOT_NODE_CAP, SNAPSHOT_NODE_CAP_VERBOSE, compact_ax_tree, resolve_uid, snapshot_cap,
};

/// JSON-RPC parse error.
pub(crate) const RPC_PARSE_ERROR: i64 = -32700;
/// JSON-RPC method not found / not implemented.
pub(crate) const RPC_METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC invalid params.
pub(crate) const RPC_INVALID_PARAMS: i64 = -32602;
/// JSON-RPC internal error.
pub(crate) const RPC_INTERNAL_ERROR: i64 = -32603;
/// Application error (URL policy, navigation, screenshot I/O).
pub(crate) const RPC_HOST_ERROR: i64 = -32000;

/// Arguments for [`run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostArgs {
    /// Pager/session id (same segment used in the default pipe name).
    pub session_id: String,
    /// Named pipe. Empty means [`pipe_name`].
    pub pipe: String,
    /// WebView2 user-data-dir. Empty means [`agent_browser_user_data_dir`].
    pub user_data_dir: PathBuf,
}

/// Host startup / runtime failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// Sidecar is not implemented outside Windows.
    #[error("turbo browser-host is Windows-only in v1")]
    WindowsOnly,
    /// Evergreen WebView2 Runtime is not installed.
    #[error(
        "WebView2 runtime is not installed. Install the Evergreen WebView2 Runtime from https://developer.microsoft.com/microsoft-edge/webview2/"
    )]
    RuntimeMissing,
    /// Win32 / COM / pipe / I/O failure while starting or running the host.
    #[error("browser host failed: {0}")]
    Failed(String),
}

impl HostError {
    /// Process exit code for this error (`2` for Windows-only, else `1`).
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::WindowsOnly => 2,
            Self::RuntimeMissing | Self::Failed(_) => 1,
        }
    }
}

/// Probe Evergreen WebView2 without creating a window.
///
/// Windows calls the runtime check. Other OS return [`HostError::WindowsOnly`].
pub fn probe_webview2_runtime() -> Result<(), HostError> {
    #[cfg(windows)]
    {
        webview::ensure_runtime_installed()
    }
    #[cfg(not(windows))]
    {
        Err(HostError::WindowsOnly)
    }
}

#[cfg(windows)]
pub use webview::ensure_runtime_installed;

impl HostArgs {
    /// Fill empty `pipe` / `user_data_dir` with product defaults.
    pub fn resolve_defaults(mut self) -> Self {
        if self.pipe.is_empty() {
            self.pipe = pipe_name(&self.session_id);
        }
        if self.user_data_dir.as_os_str().is_empty() {
            self.user_data_dir = agent_browser_user_data_dir();
        }
        self
    }
}

/// Decoded host operation (v1 single-tab + page control).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostCall {
    /// `browser.navigate`
    Navigate {
        /// Policy-checked URL.
        url: String,
    },
    /// `browser.screenshot`
    Screenshot,
    /// `browser.tabs` (single-tab v1).
    Tabs,
    /// `browser.raise`
    Raise,
    /// `browser.shutdown`
    Shutdown,
    /// `browser.snapshot`
    Snapshot {
        /// Raise node cap from 200 to 800.
        verbose: bool,
    },
    /// `browser.click`
    Click {
        /// `data-turbo-uid` (decimal).
        uid: String,
    },
    /// `browser.fill`
    Fill {
        /// `data-turbo-uid` (decimal).
        uid: String,
        /// Policy-checked value.
        value: String,
    },
    /// `browser.eval`
    Eval {
        /// Function expression; host wraps with `JSON.stringify`.
        function: String,
    },
}

/// Run the Agent WebView sidecar.
///
/// On Windows this blocks on the Win32 message loop until the window is
/// destroyed or `browser.shutdown` is received. Do not call this from a
/// default unit test.
pub fn run(args: HostArgs) -> Result<(), HostError> {
    let args = args.resolve_defaults();
    #[cfg(windows)]
    {
        run_windows(args)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        eprintln!("turbo browser-host is Windows-only in v1");
        Err(HostError::WindowsOnly)
    }
}

#[cfg(windows)]
fn run_windows(args: HostArgs) -> Result<(), HostError> {
    use std::sync::mpsc;

    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::HiDpi::{PROCESS_PER_MONITOR_DPI_AWARE, SetProcessDpiAwareness};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PostQuitMessage, TranslateMessage, WM_APP, WM_QUIT,
    };

    eprintln!(
        "turbo browser-host: session={} pipe={} user-data-dir={}",
        args.session_id,
        args.pipe,
        args.user_data_dir.display()
    );

    // SAFETY: COM STA is required on the thread that owns the WebView2
    // controller. This is the process main thread (sidecar argv).
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| HostError::Failed(format!("CoInitializeEx: {e}")))?;
    }
    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            // SAFETY: paired with CoInitializeEx on this thread.
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _com = ComGuard;

    // Best-effort; already-aware processes return an error we ignore.
    let _ = unsafe { SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE) };

    std::fs::create_dir_all(&args.user_data_dir).map_err(|e| {
        HostError::Failed(format!(
            "create user-data-dir {}: {e}",
            args.user_data_dir.display()
        ))
    })?;

    webview::ensure_runtime_installed()?;

    let hwnd = window::create_frame_window()?;
    let mut agent = webview::AgentWebView::create(hwnd, &args.user_data_dir, &args.session_id)?;
    window::show(hwnd);

    let ui_thread_id = unsafe { GetCurrentThreadId() };
    let (cmd_tx, cmd_rx) = mpsc::channel::<rpc::UiJob>();
    let pipe_thread = rpc::spawn_pipe_thread(args.pipe.clone(), ui_thread_id, cmd_tx)?;

    let mut msg = MSG::default();
    loop {
        while let Ok(job) = cmd_rx.try_recv() {
            let line = handle_ui_job(&mut agent, hwnd, job.call);
            let _ = job.reply.send(line);
            // Nested wait_with_pump may have consumed WM_QUIT; if the frame
            // is gone, re-post so this loop still exits.
            if !window::is_alive(hwnd) {
                unsafe {
                    PostQuitMessage(0);
                }
            }
        }

        // SAFETY: standard UI-thread GetMessageW pump. WM_APP is only a
        // wake-up from the pipe thread (no window; do not dispatch).
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
        match result {
            -1 => {
                pipe_thread.shutdown();
                return Err(HostError::Failed("GetMessageW failed".into()));
            }
            0 => break,
            _ => {
                if msg.message == WM_APP || msg.message == WM_QUIT {
                    continue;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    pipe_thread.shutdown();
    agent.close();
    // If shutdown came from RPC, the window may still exist.
    window::destroy(hwnd);
    Ok(())
}

#[cfg(windows)]
fn handle_ui_job(
    agent: &mut webview::AgentWebView,
    hwnd: windows::Win32::Foundation::HWND,
    call: Result<(JsonRpcId, HostCall), DecodedRpcError>,
) -> String {
    match call {
        Err(err) => encode_rpc_error(err.id.unwrap_or(JsonRpcId::Number(0)), err.error),
        Ok((id, HostCall::Navigate { url })) => match agent.navigate(&url) {
            Ok(result) => encode_rpc_ok(id, serde_json::to_value(result).unwrap_or(Value::Null)),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
        Ok((id, HostCall::Screenshot)) => match agent.screenshot() {
            Ok(result) => encode_rpc_ok(id, serde_json::to_value(result).unwrap_or(Value::Null)),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
        Ok((id, HostCall::Tabs)) => match agent.current_tab() {
            Ok(result) => encode_rpc_ok(id, serde_json::to_value(result).unwrap_or(Value::Null)),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
        Ok((id, HostCall::Raise)) => {
            window::raise(hwnd);
            encode_rpc_ok(id, Value::Object(serde_json::Map::new()))
        }
        Ok((id, HostCall::Shutdown)) => {
            window::destroy(hwnd);
            encode_rpc_ok(id, Value::Object(serde_json::Map::new()))
        }
        Ok((id, HostCall::Snapshot { verbose })) => match agent.snapshot(verbose) {
            Ok(result) => encode_rpc_ok(id, serde_json::to_value(result).unwrap_or(Value::Null)),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
        Ok((id, HostCall::Click { uid })) => match agent.click(&uid) {
            Ok(()) => encode_rpc_ok(id, Value::Object(serde_json::Map::new())),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
        Ok((id, HostCall::Fill { uid, value })) => match agent.fill(&uid, &value) {
            Ok(()) => encode_rpc_ok(id, Value::Object(serde_json::Map::new())),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
        Ok((id, HostCall::Eval { function })) => match agent.eval_function(&function) {
            Ok(value) => encode_rpc_ok(id, value),
            Err(message) => encode_rpc_error(
                id,
                JsonRpcError {
                    code: RPC_HOST_ERROR,
                    message,
                    data: None,
                },
            ),
        },
    }
}

/// Failed JSON-RPC decode (id may be missing on parse errors).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DecodedRpcError {
    /// Request id when the envelope parsed.
    pub id: Option<JsonRpcId>,
    /// JSON-RPC error object.
    pub error: JsonRpcError,
}

/// Decode one newline-delimited JSON-RPC request into a host call.
///
/// Applies [`check_url`] on navigate and [`check_fill`] on fill (fail
/// closed). Invalid click/fill uids become `unknown_uid`. Multi-tab
/// methods stay JSON-RPC errors (Task 6).
pub(crate) fn decode_host_call(line: &str) -> Result<(JsonRpcId, HostCall), DecodedRpcError> {
    let req: JsonRpcRequest =
        serde_json::from_str(line.trim_end()).map_err(|e| DecodedRpcError {
            id: None,
            error: JsonRpcError {
                code: RPC_PARSE_ERROR,
                message: e.to_string(),
                data: None,
            },
        })?;
    let id = req.id.clone();
    let request = match req.browser_request() {
        Ok(request) => request,
        Err(crate::protocol::ProtocolError::UnknownMethod(method)) => {
            return Err(DecodedRpcError {
                id: Some(id),
                error: JsonRpcError {
                    code: RPC_METHOD_NOT_FOUND,
                    message: format!("unknown method: {method}"),
                    data: None,
                },
            });
        }
        Err(crate::protocol::ProtocolError::InvalidParams(message)) => {
            return Err(DecodedRpcError {
                id: Some(id),
                error: JsonRpcError {
                    code: RPC_INVALID_PARAMS,
                    message,
                    data: None,
                },
            });
        }
    };

    match request {
        BrowserRequest::Navigate { url } => {
            if let Err(err) = check_url(&url) {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_HOST_ERROR,
                        message: err.to_string(),
                        data: None,
                    },
                });
            }
            Ok((id, HostCall::Navigate { url }))
        }
        BrowserRequest::Screenshot {} => Ok((id, HostCall::Screenshot)),
        BrowserRequest::Tabs {} => Ok((id, HostCall::Tabs)),
        BrowserRequest::Raise {} => Ok((id, HostCall::Raise)),
        BrowserRequest::Shutdown {} => Ok((id, HostCall::Shutdown)),
        BrowserRequest::Snapshot { verbose } => Ok((id, HostCall::Snapshot { verbose })),
        BrowserRequest::Click { uid } => {
            if let Err(message) = ax::resolve_uid(&uid) {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_HOST_ERROR,
                        message,
                        data: None,
                    },
                });
            }
            Ok((id, HostCall::Click { uid }))
        }
        BrowserRequest::Fill { uid, value } => {
            if let Err(message) = ax::resolve_uid(&uid) {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_HOST_ERROR,
                        message,
                        data: None,
                    },
                });
            }
            if let Err(err) = check_fill(&value, None) {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_HOST_ERROR,
                        message: err.to_string(),
                        data: None,
                    },
                });
            }
            Ok((id, HostCall::Fill { uid, value }))
        }
        BrowserRequest::Eval { function } => {
            if function.trim().is_empty() {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_INVALID_PARAMS,
                        message: "eval function is empty".into(),
                        data: None,
                    },
                });
            }
            Ok((id, HostCall::Eval { function }))
        }
        BrowserRequest::NewTab { .. }
        | BrowserRequest::SelectTab { .. }
        | BrowserRequest::CloseTab { .. } => Err(DecodedRpcError {
            id: Some(id),
            error: JsonRpcError {
                code: RPC_METHOD_NOT_FOUND,
                message: format!("{} is not implemented (single-tab v1)", req.method),
                data: None,
            },
        }),
    }
}

pub(crate) fn encode_rpc_ok(id: JsonRpcId, result: Value) -> String {
    encode_rpc_response(JsonRpcResponse {
        jsonrpc: JsonRpcVersion,
        id,
        result: Some(result),
        error: None,
    })
}

pub(crate) fn encode_rpc_error(id: JsonRpcId, error: JsonRpcError) -> String {
    encode_rpc_response(JsonRpcResponse {
        jsonrpc: JsonRpcVersion,
        id,
        result: None,
        error: Some(error),
    })
}

fn encode_rpc_response(resp: JsonRpcResponse) -> String {
    let mut line = serde_json::to_string(&resp).unwrap_or_else(|_| {
        format!(
            r#"{{"jsonrpc":"2.0","id":0,"error":{{"code":{RPC_INTERNAL_ERROR},"message":"encode failed"}}}}"#
        )
    });
    line.push('\n');
    line
}

/// Directory for `browser-<n>.png` files.
pub(crate) fn screenshot_dir(session_id: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("TURBO_BROWSER_IMAGE_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    std::env::temp_dir()
        .join("turbo-browser")
        .join(session_id)
        .join("images")
}

/// Decode CDP `Page.captureScreenshot` JSON (`{"data":"<base64>"}`) and PNG IHDR.
pub(crate) fn decode_cdp_png(json: &str) -> Result<(Vec<u8>, u32, u32), String> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("CDP screenshot JSON: {e}"))?;
    let data = value
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "CDP screenshot missing data field".to_owned())?;
    let png = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("CDP screenshot base64: {e}"))?;
    let (width, height) = parse_png_ihdr(&png)?;
    Ok((png, width, height))
}

/// Read width/height from a PNG IHDR chunk (no `image` crate).
pub(crate) fn parse_png_ihdr(png: &[u8]) -> Result<(u32, u32), String> {
    const SIG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if png.len() < 24 {
        return Err("PNG too short for IHDR".into());
    }
    if &png[0..8] != SIG {
        return Err("not a PNG (bad signature)".into());
    }
    if &png[12..16] != b"IHDR" {
        return Err("PNG missing IHDR chunk".into());
    }
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err("PNG IHDR has zero dimension".into());
    }
    Ok((width, height))
}

/// Next `browser-<n>.png` path under `dir` (creates `dir`).
pub(crate) fn next_screenshot_path(dir: &Path, n: u32) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create screenshot dir: {e}"))?;
    Ok(dir.join(format!("browser-{n}.png")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::encode_rpc_request;
    use crate::protocol::{
        METHOD_CLICK, METHOD_EVAL, METHOD_FILL, METHOD_NAVIGATE, METHOD_NEW_TAB, METHOD_SCREENSHOT,
        METHOD_SNAPSHOT,
    };

    #[test]
    fn empty_pipe_and_profile_resolve_to_defaults() {
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
        }
        .resolve_defaults();
        assert_eq!(resolved.session_id, "abc");
        assert_eq!(resolved.pipe, pipe_name("abc"));
        assert_eq!(resolved.user_data_dir, agent_browser_user_data_dir());
    }

    #[test]
    fn explicit_pipe_and_profile_are_kept() {
        let pipe = r"\\.\pipe\custom-browser";
        let dir = PathBuf::from("/tmp/custom-agent-browser");
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: pipe.into(),
            user_data_dir: dir.clone(),
        }
        .resolve_defaults();
        assert_eq!(resolved.pipe, pipe);
        assert_eq!(resolved.user_data_dir, dir);
    }

    #[test]
    fn windows_only_maps_to_exit_code_two() {
        assert_eq!(HostError::WindowsOnly.exit_code(), 2);
        assert_eq!(HostError::RuntimeMissing.exit_code(), 1);
        assert_eq!(HostError::Failed("x".into()).exit_code(), 1);
    }

    #[test]
    fn runtime_missing_display_mentions_evergreen() {
        let msg = HostError::RuntimeMissing.to_string();
        assert!(msg.contains("Evergreen WebView2 Runtime"), "{msg}");
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_windows_only() {
        let err = run(HostArgs {
            session_id: "stub-sess".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
        })
        .expect_err("non-Windows host must refuse");
        assert_eq!(err, HostError::WindowsOnly);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn decode_navigate_and_screenshot_requests() {
        let nav_line = encode_rpc_request(
            JsonRpcId::Number(1),
            METHOD_NAVIGATE,
            serde_json::json!({ "url": "https://example.com/" }),
        )
        .unwrap();
        match decode_host_call(&nav_line).unwrap() {
            (JsonRpcId::Number(1), HostCall::Navigate { url }) => {
                assert_eq!(url, "https://example.com/");
            }
            other => panic!("{other:?}"),
        }

        let shot_line = encode_rpc_request(
            JsonRpcId::Number(2),
            METHOD_SCREENSHOT,
            serde_json::json!({}),
        )
        .unwrap();
        match decode_host_call(&shot_line).unwrap() {
            (JsonRpcId::Number(2), HostCall::Screenshot) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decode_rejects_file_url_before_navigate() {
        let line = encode_rpc_request(
            JsonRpcId::Number(3),
            METHOD_NAVIGATE,
            serde_json::json!({ "url": "file:///C:/Windows/notepad.exe" }),
        )
        .unwrap();
        let err = decode_host_call(&line).unwrap_err();
        assert_eq!(err.error.code, RPC_HOST_ERROR);
        assert!(err.error.message.contains("file:"), "{}", err.error.message);
    }

    #[test]
    fn decode_rejects_javascript_and_data_urls() {
        for url in ["javascript:alert(1)", "data:text/html,hi"] {
            let line = encode_rpc_request(
                JsonRpcId::Number(4),
                METHOD_NAVIGATE,
                serde_json::json!({ "url": url }),
            )
            .unwrap();
            let err = decode_host_call(&line).unwrap_err();
            assert_eq!(err.error.code, RPC_HOST_ERROR, "{url}");
        }
    }

    #[test]
    fn decode_snapshot_click_fill_eval() {
        let snap = encode_rpc_request(
            JsonRpcId::Number(5),
            METHOD_SNAPSHOT,
            serde_json::json!({ "verbose": true }),
        )
        .unwrap();
        match decode_host_call(&snap).unwrap() {
            (JsonRpcId::Number(5), HostCall::Snapshot { verbose: true }) => {}
            other => panic!("{other:?}"),
        }

        let click = encode_rpc_request(
            JsonRpcId::Number(6),
            METHOD_CLICK,
            serde_json::json!({ "uid": "1" }),
        )
        .unwrap();
        match decode_host_call(&click).unwrap() {
            (_, HostCall::Click { uid }) => assert_eq!(uid, "1"),
            other => panic!("{other:?}"),
        }

        let fill = encode_rpc_request(
            JsonRpcId::Number(7),
            METHOD_FILL,
            serde_json::json!({ "uid": "2", "value": "hello" }),
        )
        .unwrap();
        match decode_host_call(&fill).unwrap() {
            (_, HostCall::Fill { uid, value }) => {
                assert_eq!(uid, "2");
                assert_eq!(value, "hello");
            }
            other => panic!("{other:?}"),
        }

        let eval = encode_rpc_request(
            JsonRpcId::Number(8),
            METHOD_EVAL,
            serde_json::json!({ "function": "() => 1" }),
        )
        .unwrap();
        match decode_host_call(&eval).unwrap() {
            (_, HostCall::Eval { function }) => assert_eq!(function, "() => 1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decode_fill_rejects_otp_before_host() {
        let line = encode_rpc_request(
            JsonRpcId::Number(9),
            METHOD_FILL,
            serde_json::json!({ "uid": "2", "value": "123456" }),
        )
        .unwrap();
        let err = decode_host_call(&line).unwrap_err();
        assert_eq!(err.error.code, RPC_HOST_ERROR);
        assert!(
            err.error.message.contains("one-time password"),
            "{}",
            err.error.message
        );
    }

    #[test]
    fn decode_click_unknown_uid() {
        let line = encode_rpc_request(
            JsonRpcId::Number(10),
            METHOD_CLICK,
            serde_json::json!({ "uid": "nope" }),
        )
        .unwrap();
        let err = decode_host_call(&line).unwrap_err();
        assert_eq!(err.error.code, RPC_HOST_ERROR);
        assert!(
            err.error.message.contains("unknown_uid"),
            "{}",
            err.error.message
        );
    }

    #[test]
    fn decode_new_tab_still_unimplemented() {
        let line = encode_rpc_request(JsonRpcId::Number(11), METHOD_NEW_TAB, serde_json::json!({}))
            .unwrap();
        let err = decode_host_call(&line).unwrap_err();
        assert_eq!(err.error.code, RPC_METHOD_NOT_FOUND);
        assert!(
            err.error.message.contains("not implemented"),
            "{}",
            err.error.message
        );
        assert!(
            !err.error.message.contains("Task 4"),
            "{}",
            err.error.message
        );
    }

    #[test]
    fn parse_png_ihdr_reads_width_height() {
        // 1×1 transparent PNG.
        let png = {
            const B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
            base64::engine::general_purpose::STANDARD
                .decode(B64)
                .unwrap()
        };
        assert_eq!(parse_png_ihdr(&png).unwrap(), (1, 1));
        let json = serde_json::json!({
            "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
        })
        .to_string();
        let (_, w, h) = decode_cdp_png(&json).unwrap();
        assert_eq!((w, h), (1, 1));
    }

    #[test]
    fn screenshot_dir_honors_env_override() {
        let prev = std::env::var_os("TURBO_BROWSER_IMAGE_DIR");
        // SAFETY: test process; we restore the env var after.
        unsafe {
            std::env::set_var("TURBO_BROWSER_IMAGE_DIR", "/tmp/custom-shots");
        }
        let dir = screenshot_dir("sess");
        match prev {
            Some(v) => unsafe { std::env::set_var("TURBO_BROWSER_IMAGE_DIR", v) },
            None => unsafe { std::env::remove_var("TURBO_BROWSER_IMAGE_DIR") },
        }
        assert_eq!(dir, PathBuf::from("/tmp/custom-shots"));
    }

    #[cfg(windows)]
    #[ignore]
    #[tokio::test(flavor = "current_thread")]
    async fn host_navigates_example_com() {
        if std::env::var("TURBO_WEBVIEW_IT").ok().as_deref() != Some("1") {
            return;
        }

        use std::time::Duration;

        use crate::client::BrowserClient;
        use crate::profile::pipe_name;

        let session_id = format!("it-webview-{}-{}", std::process::id(), {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        });
        let pipe = pipe_name(&session_id);
        let user_data_dir = std::env::temp_dir()
            .join("turbo-browser-it")
            .join(&session_id);
        let image_dir = std::env::temp_dir()
            .join("turbo-browser-it")
            .join(format!("{session_id}-images"));
        // SAFETY: isolated IT process env; restored below.
        let prev_images = std::env::var_os("TURBO_BROWSER_IMAGE_DIR");
        unsafe {
            std::env::set_var("TURBO_BROWSER_IMAGE_DIR", &image_dir);
        }

        let host_args = HostArgs {
            session_id: session_id.clone(),
            pipe: pipe.clone(),
            user_data_dir,
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let host_thread = std::thread::Builder::new()
            .name("browser-host-it".into())
            .spawn(move || {
                let result = run(host_args);
                let _ = done_tx.send(result);
            })
            .expect("spawn host thread");

        let client = BrowserClient::new(session_id.clone());
        let mut last_err = None;
        let mut ready = false;
        for _ in 0..80 {
            match client.tabs().await {
                Ok(_) => {
                    ready = true;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
        assert!(ready, "host pipe never became ready: {last_err:?}");

        let nav = client
            .navigate("https://example.com/")
            .await
            .expect("navigate example.com");
        assert!(
            nav.url.contains("example.com"),
            "unexpected url {}",
            nav.url
        );

        let shot = client.screenshot().await.expect("screenshot");
        assert!(
            PathBuf::from(&shot.path).exists(),
            "missing screenshot {}",
            shot.path
        );
        assert!(shot.width > 0 && shot.height > 0, "{shot:?}");

        let snap = client.snapshot(false).await.expect("snapshot");
        assert!(
            snap.nodes
                .iter()
                .any(|n| n.role == "heading" || n.role == "link"),
            "expected heading or link in snapshot: {snap:?}"
        );

        let title = client
            .eval("() => document.title")
            .await
            .expect("eval title");
        assert!(title.as_str().is_some(), "{title:?}");

        let _ = client.shutdown().await;
        let _ = done_rx.recv_timeout(Duration::from_secs(20));
        let _ = host_thread.join();

        match prev_images {
            Some(v) => unsafe { std::env::set_var("TURBO_BROWSER_IMAGE_DIR", v) },
            None => unsafe { std::env::remove_var("TURBO_BROWSER_IMAGE_DIR") },
        }
    }
}
