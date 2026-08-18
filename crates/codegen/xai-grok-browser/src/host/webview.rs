//! WebView2 environment, controller, navigate, CDP, and page-control scripts.
//!
//! Two rules shape this module:
//!
//! * **Every** top-level navigation is policy-checked, not just the ones the
//!   agent asks for by name. A redirect or a link click is a navigation too.
//! * The page-control collector runs in a CDP **isolated world**, so page
//!   script cannot replace `__turboAx` and feed the agent a forged snapshot.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PERMISSION_STATE_DENY, CreateCoreWebView2EnvironmentWithOptions,
    GetAvailableCoreWebView2BrowserVersionString, ICoreWebView2, ICoreWebView2_4,
    ICoreWebView2Controller, ICoreWebView2Environment,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, MWMO_INPUTAVAILABLE,
    MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT,
    TranslateMessage, WM_QUIT,
};
use windows::core::{BOOL, Interface, PCWSTR, PWSTR};

use super::ax::{
    compact_ax_tree, interpret_uid_action, parse_collected_nodes, parse_eval_cdp,
    parse_world_result, snapshot_cap, turbo_ax_js_injected,
};
use super::window::{attach_controller, client_rect};
use super::{HostError, next_screenshot_path, screenshot_dir};
use crate::protocol::{
    AxNode, FillTarget, NavigateResult, ScreenshotResult, SnapshotResult, SnapshotSource, TabInfo,
    TabsResult, check_fill_target, check_url_in_session,
};

/// Ceiling on a single script / CDP round trip before the host gives up.
///
/// Without this a `while(true){}` on the page wedges the UI thread forever, and
/// because requests are serialized even `browser.shutdown` cannot get through.
pub const OP_TIMEOUT: Duration = Duration::from_secs(30);
/// Ceiling on one navigation (slower: DNS, TLS, redirects, heavy documents).
pub const NAV_TIMEOUT: Duration = Duration::from_secs(60);
/// Name of the CDP isolated world the collector lives in.
const AX_WORLD: &str = "turbo_agent_ax";

/// Fail closed if the Evergreen WebView2 Runtime is not present.
pub fn ensure_runtime_installed() -> Result<(), HostError> {
    let mut version = PWSTR::null();
    // SAFETY: null browserExecutableFolder selects the Evergreen runtime;
    // `version` is an out PWSTR we free via take_pwstr on success.
    match unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) } {
        Ok(()) => {
            let _ = take_pwstr(version);
            Ok(())
        }
        Err(_) => Err(HostError::RuntimeMissing),
    }
}

/// Why the last navigation was refused, recorded by the NavigationStarting
/// handler so `navigate` can report the policy reason instead of a generic
/// "navigation failed".
#[derive(Default)]
struct BlockLog {
    last: RefCell<Option<String>>,
}

impl BlockLog {
    fn set(&self, message: String) {
        *self.last.borrow_mut() = Some(message);
    }
    fn take(&self) -> Option<String> {
        self.last.borrow_mut().take()
    }
}

/// Single-tab WebView2 controller owned by the UI thread.
pub struct AgentWebView {
    hwnd: HWND,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    session_id: String,
    screenshot_n: u32,
    /// Isolated-world execution context for the collector; dropped on navigate.
    ax_world: Option<i64>,
    blocked: Rc<BlockLog>,
}

