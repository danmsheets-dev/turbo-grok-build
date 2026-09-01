//! Shared URL-opening and scheme validation utilities.
//!
//! Extracted from the `OpenSupergrokUrl` dispatch handler so that any
//! code path (keyboard navigation, mouse click, action dispatch) can
//! open a link safely without duplicating platform-specific logic.

use std::collections::HashMap;

use crate::terminal::hyperlinks::SchemeFilter;

/// Outcome of attempting to open a URL in the system browser/handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenUrlResult {
    /// Opener was launched (or the test seam recorded the URL).
    Opened,
    /// Scheme was rejected by the safety filter.
    RejectedScheme,
    /// Browser cannot run here (headless / no display) or the opener
    /// failed to spawn. Callers should surface the URL for manual open.
    BrowserUnavailable,
}

/// Whether the environment looks capable of opening a GUI browser.
///
/// Pure helper for tests. On Linux/BSD, requires a non-empty `DISPLAY` or
/// `WAYLAND_DISPLAY` (or a non-empty `BROWSER` override). macOS/Windows
/// are treated as available at the env level (spawn failure is still
/// reported by [`open_url`]).
pub fn browser_open_likely_available_from_env(env: &HashMap<String, String>) -> bool {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    // Explicit BROWSER override: allow even without a display server so
    // scripted/headless setups that point at a CLI browser still try.
    if env.get("BROWSER").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    env.get("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || env.get("DISPLAY").is_some_and(|v| !v.is_empty())
}

/// Whether this process likely has a GUI browser available right now.
pub fn browser_open_likely_available() -> bool {
    let env = crate::host::collect_unicode_env();
    browser_open_likely_available_from_env(&env)
}

const BROWSER_UNAVAILABLE_NOTICE: &str = "Could not open a browser. Open this URL manually";

/// How long [`open_url`] waits for the Windows opener before assuming success.
/// Bounded because the caller is an event-loop input handler.
#[cfg(target_os = "windows")]
const OPEN_URL_RESULT_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// Multi-line copy for agent scrollback: notice, then the full URL alone
/// so it is easy to select/copy in the TUI.
pub fn browser_unavailable_message(url: &str) -> String {
    format!("{BROWSER_UNAVAILABLE_NOTICE}:\n{url}")
}

/// Single-line welcome toast: URL first so prefix truncation keeps the
/// destination. `copied` is true only when clipboard delivery reported
/// success — never claim a copy that did not happen.
pub fn browser_unavailable_line(url: &str, copied: bool) -> String {
    if copied {
        format!("{url} — {BROWSER_UNAVAILABLE_NOTICE} (URL copied)")
    } else {
        format!("{url} — {BROWSER_UNAVAILABLE_NOTICE}")
    }
}

/// Open a URL in the system's default browser/handler.
///
/// Spawns the platform-native opener (`open` on macOS, `xdg-open` on
/// Linux, `ShellExecuteW` on Windows) with fully detached stdio so it
/// cannot block the pager.
///
/// Returns `true` when the opener was launched (or the test seam recorded
/// the URL). Returns `false` when the environment looks headless or spawn
/// fails — callers should surface the URL via [`browser_unavailable_message`]
/// (scrollback) or [`browser_unavailable_line`] (welcome toast).
///
/// **Callers handling untrusted input** should call [`is_safe_to_open`]
/// first, or use [`open_url_if_safe`] / [`try_open_url`] which combine both.
pub fn open_url(url: &str) -> bool {
    // Test seam: PTY e2e must observe the open without launching a real
    // browser. When set, append the URL to the file and skip the OS opener.
    if let Ok(path) = std::env::var("GROK_TEST_OPEN_URL_FILE") {
        use std::io::Write;
        // Surface misconfiguration: a swallowed write leaves the PTY test
        // failing with a generic timeout and no clue why.
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

    // Skip the doomed spawn on headless Linux VMs (no DISPLAY / Wayland)
    // so billing Upgrade / Buy-credits clicks can fall back to showing the
    // URL instead of silently no-op'ing.
    if !browser_open_likely_available() {
        tracing::info!("skipping browser open: no display server / BROWSER");
        return false;
    }

    let opened = spawn_url_opener(url);
    if !opened {
        // Redact URL to avoid leaking sensitive query params to logs.
        let redacted = url::Url::parse(url)
            .map(|mut u| {
                u.set_query(None);
                u.set_fragment(None);
                u.to_string()
            })
            .unwrap_or_else(|_| "<unparseable>".to_string());
        tracing::warn!(url = %redacted, "failed to open URL");
    }
    opened
}

/// Hand the URL to the default browser via `ShellExecuteW`, which takes it
/// as a single argument. The URL must never pass through `cmd.exe`: `start`
/// splits on `&` and expands `%VAR%`, so a server-supplied URL such as
/// `https://example.com/&calc.exe` would execute a command.
///
/// Runs on a helper thread like [`shell_open_detached`]: `ShellExecuteW` may
/// delegate to a Shell extension that expects an initialized apartment, and the
/// TUI thread must not be COM-initialized.
///
/// The caller is an input handler on the event loop, so this waits only
/// [`OPEN_URL_RESULT_WAIT`] for the outcome instead of joining. A failing
/// association reports back almost immediately, which is the case worth
/// catching; a slower call means the Shell accepted the request and is still
/// working, so we report success and let the detached thread finish rather than
/// stall rendering.
#[cfg(target_os = "windows")]
fn spawn_url_opener(url: &str) -> bool {
    let wide: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
        };
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::PCWSTR;

        let verb: Vec<u16> = "open\0".encode_utf16().collect();

        // SAFETY: both buffers are NUL-terminated and outlive the call; the
        // remaining pointers are null, which ShellExecuteW documents as
        // "no parameters / use the working directory". The returned
        // pseudo-HINSTANCE is only compared (>32 means success), never
        // dereferenced.
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
        // The receiver is gone once the wait below times out; the send failing
        // is expected and means nobody is listening any more.
        let _ = tx.send(code > 32);
    });
    // Timeout means "still working", not "failed": reporting failure here would
    // show the manual-open fallback for a link that is about to open fine.
    rx.recv_timeout(OPEN_URL_RESULT_WAIT).unwrap_or(true)
}

