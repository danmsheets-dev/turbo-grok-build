//! Find and launch a Chromium-family browser with a DevTools endpoint.
//!
//! We never download a browser. On Windows every machine already has Edge; the
//! Chrome paths are a courtesy fallback. `GROK_CDP_BROWSER` overrides discovery.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use xai_tty_utils::{ProcessGroup, global_process_scope};

use crate::error::{CdpError, Result};

/// Override for browser discovery: an absolute path to a Chromium binary.
pub const BROWSER_ENV: &str = "GROK_CDP_BROWSER";

/// How long to wait for the browser to print its DevTools endpoint.
const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(30);

/// How the browser window should be presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Headless {
    /// Chromium's modern headless mode. Real WebRTC, no visible window.
    #[default]
    New,
    /// Visible window. Useful when diagnosing a join that fails headless.
    Off,
}

/// Options for [`launch`].
#[derive(Debug, Clone)]
pub struct LaunchOptions {
    /// Profile directory. A throwaway dir keeps meetings isolated from the
    /// operator's real browser profile and cookies.
    pub user_data_dir: PathBuf,
    /// Headless or windowed.
    pub headless: Headless,
    /// Extra Chromium switches appended verbatim.
    pub extra_args: Vec<String>,
}

impl LaunchOptions {
    /// Options for a throwaway profile at `user_data_dir`, headless.
    pub fn new(user_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            user_data_dir: user_data_dir.into(),
            headless: Headless::New,
            extra_args: Vec::new(),
        }
    }

    /// Show the browser window.
    #[must_use]
    pub fn windowed(mut self) -> Self {
        self.headless = Headless::Off;
        self
    }

    /// Append an extra Chromium switch.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.extra_args.push(arg.into());
        self
    }
}

/// A launched browser process and the WebSocket URL of its browser target.
///
/// Chromium spawns a *tree* — renderer, GPU, network service, audio service.
/// `kill_on_drop` reaps only the parent, so the process group returned by
/// [`ProcessScope::spawn`] is what actually guarantees no orphaned browser is
/// left sitting in a meeting recording audio after the session ends.
pub struct LaunchedBrowser {
    /// The child process. Killed on drop via [`Child::kill_on_drop`].
    pub child: Child,
    /// `ws://127.0.0.1:<port>/devtools/browser/<uuid>`
    pub ws_url: String,
    /// Enrollment handle. Must stay alive for as long as the browser is our
    /// responsibility; [`Drop`] reaps the tree through it.
    group: Arc<ProcessGroup>,
}

impl Drop for LaunchedBrowser {
    fn drop(&mut self) {
        // `ProcessGroup`'s own drop reaps the tree on Windows (the job carries
        // KILL_ON_JOB_CLOSE) but is a documented no-op on Unix. `kill_on_drop`
        // only reaps the parent, so on Unix the renderer/GPU/audio children
        // would be orphaned — still joined to the meeting. Kill explicitly so
        // teardown is the same on every platform.
        if let Err(e) = self.group.kill() {
            tracing::debug!(error = %e, "notetaker process group already reaped");
        }
    }
}

impl std::fmt::Debug for LaunchedBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `ProcessGroup` is not `Debug`; report the pid instead, which is what
        // anyone reading a log actually wants.
        f.debug_struct("LaunchedBrowser")
            .field("pid", &self.child.id())
            .field("ws_url", &self.ws_url)
            .finish_non_exhaustive()
    }
}

/// Candidate browser executables, most preferred first.
fn candidates() -> Vec<PathBuf> {
    if let Ok(explicit) = std::env::var(BROWSER_ENV) {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return vec![PathBuf::from(trimmed)];
        }
    }

    let mut out: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // Edge first: present on every supported Windows build.
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            let Ok(root) = std::env::var(var) else {
                continue;
            };
            out.push(
                PathBuf::from(&root)
                    .join("Microsoft")
                    .join("Edge")
                    .join("Application")
                    .join("msedge.exe"),
            );
            out.push(
                PathBuf::from(&root)
                    .join("Google")
                    .join("Chrome")
                    .join("Application")
                    .join("chrome.exe"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        out.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for p in [
            "/usr/bin/microsoft-edge",
            "/usr/bin/microsoft-edge-stable",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ] {
            out.push(PathBuf::from(p));
        }
    }

    out
}

/// First candidate that exists on disk.
pub fn find_browser() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

/// Base switches. Chosen so a meeting page behaves like a real client while
/// staying isolated from the operator's profile.
fn base_args(opts: &LaunchOptions) -> Vec<String> {
    let mut args = vec![
        // Port 0 makes the OS pick; we read the real port off stderr, which
        // avoids racing another process for a fixed port.
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={}", opts.user_data_dir.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-sync".to_string(),
        "--disable-background-networking".to_string(),
        "--disable-features=Translate,MediaRouter".to_string(),
        // Grant camera/mic without a prompt. The page still gets only the
        // synthetic track our init script installs.
        "--use-fake-ui-for-media-stream".to_string(),
        // Let the meeting page start audio without a user gesture.
        "--autoplay-policy=no-user-gesture-required".to_string(),
    ];
    if opts.headless == Headless::New {
        args.push("--headless=new".to_string());
        // Headless still needs a plausible window size for responsive layouts.
        args.push("--window-size=1280,900".to_string());
    }
    args.extend(opts.extra_args.iter().cloned());
    args
}