impl AgentWebView {
    /// Create the environment (user-data-dir = profile) and controller.
    ///
    /// `session_folder` widens the URL policy to allow `file:` beneath it; the
    /// same value the client uses, so both ends agree on what is reachable.
    pub fn create(
        hwnd: HWND,
        user_data_dir: &Path,
        session_id: &str,
        session_folder: Option<PathBuf>,
    ) -> Result<Self, HostError> {
        let environment = create_environment(user_data_dir)?;
        let controller = create_controller(&environment, hwnd)?;

        let bounds = client_rect(hwnd);
        // SAFETY: controller is bound to `hwnd`; bounds are the client rect.
        unsafe {
            controller
                .SetBounds(bounds)
                .map_err(|e| HostError::Failed(format!("SetBounds: {e}")))?;
            controller
                .SetIsVisible(true)
                .map_err(|e| HostError::Failed(format!("SetIsVisible: {e}")))?;
        }

        let webview = unsafe { controller.CoreWebView2() }
            .map_err(|e| HostError::Failed(format!("CoreWebView2: {e}")))?;

        apply_settings(&webview)?;
        let blocked = Rc::new(BlockLog::default());
        register_navigation_policy(&webview, session_folder, Rc::clone(&blocked))?;
        register_popup_download_permission(&webview, Rc::clone(&blocked))?;
        attach_controller(hwnd, controller.clone());

        let mut host = Self {
            hwnd,
            controller,
            webview,
            session_id: session_id.to_owned(),
            screenshot_n: 0,
            ax_world: None,
            blocked,
        };
        // First paint: about:blank until an agent navigate.
        host.navigate("about:blank").map_err(HostError::Failed)?;
        Ok(host)
    }

    /// `ICoreWebView2::Navigate` and wait for `NavigationCompleted`.
    pub fn navigate(&mut self, url: &str) -> Result<NavigateResult, String> {
        let _ = self.blocked.take();
        let (tx, rx) = mpsc::channel();
        let handler = NavigationCompletedEventHandler::create(Box::new(move |_sender, args| {
            let success = match args {
                Some(args) => {
                    let mut ok = BOOL(0);
                    match unsafe { args.IsSuccess(&mut ok) } {
                        Ok(()) => ok.as_bool(),
                        Err(_) => false,
                    }
                }
                None => false,
            };
            let _ = tx.send(success);
            Ok(())
        }));

        let mut token = 0i64;
        unsafe {
            self.webview
                .add_NavigationCompleted(&handler, &mut token)
                .map_err(|e| format!("add_NavigationCompleted: {e}"))?;
        }
        // Always detach, including timeout / Navigate failure.
        let _remove = RemoveNavCompleted {
            webview: &self.webview,
            token,
        };

        let wide = CoTaskMemPWSTR::from(url);
        // SAFETY: `wide` lives for the Navigate call.
        let nav = unsafe { self.webview.Navigate(*wide.as_ref().as_pcwstr()) };
        if let Err(err) = nav {
            return Err(format!("Navigate: {err}"));
        }

        // A navigation replaces the document, so the isolated world is gone.
        self.ax_world = None;
        let success = pump_until(&rx, NAV_TIMEOUT).map_err(|e| e.describe("navigate"))?;
        if !success {
            // Prefer the policy reason over "navigation failed" — cancelling in
            // NavigationStarting surfaces here as a plain failure otherwise.
            return Err(self
                .blocked
                .take()
                .unwrap_or_else(|| format!("navigation failed: {url}")));
        }
        if let Some(reason) = self.blocked.take() {
            return Err(reason);
        }
        self.location()
    }

    /// Compact AX snapshot from the isolated-world collector (CDP fallback).
    pub fn snapshot(&mut self, verbose: bool) -> Result<SnapshotResult, String> {
        let cap = snapshot_cap(verbose);
        let js = format!("window.__turboAx.collect({cap})");
        let (nodes, source) = match self.eval_in_world(&js) {
            Ok(value) => (parse_collected_nodes(&value, cap)?, SnapshotSource::Dom),
            Err(_) => (
                self.snapshot_cdp_fallback(verbose)?,
                SnapshotSource::AxFallback,
            ),
        };
        let loc = self.location()?;
        Ok(SnapshotResult {
            url: loc.url,
            title: loc.title,
            source,
            nodes,
        })
    }

    fn snapshot_cdp_fallback(&self, verbose: bool) -> Result<Vec<AxNode>, String> {
        let _ = call_cdp(&self.webview, "Accessibility.enable", "{}");
        let tree = call_cdp(&self.webview, "Accessibility.getFullAXTree", "{}")?;
        compact_ax_tree(&tree, verbose)
    }