#[cfg(not(target_os = "windows"))]
#[allow(clippy::disallowed_methods)] // fire and forget; the child is reaped when this process exits
fn spawn_url_opener(url: &str) -> bool {
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
    xai_grok_tools::util::detach_std_command(&mut command);
    match command.spawn() {
        Ok(_) => true,
        Err(e) => {
            tracing::debug!(error = %e, "URL opener failed to spawn");
            false
        }
    }
}

/// Build the `open`/`xdg-open` opener command (macOS / Linux / BSD).
///
/// The returned command is TTY-guarded via [`xai_tty_utils::detach_std_command`]
/// (`setsid`/`setpgid`) so the spawned GUI helper and its children can't grab
/// the TUI's `/dev/tty`, with stdio fully redirected to null. Split from
/// [`open_path`] so it can be unit-tested without spawning. The path is a single
/// argument, never interpolated into a shell string. Windows uses
/// [`reveal_in_explorer`] instead.
#[cfg(not(target_os = "windows"))]
fn build_open_path_command(path: &std::path::Path) -> std::process::Command {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(target_os = "macos"))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    xai_tty_utils::detach_std_command(&mut command);
    command
}

/// What [`open_path`] should hand to the OS for `path`.
///
/// A missing file falls back to its parent directory so the user lands near
/// the media instead of in Home. `None` means there is nothing openable and
/// the caller should report failure rather than spawn a stray window.
///
/// Windows-only: `open`/`xdg-open` already fail cleanly on a missing path, so
/// the other platforms keep their existing behaviour.
#[cfg(any(test, target_os = "windows"))]
fn resolve_open_target(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    if path.is_file() || path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent()
        .filter(|p| p.is_dir())
        .map(std::path::Path::to_path_buf)
}

