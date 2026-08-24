//! Hand the join link to the OS -- and decide when not to.
//!
//! This exists for the *local capture* paths, where the transcript comes from
//! this machine's speakers and the operator therefore has to be in the meeting
//! themselves. When a guest notetaker joins, the link must not be opened at
//! all: the bot navigates it inside its own throwaway profile, and a second
//! OS-level open either wakes the operator's signed-in desktop Teams -- a
//! different identity than the guest we are seating -- or, on Windows, leaves
//! a stray file-manager window behind.

use xai_grok_meetings::CaptureSource;

/// Whether `meeting_join` should hand the link to the OS at all.
///
/// Never on the bot path, and never when capture is disabled: with no
/// transcript and no guest, a stray window would be the only thing the tool
/// did.
pub fn should_shell_open(source: CaptureSource) -> bool {
    !matches!(source, CaptureSource::MeetingBot | CaptureSource::None)
}

/// Open a join link in the OS default handler. `false` means it did not go.
///
/// Windows uses `ShellExecuteW(open)` -- not `explorer.exe`, which spawns a new
/// Explorer window per call and reveals a folder when the URL association does
/// not resolve, and not `cmd /c start`, whose `%VAR%` expansion would re-read
/// a join URL as shell syntax. A link with no handler is reported, never
/// "helpfully" turned into a file-manager window.
pub async fn open_meeting_url(url: &str) -> bool {
    #[cfg(test)]
    {
        if let Ok(path) = std::env::var("GROK_TEST_OPEN_URL_FILE") {
            use std::io::Write;
            if let Err(e) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| writeln!(f, "{url}"))
            {
                tracing::warn!(error = %e, path, "GROK_TEST_OPEN_URL_FILE write failed");
                return false;
            }
            return true;
        }
    }

    let url = url.to_string();
    // ShellExecuteW may delegate to a Shell extension that blocks, and the
    // non-Windows spawn touches the filesystem; neither belongs on a runtime
    // worker thread.
    tokio::task::spawn_blocking(move || open_blocking(&url))
        .await
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn open_blocking(url: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = std::ffi::OsStr::new(url)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();

    // SAFETY: both buffers are NUL-terminated and outlive the call; the
    // remaining pointers are null, which ShellExecuteW documents as
    // "no parameters / use the working directory".
    let code = unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if hr.is_ok() {
            CoUninitialize();
        }
        result.0 as usize
    };

    // ShellExecuteW returns a pseudo-HINSTANCE: >32 is success, anything
    // smaller is an SE_ERR_* code.
    if code > 32 {
        return true;
    }
    // Deliberately no reveal-in-Explorer fallback. A join link that will not
    // open is a thing to *report*; opening a folder instead is what made a
    // failed Teams join look like Turbo rummaging through the operator's
    // Downloads directory.
    tracing::warn!(code, "could not open the join link in the default browser");
    false
}

#[cfg(not(target_os = "windows"))]
#[allow(clippy::disallowed_methods)] // detached OS handler; enrolled via detach_std_command
fn open_blocking(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(target_os = "macos"))]
    let cmd = "xdg-open";

    let mut command = std::process::Command::new(cmd);
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    xai_tty_utils::detach_std_command(&mut command);
    command.spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The File Explorer window the operator saw was this call, fired
    /// unconditionally before the notetaker was even attempted.
    #[test]
    fn the_bot_path_never_shell_opens_the_link() {
        assert!(
            !should_shell_open(CaptureSource::MeetingBot),
            "the notetaker navigates the link itself, in its own profile"
        );
        assert!(
            !should_shell_open(CaptureSource::None),
            "with capture disabled a stray window is the only visible effect"
        );
    }

    /// Local capture records this machine, so the operator does have to be in
    /// the meeting -- opening the link for them is the point.
    #[test]
    fn local_capture_paths_still_open_the_link() {
        assert!(should_shell_open(CaptureSource::Loopback));
        assert!(should_shell_open(CaptureSource::Microphone));
    }
}