    /// Click `[data-turbo-uid=…]`. Missing or stale node → `unknown_uid`.
    pub fn click(&mut self, uid: &str) -> Result<(), String> {
        let js = format!("window.__turboAx.click({uid})", uid = js_string(uid));
        let raw = self.eval_in_world(&js)?;
        interpret_uid_action(uid, &raw)?;
        Ok(())
    }

    /// Fill a tagged control. Policy is re-checked against the resolved field
    /// (type / autocomplete / name) **before** mutating the page.
    pub fn fill(&mut self, uid: &str, value: &str) -> Result<(), String> {
        let lookup = format!("window.__turboAx.lookup({uid})", uid = js_string(uid));
        let probe = interpret_uid_action(uid, &self.eval_in_world(&lookup)?)?;
        check_fill_target(
            value,
            &FillTarget {
                name: probe.get("name").and_then(Value::as_str),
                secret: probe.get("secret").and_then(Value::as_str),
            },
        )
        .map_err(|e| e.to_string())?;

        let fill_js = format!(
            "window.__turboAx.fill({uid},{value})",
            uid = js_string(uid),
            value = js_string(value)
        );
        interpret_uid_action(uid, &self.eval_in_world(&fill_js)?)?;
        Ok(())
    }

    /// CDP `Runtime.evaluate` of a function expression in the page's own world.
    ///
    /// Wrapped in `Promise.resolve(...)` so an `async` function returns its
    /// awaited value rather than a stringified Promise (`{}`).
    pub fn eval_function(&mut self, function: &str) -> Result<Value, String> {
        let expression = format!(
            "Promise.resolve(({function})()).then(function(v){{return JSON.stringify(v);}})"
        );
        let params = serde_json::json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        })
        .to_string();
        let json = call_cdp(&self.webview, "Runtime.evaluate", &params)?;
        parse_eval_cdp(&json)
    }

    /// Id of the main frame, for `Page.createIsolatedWorld`.
    fn main_frame_id(&self) -> Result<String, String> {
        let _ = call_cdp(&self.webview, "Page.enable", "{}");
        let tree = call_cdp(&self.webview, "Page.getFrameTree", "{}")?;
        let value: Value =
            serde_json::from_str(&tree).map_err(|e| format!("Page.getFrameTree JSON: {e}"))?;
        value
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "Page.getFrameTree has no main frame id".to_owned())
    }

    /// Create the collector's isolated world and define `__turboAx` in it.
    fn create_ax_world(&mut self) -> Result<i64, String> {
        let frame_id = self.main_frame_id()?;
        let params = serde_json::json!({
            "frameId": frame_id,
            "worldName": AX_WORLD,
            // CDP really does spell it this way.
            "grantUniveralAccess": false,
        })
        .to_string();
        let json = call_cdp(&self.webview, "Page.createIsolatedWorld", &params)?;
        let value: Value = serde_json::from_str(&json)
            .map_err(|e| format!("Page.createIsolatedWorld JSON: {e}"))?;
        let context = value
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| "Page.createIsolatedWorld returned no executionContextId".to_owned())?;
        let install = serde_json::json!({
            "expression": turbo_ax_js_injected().as_ref(),
            "contextId": context,
            "returnByValue": true,
        })
        .to_string();
        let installed = call_cdp(&self.webview, "Runtime.evaluate", &install)?;
        parse_world_result(&installed)?;
        self.ax_world = Some(context);
        Ok(context)
    }

    /// Evaluate `js` in the collector's isolated world, recreating it if the
    /// context went away (navigation, renderer restart).
    fn eval_in_world(&mut self, js: &str) -> Result<Value, String> {
        let context = match self.ax_world {
            Some(id) => id,
            None => self.create_ax_world()?,
        };
        match self.eval_in_context(js, context) {
            Ok(value) => Ok(value),
            Err(_) => {
                // Stale context: rebuild the world once and retry.
                self.ax_world = None;
                let context = self.create_ax_world()?;
                self.eval_in_context(js, context)
            }
        }
    }

    fn eval_in_context(&self, js: &str, context: i64) -> Result<Value, String> {
        let params = serde_json::json!({
            "expression": js,
            "contextId": context,
            "returnByValue": true,
        })
        .to_string();
        let json = call_cdp(&self.webview, "Runtime.evaluate", &params)?;
        parse_world_result(&json)
    }

    /// CDP `Page.captureScreenshot` → PNG file + IHDR size.
    pub fn screenshot(&mut self) -> Result<ScreenshotResult, String> {
        let json = call_cdp(
            &self.webview,
            "Page.captureScreenshot",
            r#"{"format":"png"}"#,
        )?;
        let (png, width, height) = super::decode_cdp_png(&json)?;
        let dir = screenshot_dir(&self.session_id);
        // Seed past any images a previous host in this session already wrote,
        // so a restart cannot silently overwrite browser-1.png.
        if self.screenshot_n == 0 {
            self.screenshot_n = super::highest_screenshot_index(&dir);
        }
        self.screenshot_n = self.screenshot_n.saturating_add(1);
        let path = next_screenshot_path(&dir, self.screenshot_n)?;
        std::fs::write(&path, png).map_err(|e| format!("write screenshot: {e}"))?;
        Ok(ScreenshotResult {
            path: path.to_string_lossy().into_owned(),
            width,
            height,
        })
    }

    /// Single-tab `browser.tabs` result.
    pub fn current_tab(&self) -> Result<TabsResult, String> {
        let loc = self.location()?;
        Ok(TabsResult {
            tabs: vec![TabInfo {
                tab_id: 1,
                url: loc.url,
                title: loc.title,
                active: true,
            }],
        })
    }

    /// Current Source + DocumentTitle.
    pub fn location(&self) -> Result<NavigateResult, String> {
        let mut uri = PWSTR::null();
        unsafe {
            self.webview
                .Source(&mut uri)
                .map_err(|e| format!("Source: {e}"))?;
        }
        let url = take_pwstr(uri);

        let mut title = PWSTR::null();
        unsafe {
            self.webview
                .DocumentTitle(&mut title)
                .map_err(|e| format!("DocumentTitle: {e}"))?;
        }
        let title = take_pwstr(title);
        Ok(NavigateResult { url, title })
    }

    /// Close the controller (window teardown also closes it).
    pub fn close(&self) {
        let _ = self.hwnd;
        let _ = unsafe { self.controller.Close() };
    }
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

