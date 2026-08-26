//! Sidecar host entry (`turbo browser-host`).
//!
//! Windows builds own a WS_OVERLAPPEDWINDOW + WebView2 controller and serve
//! newline-delimited JSON-RPC on the session named pipe. Non-Windows builds
//! return [`HostError::WindowsOnly`] (CLI maps to exit 2).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;

use crate::profile::pipe_name;
use crate::protocol::{
    BrowserRequest, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, JsonRpcVersion,
    check_eval_confirm, check_fill, check_navigation_hop, eval_looks_mutating,
    path_is_under_session_folder, single_tab_v1_error,
};

mod ax;
pub(crate) mod download;
#[cfg(windows)]
mod rpc;
#[cfg(windows)]
mod webview;
#[cfg(windows)]
mod window;

pub use ax::{
    SNAPSHOT_NODE_CAP, SNAPSHOT_NODE_CAP_VERBOSE, compact_ax_tree, pick_snapshot_nodes,
    resolve_uid, snapshot_cap,
};

/// HTTP error documents (404 HTML) still load a URL. Treat that as a successful
/// navigation so agents snapshot the error page instead of a dead tab.
pub(crate) fn http_error_document_counts_as_navigation(location_url: &str) -> bool {
    let loc = location_url.trim();
    !loc.is_empty() && !loc.eq_ignore_ascii_case("about:blank")
}

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
    /// Session folder. `file:` URLs beneath it are allowed; `None` denies all.
    ///
    /// Must match what the client was built with, or the two ends disagree
    /// about what is reachable and the client's allowance is a dead letter.
    pub session_folder: Option<PathBuf>,
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

/// Reject session ids that are not safe as a pipe segment and a path segment.
///
/// `session_id` reaches both `\\.\pipe\turbo-browser-<id>` and the screenshot
/// directory, so `..` or a separator in it would escape either namespace.
pub fn validate_session_id(session_id: &str) -> Result<(), HostError> {
    const MAX: usize = 64;
    let ok = !session_id.is_empty()
        && session_id.len() <= MAX
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(HostError::Failed(format!(
            "invalid session id {session_id:?}: expected 1-{MAX} chars of [A-Za-z0-9_-]"
        )))
    }
}

impl HostArgs {
    /// Fill empty `pipe` / `user_data_dir` with product defaults.
    pub fn resolve_defaults(mut self) -> Self {
        if self.pipe.is_empty() {
            self.pipe = pipe_name(&self.session_id);
        }
        if self.user_data_dir.as_os_str().is_empty() {
            self.user_data_dir = self.resolve_profile();
        }
        self
    }

