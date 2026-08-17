//! WebView2 environment, controller, navigate, and CDP screenshot.

use std::path::Path;
use std::sync::mpsc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    CreateCoreWebView2EnvironmentWithOptions, GetAvailableCoreWebView2BrowserVersionString,
    ICoreWebView2, ICoreWebView2Controller, ICoreWebView2Environment,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationCompletedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND};
use windows::Win32::UI::WindowsAndMessaging::PostQuitMessage;
use windows::core::{BOOL, PCWSTR, PWSTR};

use super::window::{attach_controller, client_rect};
use super::{HostError, next_screenshot_path, screenshot_dir};
use crate::protocol::{NavigateResult, ScreenshotResult, TabInfo, TabsResult};

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

/// Single-tab WebView2 controller owned by the UI thread.
pub struct AgentWebView {
    hwnd: HWND,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    session_id: String,
    screenshot_n: u32,
}

impl AgentWebView {
    /// Create the environment (user-data-dir = profile) and controller.
    pub fn create(hwnd: HWND, user_data_dir: &Path, session_id: &str) -> Result<Self, HostError> {
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
        attach_controller(hwnd, controller.clone());

        let mut host = Self {
            hwnd,
            controller,
            webview,
            session_id: session_id.to_owned(),
            screenshot_n: 0,
        };
        // First paint: about:blank until an agent navigate.
        host.navigate("about:blank").map_err(HostError::Failed)?;
        Ok(host)
    }

    /// `ICoreWebView2::Navigate` and wait for `NavigationCompleted`.
    pub fn navigate(&mut self, url: &str) -> Result<NavigateResult, String> {
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
        // Always detach, including TaskCanceled / Navigate failure.
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

        // `wait_with_pump` uses GetMessageA and **consumes** WM_QUIT
        // (`TaskCanceled`). Re-post so the outer host loop still exits.
        let success = match webview2_com::wait_with_pump(rx) {
            Ok(ok) => ok,
            Err(e) => {
                repost_quit_if_task_canceled(&e);
                return Err(format!("navigate wait: {e}"));
            }
        };
        if !success {
            return Err(format!("navigation failed: {url}"));
        }
        self.location()
    }

    /// CDP `Page.captureScreenshot` → PNG file + IHDR size.
    pub fn screenshot(&mut self) -> Result<ScreenshotResult, String> {
        let json = call_cdp(
            &self.webview,
            "Page.captureScreenshot",
            r#"{"format":"png"}"#,
        )?;
        let (png, width, height) = super::decode_cdp_png(&json)?;
        self.screenshot_n = self.screenshot_n.saturating_add(1);
        let dir = screenshot_dir(&self.session_id);
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
    let method_owned = method.to_owned();
    let params_owned = params_json.to_owned();
    let webview = webview.clone();

    CallDevToolsProtocolMethodCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            let method = CoTaskMemPWSTR::from(method_owned.as_str());
            let params = CoTaskMemPWSTR::from(params_owned.as_str());
            unsafe {
                webview
                    .CallDevToolsProtocolMethod(
                        *method.as_ref().as_pcwstr(),
                        *params.as_ref().as_pcwstr(),
                        &handler,
                    )
                    .map_err(webview2_com::Error::WindowsError)
            }
        }),
        Box::new(move |error_code, result_json| {
            error_code?;
            let _ = tx.send(result_json);
            Ok(())
        }),
    )
    .map_err(|e| {
        // Nested wait_with_pump inside wait_for_async_operation eats WM_QUIT.
        repost_quit_if_task_canceled(&e);
        format!("CDP {method}: {e}")
    })?;

    rx.recv()
        .map_err(|_| format!("CDP {method}: channel closed"))
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

/// `wait_with_pump` / `wait_for_async_operation` return this after
/// `GetMessage` sees `WM_QUIT` (and consumes it). The outer host loop
/// will hang unless we re-post quit.
pub(crate) fn pump_consumed_quit(err: &webview2_com::Error) -> bool {
    matches!(err, webview2_com::Error::TaskCanceled)
}

fn repost_quit_if_task_canceled(err: &webview2_com::Error) {
    if pump_consumed_quit(err) {
        // SAFETY: UI thread; re-queues WM_QUIT for the outer GetMessageW loop.
        unsafe {
            PostQuitMessage(0);
        }
    }
}

fn map_webview_err(err: webview2_com::Error) -> HostError {
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
    let msg = err.message();
    let lower = msg.to_ascii_lowercase();
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
}