// ---------------------------------------------------------------------------
// Bounded message pump
// ---------------------------------------------------------------------------

/// Why a bounded pump stopped without a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PumpError {
    /// Deadline elapsed with no completion callback.
    Timeout,
    /// `WM_QUIT` arrived; the host is shutting down.
    Quit,
    /// Sender dropped without producing a value.
    Closed,
}

impl PumpError {
    pub(crate) fn describe(self, what: &str) -> String {
        match self {
            Self::Timeout => format!("{what}: timed out"),
            Self::Quit => format!("{what}: host is shutting down"),
            Self::Closed => format!("{what}: channel closed"),
        }
    }
}

/// Pump UI messages until `rx` yields, `timeout` elapses, or `WM_QUIT` arrives.
///
/// `webview2_com::wait_with_pump` blocks in `GetMessage` forever; a hung
/// renderer would wedge the host with no way in or out. This is the same pump
/// with a deadline, and it re-posts `WM_QUIT` so the outer loop still exits.
fn pump_until<T>(rx: &mpsc::Receiver<T>, timeout: Duration) -> Result<T, PumpError> {
    let deadline = Instant::now() + timeout;
    let mut msg = MSG::default();
    loop {
        match rx.try_recv() {
            Ok(value) => return Ok(value),
            Err(mpsc::TryRecvError::Disconnected) => return Err(PumpError::Closed),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(PumpError::Timeout);
        }
        let slice = (deadline - now).as_millis().min(50) as u32;
        // SAFETY: waits on this thread's message queue only; no handles.
        unsafe {
            MsgWaitForMultipleObjectsEx(
                None,
                slice,
                QS_ALLINPUT,
                MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(MWMO_INPUTAVAILABLE.0),
            );
        }
        // SAFETY: standard PeekMessage drain on the owning UI thread.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                // We consumed it; the outer GetMessageW loop still needs it.
                unsafe { PostQuitMessage(msg.wParam.0 as i32) };
                return Err(PumpError::Quit);
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if let Ok(value) = rx.try_recv() {
                return Ok(value);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event handlers: navigation policy, popups, downloads, permissions
// ---------------------------------------------------------------------------

/// Gate **every** top-level navigation on the URL policy.
///
/// Checking only `browser.navigate` made `GROK_BROWSER_ALLOW` a one-hop check:
/// a 302, a meta refresh, or a clicked link walked straight out of it. Subframe
/// loads are deliberately not gated — third-party iframes are ordinary page
/// structure, and cancelling them breaks legitimate sites.
fn register_navigation_policy(
    webview: &ICoreWebView2,
    session_folder: Option<PathBuf>,
    blocked: Rc<BlockLog>,
) -> Result<(), HostError> {
    let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut uri = PWSTR::null();
        if unsafe { args.Uri(&mut uri) }.is_err() {
            return Ok(());
        }
        let url = take_pwstr(uri);
        if let Err(err) = check_url_in_session(&url, session_folder.as_deref()) {
            let message = format!("blocked navigation to {url}: {err}");
            eprintln!("turbo browser-host: {message}");
            blocked.set(message);
            let _ = unsafe { args.SetCancel(true) };
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NavigationStarting(&handler, &mut token) }
        .map_err(|e| HostError::Failed(format!("add_NavigationStarting: {e}")))?;
    Ok(())
}

/// Keep `window.open` in the same view, refuse downloads, deny permissions.
fn register_popup_download_permission(
    webview: &ICoreWebView2,
    blocked: Rc<BlockLog>,
) -> Result<(), HostError> {
    // window.open / target=_blank. Unhandled, WebView2 spawns a runtime-owned
    // popup the agent cannot see, drive, or close — and browser.tabs would
    // still report one tab. Redirect it into this view instead.
    let nav_target = webview.clone();
    let popup_blocked = Rc::clone(&blocked);
    let popup = NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let _ = unsafe { args.SetHandled(true) };
        let mut uri = PWSTR::null();
        if unsafe { args.Uri(&mut uri) }.is_err() {
            return Ok(());
        }
        let url = take_pwstr(uri);
        // The policy check still applies: a popup is a navigation.
        if let Err(err) = crate::protocol::check_url(&url) {
            let message = format!("blocked popup to {url}: {err}");
            eprintln!("turbo browser-host: {message}");
            popup_blocked.set(message);
            return Ok(());
        }
        let wide = CoTaskMemPWSTR::from(url.as_str());
        let _ = unsafe { nav_target.Navigate(*wide.as_ref().as_pcwstr()) };
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NewWindowRequested(&popup, &mut token) }
        .map_err(|e| HostError::Failed(format!("add_NewWindowRequested: {e}")))?;

    // Downloads are a side effect on the user's disk that nobody approved.
    // `add_DownloadStarting` arrived in ICoreWebView2_4; on an older Evergreen
    // runtime we simply cannot intercept, so say so rather than refusing to
    // start.
    let download = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let _ = unsafe { args.SetCancel(true) };
        let message = "blocked a download started by the page".to_owned();
        eprintln!("turbo browser-host: {message}");
        blocked.set(message);
        Ok(())
    }));
    match webview.cast::<ICoreWebView2_4>() {
        Ok(wv4) => {
            let mut token = 0i64;
            unsafe { wv4.add_DownloadStarting(&download, &mut token) }
                .map_err(|e| HostError::Failed(format!("add_DownloadStarting: {e}")))?;
        }
        Err(_) => {
            eprintln!(
                "turbo browser-host: WebView2 runtime predates ICoreWebView2_4; \
                 page-initiated downloads cannot be blocked"
            );
        }
    }

    // Geolocation / camera / mic / clipboard. Unhandled these raise UI that
    // blocks the UI thread, which is exactly what OP_TIMEOUT exists to avoid.
    let permission = PermissionRequestedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            let _ = unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) };
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_PermissionRequested(&permission, &mut token) }
        .map_err(|e| HostError::Failed(format!("add_PermissionRequested: {e}")))?;
    Ok(())
}

