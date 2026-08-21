//! Open a meeting URL in the OS handler (Teams/Zoom/browser).
//!
//! Windows uses `explorer.exe` (not `cmd /c start`) so `&` / `%VAR%` in a
//! join URL cannot be re-parsed as shell syntax.

/// Fire-and-forget join-link open. The test seam is compiled only for tests.
#[allow(clippy::disallowed_methods)] // detached OS handler; enrolled via detach_std_command
pub fn open_meeting_url(url: &str) -> bool {
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

    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer.exe";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