/// Open a local file in its default application.
///
/// Returns `true` when the open was dispatched. Takes a trusted filesystem
/// path (no scheme validation, unlike [`open_url`]).
///
/// - **Windows**: `ShellExecuteW` with the `open` verb. This is *not*
///   `cmd /c start`, whose `%VAR%` expansion corrupts the percent-encoded
///   session-directory segment in imagine media paths (e.g.
///   `…\C%3A%5CUsers…`), and *not* `explorer.exe`, which spawns a new
///   Explorer window per call — those accumulate across a working session.
///   A file with no registered association falls back to
///   [`reveal_in_file_manager`] rather than raising Windows' "How do you want
///   to open this file?" chooser.
/// - **macOS / Linux**: `open` / `xdg-open` open the file in its default app.
///
/// Use [`reveal_in_file_manager`] when the intent is to *show* the file in the
/// file manager rather than open it.
#[allow(clippy::disallowed_methods)] // fire and forget; the child is reaped when this process exits
pub fn open_path(path: &std::path::Path) -> bool {
    // Never launch a real GUI app in tests.
    #[cfg(test)]
    {
        !path.as_os_str().is_empty()
    }
    #[cfg(all(not(test), target_os = "windows"))]
    {
        match resolve_open_target(path) {
            Some(target) => {
                shell_open_detached(target);
                true
            }
            None => {
                tracing::warn!(path = %path.display(), "nothing to open: path and parent are both missing");
                false
            }
        }
    }
    #[cfg(all(not(test), not(target_os = "windows")))]
    {
        match build_open_path_command(path).spawn() {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to open file natively");
                false
            }
        }
    }
}

/// `ShellExecuteW(open)` on a short-lived thread.
///
/// Off-thread for two reasons: `ShellExecuteW` may delegate to a Shell
/// extension that expects an initialized apartment (so the thread calls
/// `CoInitializeEx` itself, without disturbing the TUI's thread), and a
/// handler that blocks must not stall rendering.
#[cfg(all(not(test), target_os = "windows"))]
fn shell_open_detached(target: std::path::PathBuf) {
    use std::os::windows::ffi::OsStrExt;

    std::thread::spawn(move || {
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
        };
        use windows::Win32::UI::Shell::{SE_ERR_NOASSOC, ShellExecuteW};
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::PCWSTR;

        let wide: Vec<u16> = target
            .as_os_str()
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

        // ShellExecuteW returns a pseudo-HINSTANCE: >32 is success, and any
        // smaller value is an SE_ERR_* code.
        if code > 32 {
            return;
        }
        if code == SE_ERR_NOASSOC as usize {
            // No default app for this extension (common for source files).
            // Showing it in Explorer beats the "How do you want to open this
            // file?" chooser, and matches what this used to do for everything.
            tracing::debug!(path = %target.display(), "no file association; revealing instead");
            reveal_in_file_manager(&target);
            return;
        }
        tracing::warn!(path = %target.display(), code, "ShellExecuteW failed to open path");
    });
}

/// Reveal `path` in a new Explorer window with the file selected.
///
/// **Every call opens another Explorer window** — Windows offers no way to
/// reuse one — so this is for an explicit "show me where this is" action, not
/// for opening a file. [`open_path`] is the opener; it only lands here when a
/// file has no registered application.
///
/// Uses `raw_arg` so Explorer's required `/select,"<path>"` quoting is passed
/// verbatim — the default arg quoting wraps the whole token and breaks the
/// switch. Launched directly (not via `cmd`), so percent characters in the
/// path are not expanded by the shell. Session dirs embed a urlencoded cwd
/// segment (`C%3A%5CUsers…`); those `%` chars must reach Explorer intact.
///
/// Prefer the on-disk path as-is. When the file is missing, open the parent
/// folder (no `/select`) so the user lands near the media instead of Home.
#[cfg(all(not(test), target_os = "windows"))]
#[allow(clippy::disallowed_methods)] // fire and forget; the child is reaped when this process exits
pub fn reveal_in_file_manager(path: &std::path::Path) -> bool {
    use std::os::windows::process::CommandExt;

    // Prefer the real on-disk location (absolute). Fall back to parent when
    // the file was deleted so Explorer does not dump the user in Home.
    let target = if path.is_file() || path.is_dir() {
        path.to_path_buf()
    } else if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
        tracing::debug!(
            path = %path.display(),
            parent = %parent.display(),
            "media path missing; opening parent folder in Explorer"
        );
        parent.to_path_buf()
    } else {
        path.to_path_buf()
    };

    let select_file = target.is_file();
    let mut command = std::process::Command::new("explorer");
    // Escape embedded double-quotes in the path so the `/select,"<path>"`
    // quoting does not break. Windows file-system paths cannot legally contain
    // `"`, but percent-decoded display paths or future user-chosen filenames
    // could, so be defensive.
    let escaped = target.display().to_string().replace('"', "\"\"");
    if select_file {
        command.raw_arg(format!("/select,\"{}\"", escaped));
    } else {
        // Open the folder itself (no /select) — works for dirs and as a
        // fallback when we only have a parent path.
        command.raw_arg(format!("\"{}\"", escaped));
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    xai_tty_utils::detach_std_command(&mut command);
    // explorer.exe returns exit code 1 even on success, so a successful spawn
    // is the best signal we have.
    match command.spawn() {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(path = %target.display(), error = %e, "failed to reveal file in Explorer");
            false
        }
    }
}

