//! HWND for the Agent WebView sidecar (`Turbo Agent Browser`).

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BringWindowToTop, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW,
    DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect, GetForegroundWindow,
    GetWindowLongPtrW, GetWindowThreadProcessId, HWND_NOTOPMOST, IDC_ARROW, IsIconic, IsWindow,
    LoadCursorW, PostQuitMessage, RegisterClassW, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WINDOW_EX_STYLE, WINDOW_LONG_PTR_INDEX, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_SIZE,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::PCWSTR;

use super::HostError;

/// Window title (never “Chrome” / “Edge”).
pub const WINDOW_TITLE: &str = "Turbo Agent Browser";
/// `RegisterClassW` / `CreateWindowExW` class name.
pub const WINDOW_CLASS: &str = "TurboAgentBrowser";
/// Client-area width.
pub const CLIENT_WIDTH: i32 = 1280;
/// Client-area height.
pub const CLIENT_HEIGHT: i32 = 800;

fn wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create a 1280×800 overlapped window that is **not** topmost.
pub fn create_frame_window() -> Result<HWND, HostError> {
    let class_name = wide_nul(WINDOW_CLASS);
    let title = wide_nul(WINDOW_TITLE);
    let instance = unsafe { GetModuleHandleW(None) }
        .ok()
        .map(|h| HINSTANCE(h.0));

    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        // Must match the hInstance passed to CreateWindowExW: classes are keyed
        // by (atom, hInstance), and registering under NULL while creating under
        // the module handle relies on the global-class fallback.
        hInstance: instance.unwrap_or_default(),
        ..Default::default()
    };

    // SAFETY: class lives for the RegisterClassW call.
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        // ERROR_CLASS_ALREADY_EXISTS is expected on a second window in this
        // process; anything else would surface later as a confusing
        // CreateWindowExW failure.
        const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
        let err = windows::core::Error::from_win32();
        if err.code().0 as u32 & 0xFFFF != ERROR_CLASS_ALREADY_EXISTS {
            return Err(HostError::Failed(format!("RegisterClassW: {err}")));
        }
    }

    let mut rect = RECT {
        left: 0,
        top: 0,
        right: CLIENT_WIDTH,
        bottom: CLIENT_HEIGHT,
    };
    // SAFETY: `rect` is a valid RECT out-param.
    unsafe {
        AdjustWindowRectEx(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
        )
        .map_err(|e| HostError::Failed(format!("AdjustWindowRectEx: {e}")))?;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    // SAFETY: class name matches RegisterClassW; no parent / menu; not
    // WS_EX_TOPMOST.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            None,
            None,
            instance,
            None,
        )
    }
    .map_err(|e| HostError::Failed(format!("CreateWindowExW: {e}")))?;

    if hwnd.is_invalid() {
        return Err(HostError::Failed("CreateWindowExW returned NULL".into()));
    }

    // Explicitly not topmost ( overlapped default is already not-topmost ).
    // SAFETY: hwnd is a live window we just created.
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };

    Ok(hwnd)
}

/// Store the WebView2 controller pointer for `WM_SIZE` / `WM_DESTROY`.
pub fn attach_controller(
    hwnd: HWND,
    controller: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
) {
    let state = Box::new(WindowState {
        controller: Some(controller),
    });
    let ptr = Box::into_raw(state);
    // SAFETY: we own `ptr` until WM_DESTROY / detach. UI thread only.
    unsafe {
        set_window_long(hwnd, GWLP_USERDATA, ptr as isize);
    }
}

/// Show the frame (after first `about:blank` paint).
pub fn show(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    // SAFETY: hwnd is the host frame.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
}

/// Bring the frame to the front for `browser.raise`.
///
/// `SW_SHOW` alone leaves a minimized window minimized, and Windows' foreground
/// lock makes a bare `SetForegroundWindow` a silent no-op unless the calling
/// thread already owns the foreground. Restore first, then briefly attach to
/// the current foreground thread's input queue so the request is honored.
pub fn raise(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    // SAFETY: hwnd is the host frame on the UI thread.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let foreground = GetForegroundWindow();
        let self_thread = GetCurrentThreadId();
        let fg_thread = if foreground.is_invalid() {
            self_thread
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let attached =
            fg_thread != self_thread && AttachThreadInput(fg_thread, self_thread, true).as_bool();
        let _ = SetForegroundWindow(hwnd);
        let _ = BringWindowToTop(hwnd);
        if attached {
            let _ = AttachThreadInput(fg_thread, self_thread, false);
        }
    }
}

