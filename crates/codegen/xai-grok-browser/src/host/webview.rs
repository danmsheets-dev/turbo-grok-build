//! WebView2 environment, controller, navigate, CDP, and page-control scripts.
//!
//! Two rules shape this module:
//!
//! * **Every** top-level navigation is policy-checked, not just the ones the
//!   agent asks for by name. A redirect or a link click is a navigation too.
//! * The page-control collector runs in a CDP **isolated world**, so page
//!   script cannot replace `__turboAx` and feed the agent a forged snapshot.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
    COREWEBVIEW2_PERMISSION_STATE_DENY, CreateCoreWebView2EnvironmentWithOptions,
    GetAvailableCoreWebView2BrowserVersionString, ICoreWebView2, ICoreWebView2_4,
    ICoreWebView2Controller, ICoreWebView2Deferral, ICoreWebView2Environment,
    ICoreWebView2NewWindowRequestedEventArgs,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler, StateChangedEventHandler,
    take_pwstr,
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
use super::download::{broker_attachment, list_brokered_downloads, recent_completed_download};
use super::window::{
    attach_controller, attach_controller_ex, client_rect, create_oauth_popup_window, set_title,
    show,
};
use super::{HostError, next_screenshot_path, screenshot_dir};
use crate::protocol::{
    AxNode, ClickResult, DownloadsResult, FillTarget, NavigateResult, ScreenshotResult,
    SnapshotResult, SnapshotSource, TabInfo, TabsResult, WaitResult, check_fill_target,
    check_navigation_hop, is_oauth_popup_url,
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
    session_folder: Option<PathBuf>,
    active_downloads: Rc<RefCell<HashSet<PathBuf>>>,
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
        let active_downloads = Rc::new(RefCell::new(HashSet::new()));
        register_navigation_policy(&webview, session_folder.clone(), Rc::clone(&blocked))?;
        register_popup_download_permission(
            &webview,
            environment.clone(),
            session_id,
            session_folder.clone(),
            Rc::clone(&blocked),
            Rc::clone(&active_downloads),
        )?;
        attach_controller(hwnd, controller.clone());

        let mut host = Self {
            hwnd,
            controller,
            webview,
            session_id: session_id.to_owned(),
            session_folder: session_folder.clone(),
            active_downloads,
            screenshot_n: 0,
            ax_world: None,
            blocked,
        };
        // First paint has to explain itself. `run_windows` shows the frame on
        // the line after this constructor returns, so whatever is in the
        // document right here is the first thing a human sees - and an empty
        // about:blank is a white rectangle that reads as "it hung". Navigate the
        // blank document first (the card needs a body to attach to), then write
        // the card into it.
        //
        // This is strictly before `rpc::spawn_pipe_thread`, so no agent navigate
        // can race us: the card can never clobber a real page.
        host.navigate("about:blank").map_err(HostError::Failed)?;
        host.paint_startup_placeholder(user_data_dir);
        Ok(host)
    }

    /// Write the "starting" card into the blank first document.
    ///
    /// Deliberately not `NavigateToString` and not a `data:` URL:
    /// `register_navigation_policy` gates every top-level navigation through
    /// `check_url_in_session`, which denies every scheme but https, local http,
    /// `about:blank` and session-folder `file:` - and WebView2 reports
    /// `NavigateToString` to `NavigationStarting` as a `data:` URI. Either would
    /// be cancelled by our own policy and log a bogus "blocked navigation".
    /// Writing into the already-navigated `about:blank` document leaves the
    /// policy untouched, and a real navigation replaces the whole document.
    ///
    /// Best effort: a card that fails to paint is cosmetic, so this logs and
    /// continues rather than failing startup.
    fn paint_startup_placeholder(&self, user_data_dir: &Path) {
        let params = serde_json::json!({
            "expression": startup_placeholder_js(user_data_dir),
            "returnByValue": true,
        })
        .to_string();
        if let Err(err) = call_cdp(&self.webview, "Runtime.evaluate", &params) {
            eprintln!("turbo browser-host: startup placeholder not painted: {err}");
        }
    }

    /// `ICoreWebView2::Navigate` and wait for `NavigationCompleted`, keeping the
    /// frame caption pointed at the page.
    ///
    /// The requested host goes up *before* the wait, so a slow or wedged load
    /// still says what it is loading rather than sitting on a stale caption; the
    /// resolved host replaces it once the navigation lands.
    pub fn navigate(&mut self, url: &str) -> Result<NavigateResult, String> {
        set_title(self.hwnd, &self.session_id, url);
        let out = self.navigate_inner(url);
        let settled = match &out {
            Ok(res) => res.url.clone(),
            // Failed load: the window still shows whatever the page left behind,
            // so name the attempt rather than claim success.
            Err(_) => url.to_owned(),
        };
        set_title(self.hwnd, &self.session_id, &settled);
        out
    }

    fn navigate_inner(&mut self, url: &str) -> Result<NavigateResult, String> {
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
            if let Some(reason) = self.blocked.take() {
                return Err(reason);
            }
            // Zip/PDF attachments often complete as DownloadStarting and leave
            // NavigationCompleted IsSuccess=false. Treat an in-flight brokered
            // download as success so the agent can list it.
            if !self.active_downloads.borrow().is_empty() {
                return Ok(NavigateResult {
                    url: url.to_owned(),
                    title: "Download in progress".to_owned(),
                });
            }
            if let Some(saved) = recent_completed_download(
                self.session_folder.as_deref(),
                &self.active_downloads.borrow(),
            ) {
                return Ok(NavigateResult {
                    url: url.to_owned(),
                    title: format!("Saved download {}", saved.name),
                });
            }
            // HTTP error documents (404 HTML) still have a Source URL.
            if let Ok(loc) = self.location()
                && !loc.url.is_empty()
                && !loc.url.eq_ignore_ascii_case("about:blank")
            {
                return Ok(loc);
            }
            return Err(format!("navigation failed: {url}"));
        }
        if let Some(reason) = self.blocked.take() {
            return Err(reason);
        }
        self.location()
    }

    /// Compact AX snapshot from the isolated-world collector (CDP fallback).
    pub fn snapshot(
        &mut self,
        verbose: bool,
        include_text: bool,
    ) -> Result<SnapshotResult, String> {
        let loc = self.location()?;
        let cap = snapshot_cap(verbose);
        let js = format!("window.__turboAx.collect({cap})");
        let (dom_nodes, overlay) = match self.eval_in_world(&js) {
            Ok(value) => {
                let overlay = value.get("overlay").and_then(Value::as_bool);
                (
                    parse_collected_nodes(&value, cap).unwrap_or_default(),
                    overlay,
                )
            }
            Err(_) => (Vec::new(), None),
        };
        let ax_fallback = if dom_nodes.is_empty() {
            self.snapshot_cdp_fallback(verbose)
        } else {
            Ok(Vec::new())
        };
        let (nodes, source) = super::ax::pick_snapshot_nodes(dom_nodes, ax_fallback, &loc.url)?;
        let text = if include_text && source == SnapshotSource::Dom {
            self.eval_in_world("window.__turboAx.pageText()")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        Ok(SnapshotResult {
            url: loc.url,
            title: loc.title,
            source,
            overlay,
            text,
            nodes,
        })
    }

    fn snapshot_cdp_fallback(&self, verbose: bool) -> Result<Vec<AxNode>, String> {
        let _ = call_cdp(&self.webview, "Accessibility.enable", "{}");
        let tree = call_cdp(&self.webview, "Accessibility.getFullAXTree", "{}")?;
        compact_ax_tree(&tree, verbose)
    }

    /// Click `[data-turbo-uid=…]`. Missing or stale node → `unknown_uid`.
    ///
    /// After the click, if NavigationStarting cancelled the resulting hop,
    /// surface that BlockLog instead of pretending the click succeeded on the
    /// original page.
    pub fn click(&mut self, uid: &str) -> Result<ClickResult, String> {
        let _ = self.blocked.take();
        let js = format!("window.__turboAx.click({uid})", uid = js_string(uid));
        let raw = self.eval_in_world(&js)?;
        interpret_uid_action(uid, &raw)?;
        pump_for(Duration::from_millis(400));
        if let Some(reason) = self.blocked.take() {
            return Err(reason);
        }
        let loc = self.location()?;
        set_title(self.hwnd, &self.session_id, &loc.url);
        Ok(ClickResult {
            url: loc.url,
            title: loc.title,
        })
    }

    /// Poll until `text` appears in the page or the URL contains `url_substring`.
    pub fn wait(
        &mut self,
        text: Option<&str>,
        url_substring: Option<&str>,
        timeout_ms: u64,
    ) -> Result<WaitResult, String> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        let wanted_text = text.map(str::trim).filter(|s| !s.is_empty());
        let wanted_url = url_substring.map(str::trim).filter(|s| !s.is_empty());
        loop {
            let loc = self.location()?;
            let url_ok = wanted_url.is_none_or(|needle| {
                loc.url
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            });
            let text_ok = match wanted_text {
                None => true,
                Some(needle) => self
                    .eval_in_world(&format!(
                        "window.__turboAx.pageContains({needle})",
                        needle = js_string(needle)
                    ))
                    .ok()
                    .and_then(|v| v.as_bool())
                    // Immediately after a successful navigation the isolated
                    // collector may not have been injected yet. Fall back to
                    // the document text in the main world rather than timing
                    // out even though the page has already landed.
                    .or_else(|| {
                        let params = serde_json::json!({
                            "expression": format!(
                                "String(document.body?.innerText || '').toLowerCase().includes({})",
                                js_string(&needle.to_ascii_lowercase())
                            ),
                            "returnByValue": true,
                        })
                        .to_string();
                        call_cdp(&self.webview, "Runtime.evaluate", &params)
                            .ok()
                            .and_then(|value| serde_json::from_str::<Value>(&value).ok())
                            .and_then(|value| {
                                value.pointer("/result/value").and_then(Value::as_bool)
                            })
                    })
                    .unwrap_or(false),
            };
            if url_ok && text_ok {
                return Ok(WaitResult {
                    url: loc.url,
                    title: loc.title,
                });
            }
            if Instant::now() >= deadline {
                let mut unmet = Vec::new();
                if !url_ok && let Some(u) = wanted_url {
                    unmet.push(format!("url containing {u:?}"));
                }
                if !text_ok && let Some(t) = wanted_text {
                    unmet.push(format!("text {t:?}"));
                }
                return Err(format!(
                    "wait timed out after {timeout_ms}ms; unmet: {}; current url: {}",
                    unmet.join(" and "),
                    loc.url
                ));
            }
            pump_for(Duration::from_millis(150));
        }
    }

    /// Scroll a uid into view, or scroll the window by `(dx, dy)`.
    pub fn scroll(&mut self, uid: Option<&str>, dx: i32, dy: i32) -> Result<(), String> {
        if let Some(uid) = uid.filter(|s| !s.is_empty()) {
            let js = format!("window.__turboAx.scrollTo({uid})", uid = js_string(uid));
            interpret_uid_action(uid, &self.eval_in_world(&js)?)?;
            return Ok(());
        }
        let js = format!("window.__turboAx.scrollBy({dx},{dy})");
        let _ = self.eval_in_world(&js)?;
        Ok(())
    }

    /// Dispatch a key (optionally targeting a uid first).
    pub fn press_key(&mut self, key: &str, uid: Option<&str>) -> Result<(), String> {
        let js = match uid.filter(|s| !s.is_empty()) {
            Some(uid) => format!(
                "window.__turboAx.pressKey({key},{uid})",
                key = js_string(key),
                uid = js_string(uid)
            ),
            None => format!(
                "window.__turboAx.pressKey({key},null)",
                key = js_string(key)
            ),
        };
        let raw = self.eval_in_world(&js)?;
        if raw.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(raw
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("press_key failed")
                .to_owned());
        }
        Ok(())
    }

    /// Choose a `<select>` option by value or label.
    pub fn select_option(&mut self, uid: &str, value: &str) -> Result<(), String> {
        let js = format!(
            "window.__turboAx.select({uid},{value})",
            uid = js_string(uid),
            value = js_string(value)
        );
        interpret_uid_action(uid, &self.eval_in_world(&js)?)?;
        Ok(())
    }

    /// Hover a uid.
    pub fn hover(&mut self, uid: &str) -> Result<(), String> {
        let js = format!("window.__turboAx.hover({uid})", uid = js_string(uid));
        interpret_uid_action(uid, &self.eval_in_world(&js)?)?;
        Ok(())
    }

    /// Set an `<input type=file>` from a session-folder path via CDP.
    pub fn set_file(&mut self, uid: &str, path: &str) -> Result<(), String> {
        let mark = format!(
            "window.__turboAx.markFileInput({uid})",
            uid = js_string(uid)
        );
        interpret_uid_action(uid, &self.eval_in_world(&mark)?)?;
        let _ = call_cdp(&self.webview, "DOM.enable", "{}");
        let doc = call_cdp(
            &self.webview,
            "DOM.getDocument",
            r#"{"depth":-1,"pierce":true}"#,
        )?;
        let doc_val: Value =
            serde_json::from_str(&doc).map_err(|e| format!("DOM.getDocument JSON: {e}"))?;
        let root_id = doc_val
            .pointer("/root/nodeId")
            .and_then(Value::as_u64)
            .ok_or_else(|| "DOM.getDocument missing root.nodeId".to_owned())?;
        let q = serde_json::json!({
            "nodeId": root_id,
            "selector": "[data-turbo-file-target=\"1\"]",
        })
        .to_string();
        let found = call_cdp(&self.webview, "DOM.querySelector", &q)?;
        let found_val: Value =
            serde_json::from_str(&found).map_err(|e| format!("DOM.querySelector JSON: {e}"))?;
        let node_id = found_val
            .get("nodeId")
            .and_then(Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or_else(|| "file input not found in DOM".to_owned())?;
        let set = serde_json::json!({
            "nodeId": node_id,
            "files": [path],
        })
        .to_string();
        let _ = call_cdp(&self.webview, "DOM.setFileInputFiles", &set)?;
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

    /// List regular files in the session-scoped download broker directory.
    pub fn downloads(&self) -> Result<DownloadsResult, String> {
        self.session_folder.as_deref().map_or_else(
            || Ok(DownloadsResult::default()),
            |folder| list_brokered_downloads(folder, &self.active_downloads.borrow()),
        )
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

/// JS that paints the startup card into the current (`about:blank`) document.
///
/// Every node is a plain `div` written through `textContent`, and the card
/// carries `aria-hidden="true"`. That keeps it out of BOTH snapshot paths: the
/// isolated-world collector selects only links, buttons, fields and headings and
/// skips anything under `[aria-hidden=true]`, and the `getFullAXTree` fallback
/// drops `ignored` nodes. A snapshot taken before the first navigate therefore
/// reports zero nodes at `about:blank`, which is the truth.
fn startup_placeholder_js(user_data_dir: &Path) -> String {
    let heading = js_string("Turbo Agent Browser");
    let waiting = js_string("Waiting for the agent's first browser_navigate…");
    let profile = js_string(&format!("profile: {}", user_data_dir.display()));
    let hint = js_string(
        "This window is driven by the agent. Closing it hides it; the host keeps running.",
    );
    let body_style = js_string(
        "margin:0;height:100%;display:flex;align-items:center;justify-content:center;\
         background:#12141a;color:#e6e8ee;font:15px/1.6 'Segoe UI',system-ui,sans-serif",
    );
    let card_style = js_string("text-align:center;max-width:80ch;padding:0 32px");
    let title_style = js_string("font-size:20px;font-weight:600;margin-bottom:12px");
    let path_style = js_string("opacity:.72;word-break:break-all;font-family:Consolas,monospace");
    let hint_style = js_string("opacity:.55;margin-top:12px");

    format!(
        "(function(){{document.title={heading};var b=document.body;if(!b){{return false;}}\
         b.setAttribute('style',{body_style});var c=document.createElement('div');\
         c.setAttribute('aria-hidden','true');c.setAttribute('style',{card_style});\
         var t=document.createElement('div');t.setAttribute('style',{title_style});\
         t.textContent={heading};var w=document.createElement('div');w.textContent={waiting};\
         var p=document.createElement('div');p.setAttribute('style',{path_style});\
         p.textContent={profile};var h=document.createElement('div');\
         h.setAttribute('style',{hint_style});h.textContent={hint};\
         c.appendChild(t);c.appendChild(w);c.appendChild(p);c.appendChild(h);\
         b.appendChild(c);return true;}})()"
    )
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

/// Pump the UI thread for `dur` so a click-driven navigation can commit
/// (or be cancelled) before we read `BlockLog` / `Source`.
fn pump_for(dur: Duration) {
    let deadline = Instant::now() + dur;
    let mut msg = MSG::default();
    while Instant::now() < deadline {
        let slice = (deadline - Instant::now()).as_millis().min(50) as u32;
        unsafe {
            MsgWaitForMultipleObjectsEx(
                None,
                slice,
                QS_ALLINPUT,
                MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(MWMO_INPUTAVAILABLE.0),
            );
        }
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                unsafe { PostQuitMessage(msg.wParam.0 as i32) };
                return;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event handlers: navigation policy, popups, downloads, permissions
// ---------------------------------------------------------------------------

/// Gate **every** top-level *and* iframe navigation on the URL policy.
///
/// Checking only `browser.navigate` made `GROK_BROWSER_ALLOW` a one-hop check:
/// a 302, a meta refresh, or a clicked link walked straight out of it. An
/// allowed page can also iframe public http / off-allowlist https and share
/// the Agent cookie jar, so subframe starts are cancelled the same way.
fn register_navigation_policy(
    webview: &ICoreWebView2,
    session_folder: Option<PathBuf>,
    blocked: Rc<BlockLog>,
) -> Result<(), HostError> {
    let folder_main = session_folder.clone();
    let blocked_main = Rc::clone(&blocked);
    let handler = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut uri = PWSTR::null();
        let url = if unsafe { args.Uri(&mut uri) }.is_err() {
            None
        } else {
            Some(take_pwstr(uri))
        };
        if let Err(err) = check_navigation_hop(url.as_deref(), folder_main.as_deref()) {
            let shown = url.as_deref().unwrap_or("<missing>");
            let message = format!("blocked navigation to {shown}: {err}");
            eprintln!("turbo browser-host: {message}");
            blocked_main.set(message);
            let _ = unsafe { args.SetCancel(true) };
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NavigationStarting(&handler, &mut token) }
        .map_err(|e| HostError::Failed(format!("add_NavigationStarting: {e}")))?;

    let folder_frame = session_folder;
    let blocked_frame = blocked;
    let frame = NavigationStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut uri = PWSTR::null();
        let url = if unsafe { args.Uri(&mut uri) }.is_err() {
            None
        } else {
            Some(take_pwstr(uri))
        };
        if let Err(err) = check_navigation_hop(url.as_deref(), folder_frame.as_deref()) {
            let shown = url.as_deref().unwrap_or("<missing>");
            let message = format!("blocked frame navigation to {shown}: {err}");
            eprintln!("turbo browser-host: {message}");
            blocked_frame.set(message);
            let _ = unsafe { args.SetCancel(true) };
        }
        Ok(())
    }));
    let mut frame_token = 0i64;
    unsafe { webview.add_FrameNavigationStarting(&frame, &mut frame_token) }
        .map_err(|e| HostError::Failed(format!("add_FrameNavigationStarting: {e}")))?;
    Ok(())
}

/// Keep `window.open` in the same view, broker downloads, and deny permissions.
fn register_popup_download_permission(
    webview: &ICoreWebView2,
    environment: ICoreWebView2Environment,
    session_id: &str,
    session_folder: Option<PathBuf>,
    blocked: Rc<BlockLog>,
    active_downloads: Rc<RefCell<HashSet<PathBuf>>>,
) -> Result<(), HostError> {
    // window.open / target=_blank. Unhandled, WebView2 spawns a runtime-owned
    // popup the agent cannot see, drive, or close — and browser.tabs would
    // still report one tab. Redirect it into this view instead.
    let nav_target = webview.clone();
    let popup_blocked = Rc::clone(&blocked);
    let popup_folder = session_folder.clone();
    let popup_downloads = Rc::clone(&active_downloads);
    let popup_session = session_id.to_owned();
    let popup = NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let _ = unsafe { args.SetHandled(true) };
        let mut uri = PWSTR::null();
        let url = if unsafe { args.Uri(&mut uri) }.is_err() {
            None
        } else {
            Some(take_pwstr(uri))
        };
        // The policy check still applies: a popup is a navigation.
        if let Err(err) = check_navigation_hop(url.as_deref(), popup_folder.as_deref()) {
            let shown = url.as_deref().unwrap_or("<missing>");
            let message = format!("blocked popup to {shown}: {err}");
            eprintln!("turbo browser-host: {message}");
            popup_blocked.set(message);
            return Ok(());
        }
        let url = url.unwrap_or_default();
        // GSI / OAuth popups postMessage back to the opener and then close.
        // Navigating them into the only tab leaves a white gsi/select page
        // with no opener. Own a real HWND + CoreWebView2 so later hops still
        // hit NavigationStarting / DownloadStarting.
        if is_oauth_popup_url(&url) {
            if let Err(err) = open_host_owned_oauth_popup(
                &environment,
                &popup_session,
                popup_folder.clone(),
                Rc::clone(&popup_blocked),
                Rc::clone(&popup_downloads),
                &args,
                &url,
            ) {
                let message = format!("oauth popup failed ({err}); cancelled: {url}");
                eprintln!("turbo browser-host: {message}");
                popup_blocked.set(message);
            }
            return Ok(());
        }
        let wide = CoTaskMemPWSTR::from(url.as_str());
        let _ = unsafe { nav_target.Navigate(*wide.as_ref().as_pcwstr()) };
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { webview.add_NewWindowRequested(&popup, &mut token) }
        .map_err(|e| HostError::Failed(format!("add_NewWindowRequested: {e}")))?;

    // Downloads are brokered into the session folder instead of allowing a
    // page to choose an arbitrary path. The browser tool can then retrieve the
    // file with read_file, while the host never writes outside the session root.
    let download_folder = session_folder;
    let download_blocked = Rc::clone(&blocked);
    let download_active = Rc::clone(&active_downloads);
    let download = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
        let Some(args) = args else {
            // Missing COM args: fail closed (do not allow an unbrokered download).
            return Ok(());
        };
        let Some(session_folder) = download_folder.as_ref() else {
            let _ = unsafe { args.SetCancel(true) };
            download_blocked.set("download cancelled: no session folder is configured".to_owned());
            return Ok(());
        };
        let operation = unsafe { args.DownloadOperation() }.ok();
        let suggested = {
            let mut path = PWSTR::null();
            if unsafe { args.ResultFilePath(&mut path) }.is_ok() {
                let raw = take_pwstr(path);
                Path::new(&raw)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
            } else {
                None
            }
        };
        let source_uri = operation.as_ref().and_then(|operation| {
            let mut uri = PWSTR::null();
            unsafe { operation.Uri(&mut uri) }.ok()?;
            Some(take_pwstr(uri))
        });
        if let Err(err) = check_navigation_hop(source_uri.as_deref(), Some(session_folder.as_path()))
        {
            let _ = unsafe { args.SetCancel(true) };
            download_blocked.set(format!("download cancelled: {err}"));
            return Ok(());
        }
        let (partial_destination, final_destination) =
            match broker_attachment(session_folder, suggested.as_deref(), source_uri.as_deref()) {
                Ok(paths) => paths,
                Err(error) => {
                    let _ = unsafe { args.SetCancel(true) };
                    download_blocked.set(format!("download cancelled: {error}"));
                    return Ok(());
                }
            };
        let destination_string = partial_destination.to_string_lossy().into_owned();
        let destination_wide = CoTaskMemPWSTR::from(destination_string.as_str());
        if unsafe { args.SetResultFilePath(*destination_wide.as_ref().as_pcwstr()) }.is_err() {
            let _ = unsafe { args.SetCancel(true) };
            download_blocked.set("download cancelled: WebView2 rejected broker path".to_owned());
            return Ok(());
        }
        download_active
            .borrow_mut()
            .insert(partial_destination.clone());
        if let Some(operation) = operation {
            let active = Rc::clone(&download_active);
            let blocked = Rc::clone(&download_blocked);
            let partial_path = partial_destination.clone();
            let final_path = final_destination.clone();
            let state_changed =
                StateChangedEventHandler::create(Box::new(move |operation, _args| {
                    let Some(operation) = operation else {
                        return Ok(());
                    };
                    let mut state = Default::default();
                    unsafe { operation.State(&mut state) }?;
                    if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                        let finalization = match std::fs::symlink_metadata(&final_path) {
                            Ok(_) => Err(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                "final download path already exists",
                            )),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                std::fs::rename(&partial_path, &final_path)
                            }
                            Err(error) => Err(error),
                        };
                        if let Err(error) = finalization {
                            blocked.set(format!(
                                "brokered download completed but could not finalize file: {error}"
                            ));
                        }
                        active.borrow_mut().remove(&partial_path);
                    } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                        active.borrow_mut().remove(&partial_path);
                        let _ = std::fs::remove_file(&partial_path);
                    }
                    Ok(())
                }));
            let mut state_token = 0i64;
            if let Err(error) =
                unsafe { operation.add_StateChanged(&state_changed, &mut state_token) }
            {
                download_active.borrow_mut().remove(&partial_destination);
                let _ = unsafe { args.SetCancel(true) };
                download_blocked.set(format!(
                    "download cancelled: cannot track brokered download: {error}"
                ));
                return Ok(());
            }
        }
        eprintln!(
            "turbo browser-host: brokered download to {}",
            final_destination.display()
        );
        Ok(())
    }));
    match webview.cast::<ICoreWebView2_4>() {
        Ok(wv4) => {
            let mut token = 0i64;
            unsafe { wv4.add_DownloadStarting(&download, &mut token) }
                .map_err(|e| HostError::Failed(format!("add_DownloadStarting: {e}")))?;
        }
        Err(_) => {
            return Err(HostError::Failed(
                "WebView2 runtime lacks download interception; refusing unsafe browser host startup"
                    .to_owned(),
            ));
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

/// Host-owned OAuth HWND + CoreWebView2. `SetHandled` stays true; the runtime
/// navigates the provided NewWindow so `window.opener` still works. Every hop
/// on that view is gated the same way as the agent tab.
fn open_host_owned_oauth_popup(
    environment: &ICoreWebView2Environment,
    session_id: &str,
    session_folder: Option<PathBuf>,
    blocked: Rc<BlockLog>,
    active_downloads: Rc<RefCell<HashSet<PathBuf>>>,
    args: &ICoreWebView2NewWindowRequestedEventArgs,
    url: &str,
) -> Result<(), HostError> {
    let deferral: ICoreWebView2Deferral = unsafe { args.GetDeferral() }
        .map_err(|e| HostError::Failed(format!("oauth GetDeferral: {e}")))?;
    let complete = || {
        let _ = unsafe { deferral.Complete() };
    };

    let hwnd = match create_oauth_popup_window(session_id, url) {
        Ok(h) => h,
        Err(e) => {
            complete();
            return Err(e);
        }
    };
    let controller = match create_controller(environment, hwnd) {
        Ok(c) => c,
        Err(e) => {
            complete();
            super::window::destroy(hwnd);
            return Err(e);
        }
    };
    if let Err(e) = (|| -> Result<(), HostError> {
        let bounds = client_rect(hwnd);
        unsafe {
            controller
                .SetBounds(bounds)
                .map_err(|e| HostError::Failed(format!("oauth SetBounds: {e}")))?;
            controller
                .SetIsVisible(true)
                .map_err(|e| HostError::Failed(format!("oauth SetIsVisible: {e}")))?;
        }
        let webview = unsafe { controller.CoreWebView2() }
            .map_err(|e| HostError::Failed(format!("oauth CoreWebView2: {e}")))?;
        apply_settings(&webview)?;
        register_navigation_policy(&webview, session_folder.clone(), Rc::clone(&blocked))?;
        register_popup_download_permission(
            &webview,
            environment.clone(),
            session_id,
            session_folder,
            blocked,
            active_downloads,
        )?;
        unsafe { args.SetNewWindow(&webview) }
            .map_err(|e| HostError::Failed(format!("oauth SetNewWindow: {e}")))?;
        attach_controller_ex(hwnd, controller, false);
        show(hwnd);
        Ok(())
    })() {
        complete();
        super::window::destroy(hwnd);
        return Err(e);
    }
    complete();
    eprintln!("turbo browser-host: host-owned oauth popup: {url}");
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