/// Check if a URL's scheme is safe to open.
///
/// Uses the `url` crate for robust scheme extraction. Falls back to
/// prefix matching for non-standard URLs that `url::Url::parse` rejects.
pub fn is_safe_to_open(url: &str, filter: SchemeFilter) -> bool {
    let url = url.trim();
    if let Ok(parsed) = url::Url::parse(url) {
        return filter.allows(parsed.scheme());
    }
    // Fallback: check for scheme via "://" prefix, lowercasing for
    // case-insensitive comparison (SchemeFilter matches lowercase literals).
    if let Some((scheme, _)) = url.split_once("://") {
        return filter.allows(&scheme.to_ascii_lowercase());
    }
    // Defensive: url::Url::parse handles well-formed mailto, but guard
    // against edge cases where the parser rejects a mailto-like string.
    if let Some((scheme, _)) = url.split_once(':')
        && scheme.eq_ignore_ascii_case("mailto")
    {
        return filter.allows(&scheme.to_ascii_lowercase());
    }
    false
}

/// Validate scheme and open a URL if permitted.
///
/// Returns `true` only when the scheme is allowed **and** the opener was
/// launched. Distinguishes scheme rejection from browser unavailability
/// via [`try_open_url`].
pub fn open_url_if_safe(url: &str, filter: SchemeFilter) -> bool {
    matches!(try_open_url(url, filter), OpenUrlResult::Opened)
}

/// Validate scheme and attempt to open. Prefer this when the caller needs
/// to show a manual-URL fallback on [`OpenUrlResult::BrowserUnavailable`].
pub fn try_open_url(url: &str, filter: SchemeFilter) -> OpenUrlResult {
    if !is_safe_to_open(url, filter) {
        tracing::debug!(url, "URL scheme not permitted");
        return OpenUrlResult::RejectedScheme;
    }
    if open_url(url) {
        OpenUrlResult::Opened
    } else {
        OpenUrlResult::BrowserUnavailable
    }
}