/// Whether `hwnd` still refers to a live window.
pub fn is_alive(hwnd: HWND) -> bool {
    !hwnd.is_invalid() && unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

/// Destroy the frame (`WM_DESTROY` → `PostQuitMessage`).
pub fn destroy(hwnd: HWND) {
    if hwnd.is_invalid() {
        return;
    }
    // SAFETY: DestroyWindow is idempotent enough; invalid hwnd is skipped.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// Client-area size of `hwnd`.
pub fn client_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    if !hwnd.is_invalid() {
        // SAFETY: `rect` is a valid out-param.
        let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    }
    rect
}

struct WindowState {
    controller: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller>,
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            if let Some(state) = window_state(hwnd)
                && let Some(controller) = state.controller.as_ref()
            {
                let rect = client_rect(hwnd);
                // SAFETY: controller is created for this hwnd; bounds are the
                // current client rect.
                let _ = unsafe { controller.SetBounds(rect) };
            }
            LRESULT::default()
        }
        // Per-monitor DPI aware: Windows hands us the suggested rect when the
        // window moves to a monitor with a different scale. Ignoring it leaves
        // the frame the wrong physical size.
        WM_DPICHANGED => {
            let suggested = l_param.0 as *const RECT;
            if !suggested.is_null() {
                // SAFETY: lParam is a RECT* owned by the system for this message.
                let r = unsafe { *suggested };
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                };
            }
            LRESULT::default()
        }
        WM_CLOSE => {
            destroy(hwnd);
            LRESULT::default()
        }
        WM_DESTROY => {
            if let Some(mut state) = take_window_state(hwnd)
                && let Some(controller) = state.controller.take() {
                    let _ = unsafe { controller.Close() };
                }
            // SAFETY: ends the host message loop.
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT::default()
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) },
    }
}

fn window_state(hwnd: HWND) -> Option<&'static mut WindowState> {
    // SAFETY: pointer was Box::into_raw in attach_controller; only the UI
    // thread reads it. Returned reference is valid until take_window_state.
    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

fn take_window_state(hwnd: HWND) -> Option<Box<WindowState>> {
    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if ptr.is_null() {
        return None;
    }
    unsafe {
        set_window_long(hwnd, GWLP_USERDATA, 0);
        Some(Box::from_raw(ptr))
    }
}

#[cfg(target_pointer_width = "64")]
unsafe fn set_window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    unsafe { SetWindowLongPtrW(hwnd, index, value) }
}

#[cfg(target_pointer_width = "32")]
unsafe fn set_window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> isize {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongW(hwnd, index, value as i32) as isize
    }
}

#[cfg(target_pointer_width = "64")]
unsafe fn get_window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX) -> isize {
    unsafe { GetWindowLongPtrW(hwnd, index) }
}

#[cfg(target_pointer_width = "32")]
unsafe fn get_window_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX) -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongW(hwnd, index) as isize }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_identity_is_turbo_agent_browser() {
        assert_eq!(WINDOW_TITLE, "Turbo Agent Browser");
        assert_eq!(WINDOW_CLASS, "TurboAgentBrowser");
        assert!(!WINDOW_TITLE.contains("Chrome"));
        assert!(!WINDOW_TITLE.contains("Edge"));
        assert_eq!(CLIENT_WIDTH, 1280);
        assert_eq!(CLIENT_HEIGHT, 800);
        let class_w = wide_nul(WINDOW_CLASS);
        let title_w = wide_nul(WINDOW_TITLE);
        assert_eq!(class_w.last().copied(), Some(0));
        assert_eq!(title_w.last().copied(), Some(0));
        assert_eq!(
            String::from_utf16_lossy(&class_w[..class_w.len() - 1]),
            WINDOW_CLASS
        );
        assert_eq!(
            String::from_utf16_lossy(&title_w[..title_w.len() - 1]),
            WINDOW_TITLE
        );
    }
}