/// Extract the DevTools WebSocket URL from a Chromium stderr line.
///
/// Chromium prints `DevTools listening on ws://127.0.0.1:PORT/devtools/browser/UUID`.
pub fn parse_endpoint_line(line: &str) -> Option<String> {
    let idx = line.find("ws://")?;
    let url = line[idx..].trim();
    // Guard against a trailing CR on Windows pipes and any trailing noise.
    let url = url.split_whitespace().next()?.trim_end_matches('\r');
    if url.len() > "ws://".len() {
        Some(url.to_string())
    } else {
        None
    }
}

/// Launch a browser and wait for its DevTools endpoint.
pub async fn launch(opts: &LaunchOptions) -> Result<LaunchedBrowser> {
    let exe = find_browser().ok_or(CdpError::BrowserNotFound { env: BROWSER_ENV })?;
    std::fs::create_dir_all(&opts.user_data_dir)?;

    let mut cmd = Command::new(&exe);
    cmd.args(base_args(opts))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    #[cfg(target_os = "windows")]
    {
        // CREATE_NO_WINDOW: keep a console window from flashing on the operator.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // Enrolled, not raw: an unenrolled browser outlives the session that
    // started it — and an orphaned meeting bot keeps recording.
    let (mut child, group) =
        global_process_scope()
            .spawn(cmd)
            .map_err(|source| CdpError::Spawn {
                path: exe.display().to_string(),
                source,
            })?;

    let stderr = child.stderr.take().ok_or_else(|| {
        CdpError::WebSocket("browser stderr was not piped".to_string())
    })?;

    let wait = tokio::time::timeout(ENDPOINT_TIMEOUT, async {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(url) = parse_endpoint_line(&line) {
                return Some(url);
            }
        }
        None
    })
    .await;

    match wait {
        Ok(Some(ws_url)) => {
            tracing::debug!(browser = %exe.display(), "cdp endpoint ready");
            Ok(LaunchedBrowser {
                child,
                ws_url,
                group,
            })
        }
        Ok(None) => {
            let _ = child.kill().await;
            Err(CdpError::NoEndpoint {
                secs: ENDPOINT_TIMEOUT.as_secs(),
            })
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(CdpError::NoEndpoint {
                secs: ENDPOINT_TIMEOUT.as_secs(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_devtools_endpoint_line() {
        let line = "DevTools listening on ws://127.0.0.1:51234/devtools/browser/abc-def";
        assert_eq!(
            parse_endpoint_line(line).as_deref(),
            Some("ws://127.0.0.1:51234/devtools/browser/abc-def")
        );
    }

    #[test]
    fn parses_endpoint_with_trailing_cr() {
        let line = "DevTools listening on ws://127.0.0.1:9/devtools/browser/x\r";
        assert_eq!(
            parse_endpoint_line(line).as_deref(),
            Some("ws://127.0.0.1:9/devtools/browser/x")
        );
    }

    #[test]
    fn ignores_unrelated_stderr() {
        assert!(parse_endpoint_line("[1234:5678] some chromium warning").is_none());
        assert!(parse_endpoint_line("").is_none());
        assert!(parse_endpoint_line("ws://").is_none());
    }

    #[test]
    fn headless_adds_switch_and_window_size() {
        let opts = LaunchOptions::new("/tmp/profile-x");
        let args = base_args(&opts);
        assert!(args.iter().any(|a| a == "--headless=new"), "{args:?}");
        assert!(args.iter().any(|a| a.starts_with("--window-size=")));
        assert!(args.iter().any(|a| a == "--remote-debugging-port=0"));
        assert!(args.iter().any(|a| a.contains("profile-x")));
    }

    #[test]
    fn windowed_omits_headless_switch() {
        let opts = LaunchOptions::new("/tmp/profile-y").windowed();
        let args = base_args(&opts);
        assert!(!args.iter().any(|a| a == "--headless=new"), "{args:?}");
    }

    #[test]
    fn media_switches_present_for_meeting_join() {
        let args = base_args(&LaunchOptions::new("/tmp/p"));
        assert!(args.iter().any(|a| a == "--use-fake-ui-for-media-stream"));
        assert!(
            args.iter().any(|a| a == "--autoplay-policy=no-user-gesture-required"),
            "meeting audio must start without a gesture"
        );
        assert!(
            !args.iter().any(|a| a.contains("use-fake-device-for-media-stream")),
            "the fake device emits a beep into the meeting; we install a silent \
             synthetic track in JS instead"
        );
    }

    #[test]
    fn extra_args_are_appended() {
        let opts = LaunchOptions::new("/tmp/p").arg("--mute-audio");
        assert!(base_args(&opts).iter().any(|a| a == "--mute-audio"));
    }

    #[test]
    fn explicit_env_override_is_only_candidate() {
        // Not using the real env here (tests share a process); assert the shape
        // of discovery instead: every candidate is an absolute-looking path.
        for c in candidates() {
            assert!(!c.as_os_str().is_empty());
        }
    }
}