fn apply_settings(webview: &ICoreWebView2) -> Result<(), HostError> {
    let settings =
        unsafe { webview.Settings() }.map_err(|e| HostError::Failed(format!("Settings: {e}")))?;
    unsafe {
        settings
            .SetAreDefaultContextMenusEnabled(true)
            .map_err(|e| HostError::Failed(format!("AreDefaultContextMenusEnabled: {e}")))?;
        settings
            .SetAreDevToolsEnabled(true)
            .map_err(|e| HostError::Failed(format!("AreDevToolsEnabled: {e}")))?;
        settings
            .SetIsZoomControlEnabled(true)
            .map_err(|e| HostError::Failed(format!("IsZoomControlEnabled: {e}")))?;
        settings
            .SetAreHostObjectsAllowed(false)
            .map_err(|e| HostError::Failed(format!("AreHostObjectsAllowed: {e}")))?;
        // With this off and no ScriptDialogOpening handler, alert/confirm/
        // beforeunload resolve immediately instead of raising a modal that
        // blocks the UI thread until a human clicks it.
        settings
            .SetAreDefaultScriptDialogsEnabled(false)
            .map_err(|e| HostError::Failed(format!("AreDefaultScriptDialogsEnabled: {e}")))?;
    }
    Ok(())
}

fn create_environment(user_data_dir: &Path) -> Result<ICoreWebView2Environment, HostError> {
    let folder = user_data_dir.to_string_lossy().into_owned();
    let (tx, rx) = mpsc::channel();

    let start = CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            let wide = CoTaskMemPWSTR::from(folder.as_str());
            // SAFETY: `wide` lives for the FFI call; browserExecutableFolder
            // is null (Evergreen). userDataFolder is the agent profile.
            unsafe {
                CreateCoreWebView2EnvironmentWithOptions(
                    PCWSTR::null(),
                    *wide.as_ref().as_pcwstr(),
                    None,
                    &handler,
                )
                .map_err(webview2_com::Error::WindowsError)
            }
        }),
        Box::new(move |error_code, environment| {
            error_code?;
            tx.send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .map_err(|_| windows::core::Error::from(E_POINTER))?;
            Ok(())
        }),
    );

    match start {
        Ok(()) => {}
        Err(webview2_com::Error::WindowsError(e)) if is_runtime_missing(&e) => {
            return Err(HostError::RuntimeMissing);
        }
        Err(e) => return Err(map_webview_err(e)),
    }

    match rx.recv() {
        Ok(Ok(env)) => Ok(env),
        Ok(Err(e)) if is_runtime_missing(&e) => Err(HostError::RuntimeMissing),
        Ok(Err(e)) => Err(HostError::Failed(format!(
            "CreateCoreWebView2EnvironmentWithOptions: {e}"
        ))),
        Err(_) => Err(HostError::Failed(
            "CreateCoreWebView2EnvironmentWithOptions: channel closed".into(),
        )),
    }
}