/// Ensure `url` carries the given query parameter, returning the rewritten URL.
///
/// If the URL already contains a parameter with that name, its value is left
/// untouched (the caller upstream may have intentionally set one). On parse
/// failure, the original string is returned unchanged so this is safe to apply
/// to opener input from untrusted sources.
///
/// Used by the SuperGrok upsell flow to attribute clicks to `referrer=grok-build`,
/// matching the OAuth consent screen and x.ai/cli marketing links regardless of
/// what the remote settings `gate_url` value happens to be.
pub fn ensure_query_param(url: &str, key: &str, value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let already_present = parsed.query_pairs().any(|(k, _)| k == key);
    if already_present {
        return parsed.to_string();
    }
    parsed.query_pairs_mut().append_pair(key, value);
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_target_is_the_file_itself_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(resolve_open_target(&file).as_deref(), Some(file.as_path()));
    }

    #[test]
    fn open_target_is_the_directory_itself() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_open_target(dir.path()).as_deref(),
            Some(dir.path())
        );
    }

    /// A deleted or moved file lands the user in its folder rather than Home.
    /// This is the case that produced the stray `…/research` window.
    #[test]
    fn missing_file_falls_back_to_its_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("deleted.png");
        assert_eq!(
            resolve_open_target(&gone).as_deref(),
            Some(dir.path()),
            "a missing file should open its folder"
        );
    }

    /// Nothing openable must report failure, not spawn a window at Home.
    #[test]
    fn unopenable_paths_resolve_to_nothing() {
        assert_eq!(resolve_open_target(std::path::Path::new("")), None);
        let nowhere = std::path::Path::new("/no/such/dir/anywhere/at/all/x.png");
        assert_eq!(resolve_open_target(nowhere), None);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn open_path_command_passes_path_as_a_single_arg() {
        // Path with spaces must be one argument, never shell-interpolated.
        // Windows uses reveal_in_explorer instead of this helper.
        let path = std::path::Path::new("/tmp/grok session/image 1.jpg");
        let command = build_open_path_command(path);
        let args: Vec<_> = command.get_args().map(|a| a.to_os_string()).collect();
        assert!(args.contains(&path.as_os_str().to_os_string()));
    }

    #[test]
    fn standard_http_schemes_allowed() {
        assert!(is_safe_to_open(
            "http://example.com",
            SchemeFilter::Standard
        ));
        assert!(is_safe_to_open(
            "https://example.com/path?q=1",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn mailto_allowed() {
        assert!(is_safe_to_open(
            "mailto:user@example.com",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn file_scheme_blocked_by_standard() {
        // file:// removed from Standard to prevent local file / SSRF attacks.
        assert!(!is_safe_to_open(
            "file:///home/user/doc.pdf",
            SchemeFilter::Standard
        ));
        // But allowed under EditorExtended.
        assert!(is_safe_to_open(
            "file:///home/user/doc.pdf",
            SchemeFilter::EditorExtended
        ));
    }

    #[test]
    fn javascript_scheme_blocked() {
        assert!(!is_safe_to_open(
            "javascript:alert(1)",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn data_scheme_blocked() {
        assert!(!is_safe_to_open(
            "data:text/html,<h1>hi</h1>",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn empty_and_garbage_rejected() {
        assert!(!is_safe_to_open("", SchemeFilter::Standard));
        assert!(!is_safe_to_open("not-a-url", SchemeFilter::Standard));
        assert!(!is_safe_to_open(
            "://missing-scheme",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn editor_schemes_with_extended_filter() {
        assert!(is_safe_to_open(
            "vscode://file/path",
            SchemeFilter::EditorExtended
        ));
        assert!(is_safe_to_open(
            "cursor://open",
            SchemeFilter::EditorExtended
        ));
        assert!(is_safe_to_open("idea://open", SchemeFilter::EditorExtended));
        assert!(is_safe_to_open("zed://open", SchemeFilter::EditorExtended));
    }

    #[test]
    fn editor_schemes_blocked_by_standard_filter() {
        assert!(!is_safe_to_open(
            "vscode://file/path",
            SchemeFilter::Standard
        ));
        assert!(!is_safe_to_open("cursor://open", SchemeFilter::Standard));
    }

    #[test]
    fn scheme_case_sensitivity() {
        // url::Url normalizes to lowercase
        assert!(is_safe_to_open(
            "HTTP://EXAMPLE.COM",
            SchemeFilter::Standard
        ));
        assert!(is_safe_to_open(
            "HTTPS://EXAMPLE.COM",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn url_with_fragment_and_query() {
        assert!(is_safe_to_open(
            "https://example.com/page?key=val#section",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn ftp_scheme_blocked() {
        assert!(!is_safe_to_open(
            "ftp://files.example.com/pub",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn fallback_colon_slash_slash_path() {
        // A custom scheme that url::Url may reject but has ://
        assert!(!is_safe_to_open(
            "custom://something",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn non_mailto_colon_without_slashes_rejected() {
        assert!(!is_safe_to_open("tel:+1234567890", SchemeFilter::Standard));
    }

    #[test]
    fn whitespace_trimmed_before_parse() {
        assert!(is_safe_to_open(
            "  https://example.com  ",
            SchemeFilter::Standard
        ));
        assert!(is_safe_to_open(
            "\thttps://example.com\n",
            SchemeFilter::Standard
        ));
    }

    #[test]
    fn ensure_query_param_appends_when_missing() {
        let out = ensure_query_param("https://grok.com/supergrok", "referrer", "grok-build");
        assert_eq!(out, "https://grok.com/supergrok?referrer=grok-build");
    }

    #[test]
    fn ensure_query_param_preserves_existing_value() {
        let out = ensure_query_param(
            "https://grok.com/supergrok?referrer=other",
            "referrer",
            "grok-build",
        );
        assert_eq!(out, "https://grok.com/supergrok?referrer=other");
    }

    #[test]
    fn ensure_query_param_keeps_other_query_pairs() {
        let out = ensure_query_param(
            "https://grok.com/supergrok?heavy=1",
            "referrer",
            "grok-build",
        );
        assert_eq!(
            out,
            "https://grok.com/supergrok?heavy=1&referrer=grok-build"
        );
    }

    #[test]
    fn ensure_query_param_preserves_fragment() {
        // The current remote settings value uses a hash fragment for client-side
        // routing (`grok.com/#supergrok`); we still want the referrer attached.
        let out = ensure_query_param("https://grok.com/#supergrok", "referrer", "grok-build");
        assert_eq!(out, "https://grok.com/?referrer=grok-build#supergrok");
    }

    #[test]
    fn ensure_query_param_returns_unchanged_on_parse_failure() {
        let out = ensure_query_param("not a url", "referrer", "grok-build");
        assert_eq!(out, "not a url");
    }

    #[test]
    fn ensure_query_param_url_encodes_value() {
        let out = ensure_query_param("https://grok.com/supergrok", "referrer", "grok build");
        assert_eq!(out, "https://grok.com/supergrok?referrer=grok+build");
    }

    #[test]
    fn fallback_scheme_case_insensitive() {
        // Uppercase scheme that url::Url::parse rejects triggers fallback path;
        // the fallback must lowercase before matching SchemeFilter.
        assert!(!is_safe_to_open(
            "CUSTOM://something",
            SchemeFilter::Standard
        ));
        // Ensure mailto fallback is case-insensitive too.
        assert!(is_safe_to_open(
            "MAILTO:user@example.com",
            SchemeFilter::Standard
        ));
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn browser_available_with_x11_display() {
        assert!(browser_open_likely_available_from_env(&env(&[(
            "DISPLAY", ":0"
        )])));
    }

    #[test]
    fn browser_available_with_wayland() {
        assert!(browser_open_likely_available_from_env(&env(&[(
            "WAYLAND_DISPLAY",
            "wayland-0"
        )])));
    }

    #[test]
    fn browser_available_with_browser_env_override() {
        // Headless boxes can still open via BROWSER=… even without DISPLAY.
        assert!(browser_open_likely_available_from_env(&env(&[(
            "BROWSER", "firefox"
        )])));
    }

    #[test]
    fn browser_unavailable_when_display_vars_empty_or_missing() {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            // Desktop OSes do not gate on DISPLAY.
            assert!(browser_open_likely_available_from_env(&env(&[])));
            return;
        }
        assert!(!browser_open_likely_available_from_env(&env(&[])));
        assert!(!browser_open_likely_available_from_env(&env(&[
            ("DISPLAY", ""),
            ("WAYLAND_DISPLAY", ""),
            ("BROWSER", ""),
        ])));
    }

    #[test]
    fn browser_unavailable_message_includes_full_url() {
        let url = "https://grok.com/supergrok?referrer=grok-build";
        assert_eq!(
            browser_unavailable_message(url),
            format!("{BROWSER_UNAVAILABLE_NOTICE}:\n{url}")
        );
    }

    #[test]
    fn browser_unavailable_line_is_url_first_single_line() {
        let url = "https://grok.com/supergrok?referrer=grok-build";
        let plain = browser_unavailable_line(url, false);
        assert!(plain.starts_with(url), "{plain}");
        assert!(!plain.contains('\n'), "{plain}");
        assert!(
            !plain.to_ascii_lowercase().contains("copied"),
            "must not claim copy on failure: {plain}"
        );
        assert!(
            plain.contains(BROWSER_UNAVAILABLE_NOTICE),
            "shares notice stem with multi-line form: {plain}"
        );

        let with_copy = browser_unavailable_line(url, true);
        assert!(with_copy.starts_with(url), "{with_copy}");
        assert!(!with_copy.contains('\n'), "{with_copy}");
        assert!(
            with_copy.contains("URL copied"),
            "copy claim only when copied=true: {with_copy}"
        );
    }

    #[test]
    fn try_open_url_rejects_unsafe_scheme_without_opening() {
        assert_eq!(
            try_open_url("javascript:alert(1)", SchemeFilter::Standard),
            OpenUrlResult::RejectedScheme
        );
    }
}