    /// Resolve the WebView2 user-data-dir from env (fresh / session / durable).
    pub fn resolve_profile(&self) -> PathBuf {
        crate::profile::agent_browser_profile_dir(&self.session_id)
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
    /// `browser.tabs` (main view plus host-owned OAuth popups).
    Tabs,
    /// `browser.downloads` (session-scoped broker directory).
    Downloads,
    /// `browser.raise`
    Raise,
    /// `browser.shutdown`
    Shutdown,
    /// `browser.snapshot`
    Snapshot {
        /// Raise node cap from 200 to 800.
        verbose: bool,
        /// Include truncated main-landmark text.
        include_text: bool,
        /// Optional tab (`1` = main, `2+` = OAuth popup).
        tab_id: Option<u32>,
    },
    /// `browser.select_tab`
    SelectTab {
        /// Main tab is `1`; OAuth popups are `2+`.
        tab_id: u32,
    },
    /// `browser.close_tab`
    CloseTab {
        /// OAuth popup tab to close. The main tab cannot be closed.
        tab_id: u32,
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
        /// Caller confirmed a mutating script.
        confirm: bool,
    },
    /// `browser.wait`
    Wait {
        /// Visible text to wait for.
        text: Option<String>,
        /// URL substring to wait for.
        url_substring: Option<String>,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// `browser.scroll`
    Scroll {
        /// Optional uid to scroll into view.
        uid: Option<String>,
        /// Horizontal delta.
        dx: i32,
        /// Vertical delta.
        dy: i32,
    },
    /// `browser.press_key`
    PressKey {
        /// Key name (`Enter`, `Tab`, …).
        key: String,
        /// Optional uid to focus first.
        uid: Option<String>,
    },
    /// `browser.select`
    Select {
        /// Snapshot uid of a `<select>`.
        uid: String,
        /// Option value or label.
        value: String,
    },
    /// `browser.hover`
    Hover {
        /// Snapshot uid.
        uid: String,
    },
    /// `browser.set_file`
    SetFile {
        /// Snapshot uid of a file input.
        uid: String,
        /// Canonical session-folder path.
        path: String,
    },
}

/// Run the Agent WebView sidecar.
///
/// On Windows this blocks on the Win32 message loop until the window is
/// destroyed or `browser.shutdown` is received. Do not call this from a
/// default unit test.
pub fn run(args: HostArgs) -> Result<(), HostError> {
    validate_session_id(&args.session_id)?;
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

    let hwnd = window::create_frame_window(&args.session_id)?;
    window::show(hwnd);

    // Bind the named pipe *before* WebView2 environment/controller create.
    // Those can take tens of seconds on a cold Evergreen install; the shell's
    // ensure() used to kill the child at 15s because the pipe did not exist
    // until after that work finished.
    let ui_thread_id = unsafe { GetCurrentThreadId() };
    let (cmd_tx, cmd_rx) = mpsc::channel::<rpc::UiJob>();
    let pipe_thread = rpc::spawn_pipe_thread(
        args.pipe.clone(),
        ui_thread_id,
        cmd_tx,
        args.session_folder.clone(),
    )?;

    let mut agent = webview::AgentWebView::create(
        hwnd,
        &args.user_data_dir,
        &args.session_id,
        args.session_folder.clone(),
    )?;

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
    // Say goodbye on the way out. The shell drains this stderr into tracing, so
    // a window that disappears is explainable instead of reading as a crash -
    // whether it went away by RPC shutdown, a dead pipe, or the job object
    // taking the host down with a restarting pager.
    eprintln!(
        "turbo browser-host: exiting (pump ended); session={}",
        args.session_id
    );
    Ok(())
}

#[cfg(windows)]
fn handle_ui_job(
    agent: &mut webview::AgentWebView,
    hwnd: windows::Win32::Foundation::HWND,
    call: Result<(JsonRpcId, HostCall), DecodedRpcError>,
) -> String {
    // The close button hides the frame rather than destroying it, so any call
    // may arrive at an invisible window. Un-hide before doing the work:
    // otherwise the agent drives a page nobody can see and `browser.screenshot`
    // captures a hidden frame. Shutdown is exempt - it is on its way out.
    if !matches!(call, Ok((_, HostCall::Shutdown))) {
        window::ensure_visible(hwnd);
    }
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
        Ok((id, HostCall::Tabs)) => match agent.list_tabs() {
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
        Ok((id, HostCall::Downloads)) => match agent.downloads() {
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
            window::raise(agent.selected_hwnd());
            encode_rpc_ok(id, Value::Object(serde_json::Map::new()))
        }
        Ok((id, HostCall::Shutdown)) => {
            window::destroy(hwnd);
            encode_rpc_ok(id, Value::Object(serde_json::Map::new()))
        }
        Ok((
            id,
            HostCall::Snapshot {
                verbose,
                include_text,
                tab_id,
            },
        )) => match agent.snapshot(verbose, include_text, tab_id) {
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
        Ok((id, HostCall::SelectTab { tab_id })) => match agent.select_tab(tab_id) {
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
        Ok((id, HostCall::CloseTab { tab_id })) => match agent.close_tab(tab_id) {
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
        Ok((id, HostCall::Click { uid })) => match agent.click(&uid) {
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
        Ok((
            id,
            HostCall::Eval {
                function,
                confirm: _,
            },
        )) => {
            if eval_looks_mutating(&function) {
                encode_rpc_error(
                    id,
                    JsonRpcError {
                        code: RPC_HOST_ERROR,
                        message: "eval writes to the page; prefer browser_click or browser_fill"
                            .into(),
                        data: None,
                    },
                )
            } else {
                match agent.eval_function(&function) {
                    Ok(value) => encode_rpc_ok(id, value),
                    Err(message) => encode_rpc_error(
                        id,
                        JsonRpcError {
                            code: RPC_HOST_ERROR,
                            message,
                            data: None,
                        },
                    ),
                }
            }
        }
        Ok((
            id,
            HostCall::Wait {
                text,
                url_substring,
                timeout_ms,
            },
        )) => match agent.wait(text.as_deref(), url_substring.as_deref(), timeout_ms) {
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
        Ok((id, HostCall::Scroll { uid, dx, dy })) => match agent.scroll(uid.as_deref(), dx, dy) {
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
        Ok((id, HostCall::PressKey { key, uid })) => match agent.press_key(&key, uid.as_deref()) {
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
        Ok((id, HostCall::Select { uid, value })) => match agent.select_option(&uid, &value) {
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
        Ok((id, HostCall::Hover { uid })) => match agent.hover(&uid) {
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
        Ok((id, HostCall::SetFile { uid, path })) => match agent.set_file(&uid, &path) {
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
/// closed). Invalid click/fill uids become `unknown_uid`. `browser.new_tab`
/// stays unimplemented; `select_tab` / `close_tab` address OAuth popups.
pub(crate) fn decode_host_call(
    line: &str,
    session_folder: Option<&Path>,
) -> Result<(JsonRpcId, HostCall), DecodedRpcError> {
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
            if let Err(err) = check_navigation_hop(Some(&url), session_folder) {
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
        BrowserRequest::Downloads {} => Ok((id, HostCall::Downloads)),
        BrowserRequest::Raise {} => Ok((id, HostCall::Raise)),
        BrowserRequest::Shutdown {} => Ok((id, HostCall::Shutdown)),
        BrowserRequest::Snapshot {
            verbose,
            include_text,
            tab_id,
        } => Ok((
            id,
            HostCall::Snapshot {
                verbose,
                include_text,
                tab_id,
            },
        )),
        BrowserRequest::SelectTab { tab_id } => Ok((id, HostCall::SelectTab { tab_id })),
        BrowserRequest::CloseTab { tab_id } => Ok((id, HostCall::CloseTab { tab_id })),
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
        BrowserRequest::Eval { function, confirm } => {
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
            if let Err(err) = check_eval_confirm(&function, confirm) {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_HOST_ERROR,
                        message: err.to_string(),
                        data: None,
                    },
                });
            }
            Ok((id, HostCall::Eval { function, confirm }))
        }
        BrowserRequest::Wait {
            text,
            url_substring,
            timeout_ms,
        } => {
            if text.as_ref().is_none_or(|s| s.trim().is_empty())
                && url_substring.as_ref().is_none_or(|s| s.trim().is_empty())
            {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_INVALID_PARAMS,
                        message: "browser.wait requires text or url_substring".into(),
                        data: None,
                    },
                });
            }
            Ok((
                id,
                HostCall::Wait {
                    text,
                    url_substring,
                    timeout_ms: timeout_ms.unwrap_or(15_000).min(60_000),
                },
            ))
        }
        BrowserRequest::Scroll { uid, dx, dy } => Ok((
            id,
            HostCall::Scroll {
                uid,
                dx: dx.unwrap_or(0),
                dy: dy.unwrap_or(0),
            },
        )),
        BrowserRequest::PressKey { key, uid } => {
            if key.trim().is_empty() {
                return Err(DecodedRpcError {
                    id: Some(id),
                    error: JsonRpcError {
                        code: RPC_INVALID_PARAMS,
                        message: "browser.press_key requires key".into(),
                        data: None,
                    },
                });
            }
            Ok((id, HostCall::PressKey { key, uid }))
        }
        BrowserRequest::Select { uid, value } => {
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
            Ok((id, HostCall::Select { uid, value }))
        }
        BrowserRequest::Hover { uid } => {
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
            Ok((id, HostCall::Hover { uid }))
        }
        BrowserRequest::SetFile { uid, path } => {
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
            let path = std::path::PathBuf::from(path);
            match session_folder {
                None => {
                    return Err(DecodedRpcError {
                        id: Some(id),
                        error: JsonRpcError {
                            code: RPC_HOST_ERROR,
                            message: "browser.set_file requires a session folder".into(),
                            data: None,
                        },
                    });
                }
                Some(folder) => {
                    if !path_is_under_session_folder(&path, folder) {
                        return Err(DecodedRpcError {
                            id: Some(id),
                            error: JsonRpcError {
                                code: RPC_HOST_ERROR,
                                message: format!(
                                    "file path is not under the session folder (`{}` must be brokered into the session uploads/ directory first)",
                                    path.display()
                                ),
                                data: None,
                            },
                        });
                    }
                    let canon_path = dunce::canonicalize(&path).map_err(|e| DecodedRpcError {
                        id: Some(id.clone()),
                        error: JsonRpcError {
                            code: RPC_HOST_ERROR,
                            message: format!(
                                "file path could not be canonicalized under the session folder: {e}"
                            ),
                            data: None,
                        },
                    })?;
                    if !path_is_under_session_folder(&canon_path, folder) {
                        return Err(DecodedRpcError {
                            id: Some(id),
                            error: JsonRpcError {
                                code: RPC_HOST_ERROR,
                                message: "canonical file path escaped the session folder".into(),
                                data: None,
                            },
                        });
                    }
                    Ok((
                        id,
                        HostCall::SetFile {
                            uid,
                            path: canon_path.to_string_lossy().into_owned(),
                        },
                    ))
                }
            }
        }
        BrowserRequest::NewTab { .. } => Err(DecodedRpcError {
            id: Some(id),
            error: JsonRpcError {
                code: RPC_METHOD_NOT_FOUND,
                message: single_tab_v1_error(&req.method),
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

/// Highest `browser-<n>.png` already in `dir` (0 when empty or unreadable).
///
/// A restarted host resets its counter, and without this it would overwrite
/// `browser-1.png` — a path the model may already have reported to the user.
pub(crate) fn highest_screenshot_index(dir: &Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("browser-"))
                .and_then(|n| n.strip_suffix(".png"))
                .and_then(|n| n.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::encode_rpc_request;
    use crate::protocol::{
        METHOD_CLICK, METHOD_CLOSE_TAB, METHOD_EVAL, METHOD_FILL, METHOD_NAVIGATE, METHOD_NEW_TAB,
        METHOD_SCREENSHOT, METHOD_SELECT_TAB, METHOD_SET_FILE, METHOD_SNAPSHOT,
    };

    #[test]
    fn empty_pipe_and_profile_resolve_to_defaults() {
        let _guard = crate::profile::ProfileEnvGuard::lock_cleared();
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
            session_folder: None,
        }
        .resolve_defaults();
        assert_eq!(resolved.session_id, "abc");
        assert_eq!(resolved.pipe, pipe_name("abc"));
        assert_eq!(
            resolved.user_data_dir,
            crate::profile::agent_browser_user_data_dir()
        );
    }

    #[test]
    fn user_data_dir_is_stable_across_two_constructed_hosts() {
        let _guard = crate::profile::ProfileEnvGuard::lock_cleared();
        let a = HostArgs {
            session_id: "abc".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
            session_folder: None,
        }
        .resolve_defaults();
        let b = HostArgs {
            session_id: "xyz".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
            session_folder: None,
        }
        .resolve_defaults();
        assert_eq!(a.user_data_dir, b.user_data_dir);
        assert_eq!(
            a.user_data_dir,
            crate::profile::agent_browser_user_data_dir()
        );
        assert_eq!(a.user_data_dir, a.resolve_profile());
    }

    #[test]
    fn session_profile_env_is_per_session() {
        let guard = crate::profile::ProfileEnvGuard::lock_cleared();
        guard.set_profile("session");
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
            session_folder: None,
        }
        .resolve_defaults();
        assert_eq!(
            resolved.user_data_dir,
            crate::profile::agent_browser_user_data_dir()
                .join("sessions")
                .join("abc")
        );
    }

    #[test]
    fn fresh_profile_flag_uses_temp_dir() {
        let guard = crate::profile::ProfileEnvGuard::lock_cleared();
        guard.set_fresh("1");
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: String::new(),
            user_data_dir: PathBuf::new(),
            session_folder: None,
        }
        .resolve_defaults();
        assert!(
            resolved.user_data_dir.starts_with(std::env::temp_dir()),
            "{:?}",
            resolved.user_data_dir
        );
        assert_ne!(
            resolved.user_data_dir,
            crate::profile::agent_browser_user_data_dir()
        );
    }

    #[test]
    fn explicit_pipe_and_profile_are_kept() {
        let pipe = r"\\.\pipe\custom-browser";
        let dir = PathBuf::from("/tmp/custom-agent-browser");
        let resolved = HostArgs {
            session_id: "abc".into(),
            pipe: pipe.into(),
            user_data_dir: dir.clone(),
            session_folder: None,
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
            session_folder: None,
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
        match decode_host_call(&nav_line, None).unwrap() {
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
        match decode_host_call(&shot_line, None).unwrap() {
            (JsonRpcId::Number(2), HostCall::Screenshot) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decode_set_file_refuses_workspace_path() {
        let tmp = std::env::temp_dir().join(format!(
            "turbo-set-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session = tmp.join("session");
        let workspace = tmp.join("ws");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let workspace_file = workspace.join("resume.pdf");
        std::fs::write(&workspace_file, b"%PDF").unwrap();
        let upload = session.join("uploads");
        std::fs::create_dir_all(&upload).unwrap();
        let session_file = upload.join("resume.pdf");
        std::fs::write(&session_file, b"%PDF").unwrap();

        let bad = encode_rpc_request(
            JsonRpcId::Number(40),
            METHOD_SET_FILE,
            serde_json::json!({
                "uid": "1-1",
                "path": workspace_file.to_string_lossy(),
            }),
        )
        .unwrap();
        let err = decode_host_call(&bad, Some(&session)).unwrap_err();
        assert_eq!(err.error.code, RPC_HOST_ERROR);
        assert!(
            err.error.message.contains("session folder"),
            "{}",
            err.error.message
        );

        let good = encode_rpc_request(
            JsonRpcId::Number(41),
            METHOD_SET_FILE,
            serde_json::json!({
                "uid": "1-1",
                "path": session_file.to_string_lossy(),
            }),
        )
        .unwrap();
        match decode_host_call(&good, Some(&session)).unwrap() {
            (_, HostCall::SetFile { uid, path }) => {
                assert_eq!(uid, "1-1");
                assert!(path.contains("resume.pdf"), "{path}");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn decode_rejects_file_url_before_navigate() {
        let line = encode_rpc_request(
            JsonRpcId::Number(3),
            METHOD_NAVIGATE,
            serde_json::json!({ "url": "file:///C:/Windows/notepad.exe" }),
        )
        .unwrap();
        let err = decode_host_call(&line, None).unwrap_err();
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
            let err = decode_host_call(&line, None).unwrap_err();
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
        match decode_host_call(&snap, None).unwrap() {
            (
                JsonRpcId::Number(5),
                HostCall::Snapshot {
                    verbose: true,
                    include_text: false,
                    tab_id: None,
                },
            ) => {}
            other => panic!("{other:?}"),
        }

        let click = encode_rpc_request(
            JsonRpcId::Number(6),
            METHOD_CLICK,
            serde_json::json!({ "uid": "1-1" }),
        )
        .unwrap();
        match decode_host_call(&click, None).unwrap() {
            (_, HostCall::Click { uid }) => assert_eq!(uid, "1-1"),
            other => panic!("{other:?}"),
        }

        let fill = encode_rpc_request(
            JsonRpcId::Number(7),
            METHOD_FILL,
            serde_json::json!({ "uid": "1-2", "value": "hello" }),
        )
        .unwrap();
        match decode_host_call(&fill, None).unwrap() {
            (_, HostCall::Fill { uid, value }) => {
                assert_eq!(uid, "1-2");
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
        match decode_host_call(&eval, None).unwrap() {
            (_, HostCall::Eval { function, confirm }) => {
                assert_eq!(function, "() => 1");
                assert!(!confirm);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decode_fill_rejects_otp_before_host() {
        let line = encode_rpc_request(
            JsonRpcId::Number(9),
            METHOD_FILL,
            serde_json::json!({ "uid": "1-2", "value": "123456" }),
        )
        .unwrap();
        let err = decode_host_call(&line, None).unwrap_err();
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
        let err = decode_host_call(&line, None).unwrap_err();
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
        let err = decode_host_call(&line, None).unwrap_err();
        assert_eq!(err.error.code, RPC_METHOD_NOT_FOUND);
        assert!(
            err.error.message.contains("not implemented"),
            "{}",
            err.error.message
        );
        assert!(
            err.error.message.contains("v1 is a single tab"),
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
    fn decode_select_and_close_tab_are_host_calls() {
        let select = encode_rpc_request(
            JsonRpcId::Number(12),
            METHOD_SELECT_TAB,
            serde_json::json!({ "tab_id": 2 }),
        )
        .unwrap();
        match decode_host_call(&select, None).unwrap() {
            (_, HostCall::SelectTab { tab_id: 2 }) => {}
            other => panic!("{other:?}"),
        }
        let close = encode_rpc_request(
            JsonRpcId::Number(12),
            METHOD_CLOSE_TAB,
            serde_json::json!({ "tab_id": 2 }),
        )
        .unwrap();
        match decode_host_call(&close, None).unwrap() {
            (_, HostCall::CloseTab { tab_id: 2 }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn decode_mutating_eval_without_confirm_fails_closed() {
        let line = encode_rpc_request(
            JsonRpcId::Number(13),
            METHOD_EVAL,
            serde_json::json!({
                "function": "() => document.forms[0].submit()",
                "confirm": false
            }),
        )
        .unwrap();
        let err = decode_host_call(&line, None).unwrap_err();
        assert_eq!(err.error.code, RPC_HOST_ERROR);
        assert!(
            err.error.message.contains("browser_click")
                || err.error.message.contains("writes to the page"),
            "{}",
            err.error.message
        );

        let still_denied = encode_rpc_request(
            JsonRpcId::Number(14),
            METHOD_EVAL,
            serde_json::json!({
                "function": "() => document.forms[0].submit()",
                "confirm": true
            }),
        )
        .unwrap();
        let err = decode_host_call(&still_denied, None).unwrap_err();
        assert_eq!(err.error.code, RPC_HOST_ERROR, "{err:?}");

        let read = encode_rpc_request(
            JsonRpcId::Number(15),
            METHOD_EVAL,
            serde_json::json!({ "function": "() => document.title" }),
        )
        .unwrap();
        match decode_host_call(&read, None).unwrap() {
            (
                _,
                HostCall::Eval {
                    confirm: false,
                    function,
                },
            ) => {
                assert_eq!(function, "() => document.title");
            }
            other => panic!("{other:?}"),
        }
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
            session_folder: None,
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

    #[test]
    fn http_404_document_is_a_loaded_page() {
        assert!(http_error_document_counts_as_navigation(
            "https://file-examples.com/missing"
        ));
        assert!(!http_error_document_counts_as_navigation("about:blank"));
        assert!(!http_error_document_counts_as_navigation(""));
        assert!(!http_error_document_counts_as_navigation("   "));
    }
}