fn create_controller(
    environment: &ICoreWebView2Environment,
    hwnd: HWND,
) -> Result<ICoreWebView2Controller, HostError> {
    let (tx, rx) = mpsc::channel();
    let env = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            env.CreateCoreWebView2Controller(hwnd, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error_code, controller| {
            error_code?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .map_err(|_| windows::core::Error::from(E_POINTER))?;
            Ok(())
        }),
    )
    .map_err(map_webview_err)?;

    rx.recv()
        .map_err(|_| HostError::Failed("CreateCoreWebView2Controller: channel closed".into()))?
        .map_err(|e| HostError::Failed(format!("CreateCoreWebView2Controller: {e}")))
}

fn call_cdp(webview: &ICoreWebView2, method: &str, params_json: &str) -> Result<String, String> {
    let (tx, rx) = mpsc::channel();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |error_code, result| {
            error_code?;
            let _ = tx.send(result);
            Ok(())
        }));
    let method_w = CoTaskMemPWSTR::from(method);
    let params_w = CoTaskMemPWSTR::from(params_json);
    // SAFETY: both wide strings live for the duration of the call.
    unsafe {
        webview.CallDevToolsProtocolMethod(
            *method_w.as_ref().as_pcwstr(),
            *params_w.as_ref().as_pcwstr(),
            &handler,
        )
    }
    .map_err(|e| format!("CDP {method}: {e}"))?;
    pump_until(&rx, OP_TIMEOUT).map_err(|e| e.describe(&format!("CDP {method}")))
}

struct RemoveNavCompleted<'a> {
    webview: &'a ICoreWebView2,
    token: i64,
}

impl Drop for RemoveNavCompleted<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.webview.remove_NavigationCompleted(self.token) };
    }
}

/// `wait_for_async_operation` returns this after `GetMessage` sees `WM_QUIT`
/// (and consumes it). The outer host loop hangs unless we re-post quit.
pub(crate) fn pump_consumed_quit(err: &webview2_com::Error) -> bool {
    matches!(err, webview2_com::Error::TaskCanceled)
}

fn map_webview_err(err: webview2_com::Error) -> HostError {
    if pump_consumed_quit(&err) {
        // SAFETY: UI thread; re-queues WM_QUIT for the outer GetMessageW loop.
        unsafe { PostQuitMessage(0) };
    }
    match err {
        webview2_com::Error::WindowsError(e) if is_runtime_missing(&e) => HostError::RuntimeMissing,
        other => HostError::Failed(other.to_string()),
    }
}

fn is_runtime_missing(err: &windows::core::Error) -> bool {
    let code = err.code().0 as u32;
    // HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND / ERROR_MOD_NOT_FOUND / ERROR_NOT_FOUND)
    const FILE_NOT_FOUND: u32 = 0x8007_0002;
    const MOD_NOT_FOUND: u32 = 0x8007_007E;
    const NOT_FOUND: u32 = 0x8007_0490;
    if matches!(code, FILE_NOT_FOUND | MOD_NOT_FOUND | NOT_FOUND) {
        return true;
    }
    // Fallback only: `message()` is localized, so the HRESULTs above are the
    // real signal and this just widens the net on English installs.
    let lower = err.message().to_ascii_lowercase();
    lower.contains("webview2") && (lower.contains("not found") || lower.contains("not installed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_canceled_means_nested_pump_consumed_quit() {
        assert!(
            pump_consumed_quit(&webview2_com::Error::TaskCanceled),
            "WM_QUIT inside wait_with_pump must be treated as a consumed quit"
        );
        assert!(
            !pump_consumed_quit(&webview2_com::Error::SendError),
            "channel errors are not a consumed WM_QUIT"
        );
        assert!(!pump_consumed_quit(&webview2_com::Error::CallbackError(
            "x".into()
        )));
    }

    #[test]
    fn pump_until_reports_timeout_without_a_sender() {
        let (_tx, rx) = mpsc::channel::<u8>();
        let start = Instant::now();
        let err = pump_until(&rx, Duration::from_millis(120)).unwrap_err();
        assert_eq!(err, PumpError::Timeout);
        assert!(
            start.elapsed() >= Duration::from_millis(100),
            "returned early"
        );
        assert!(err.describe("CDP x").contains("timed out"));
    }

    #[test]
    fn pump_until_yields_a_ready_value() {
        let (tx, rx) = mpsc::channel();
        tx.send(7u8).unwrap();
        assert_eq!(pump_until(&rx, Duration::from_secs(1)).unwrap(), 7);
    }

    #[test]
    fn pump_until_reports_closed_channel() {
        let (tx, rx) = mpsc::channel::<u8>();
        drop(tx);
        assert_eq!(
            pump_until(&rx, Duration::from_secs(1)).unwrap_err(),
            PumpError::Closed
        );
    }

    #[test]
    fn operation_timeouts_are_bounded() {
        assert!(OP_TIMEOUT <= Duration::from_secs(60));
        assert!(NAV_TIMEOUT >= OP_TIMEOUT);
    }
}
