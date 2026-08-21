//! Lazy Agent WebView sidecar lifecycle (`turbo browser-host`).
//!
//! The first `browser_*` tool call asks this module to spawn the same
//! `turbo.exe` with `browser-host --session-id <id>` and wait for
//! `\\.\pipe\turbo-browser-<id>`. Session teardown sends `browser.shutdown`
//! and kills the child if it is still alive.
//!
//! Tests never spawn WebView2.

use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use xai_tty_utils::{ProcessGroup, global_process_scope};

use xai_grok_browser::{BrowserClient, pipe_name};
#[cfg(windows)]
use xai_grok_tools::implementations::grok_build::browser::WEBVIEW2_RUNTIME_HELP;
use xai_grok_tools::implementations::grok_build::browser::set_browser_ensure;

/// How long [`ensure_browser_host`] waits for the named pipe.
///
/// WebView2 environment/controller create is unbounded on a cold Evergreen
/// install; the pipe now binds before that work, but 45s still covers a
/// slow first paint so we do not kill a host that is about to come up.
pub const ENSURE_TIMEOUT: Duration = Duration::from_secs(45);

/// Sidecar is Windows-only in v1; mock tool tests still run everywhere.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentBrowserError {
    /// Host / tools compile on all platforms; spawn is Windows-only.
    #[error("Agent WebView is Windows-only in v1")]
    WindowsOnly,
    /// Spawn, pipe wait, or early host exit.
    #[error("{0}")]
    Failed(String),
}

/// Result of a successful [`ensure_browser_host`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserHostHandle {
    /// Pager/session id the host is bound to.
    pub session_id: String,
    /// `true` when the pipe was already accepting connections (no spawn).
    pub already_running: bool,
}

struct TrackedHost {
    child: Child,
    /// Job Object / process group so leftover msedgewebview2.exe dies with us.
    _group: Option<Arc<ProcessGroup>>,
}

fn children() -> &'static Mutex<HashMap<String, TrackedHost>> {
    static CHILDREN: OnceLock<Mutex<HashMap<String, TrackedHost>>> = OnceLock::new();
    CHILDREN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-session spawn lock.
///
/// A single global lock was held across the whole 15s wait, so a second
/// session starting a browser queued behind the first session's timeout.
#[cfg(windows)]
fn ensure_lock_for(session_id: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        guard
            .entry(session_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

fn lock_map() -> std::sync::MutexGuard<'static, HashMap<String, TrackedHost>> {
    children().lock().unwrap_or_else(|e| e.into_inner())
}

fn take_child(session_id: &str) -> Option<Child> {
    lock_map().remove(session_id).map(|tracked| tracked.child)
}

#[cfg(windows)]
fn store_child(session_id: &str, child: Child, group: Option<Arc<ProcessGroup>>) {
    lock_map().insert(
        session_id.to_owned(),
        TrackedHost {
            child,
            _group: group,
        },
    );
}

/// Argv after `current_exe()` for the sidecar (no program name).
pub fn browser_host_argv(
    session_id: &str,
    session_folder: Option<&std::path::Path>,
) -> Vec<String> {
    let mut argv = vec![
        "browser-host".to_owned(),
        "--session-id".to_owned(),
        session_id.to_owned(),
    ];
    // Without this the host refuses every `file:` URL the client policy allows,
    // because it has no session folder to measure them against.
    if let Some(folder) = session_folder {
        argv.push("--session-folder".to_owned());
        argv.push(folder.display().to_string());
    }
    argv
}

/// Product-facing timeout text (always 45s, even if a test uses a shorter wait).
pub fn ensure_timeout_message(session_id: &str) -> String {
    format!(
        "browser host did not become ready within 45s (pipe {})",
        pipe_name(session_id)
    )
}

/// Whether `\\.\pipe\turbo-browser-<id>` accepts a client connection.
///
/// `ClientOptions::open` needs a Tokio 1.x reactor. Reuse the current
/// runtime when the tool path already has one; otherwise spin a
/// current-thread runtime for this probe only.
pub fn pipe_connectable(session_id: &str) -> bool {
    #[cfg(windows)]
    {
        fn try_open(session_id: &str) -> bool {
            tokio::net::windows::named_pipe::ClientOptions::new()
                .open(pipe_name(session_id))
                .is_ok()
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            return try_open(session_id);
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .ok()
            .is_some_and(|rt| rt.block_on(async { try_open(session_id) }))
    }
    #[cfg(not(windows))]
    {
        let _ = session_id;
        false
    }
}

#[cfg(windows)]
fn looks_like_missing_webview2(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("webview2")
        && (lower.contains("runtime")
            || lower.contains("not installed")
            || lower.contains("evergreen"))
}

#[cfg(windows)]
fn early_exit_error(
    session_id: &str,
    status: std::process::ExitStatus,
    stderr: &str,
) -> AgentBrowserError {
    if looks_like_missing_webview2(stderr) || looks_like_missing_webview2(&status.to_string()) {
        return AgentBrowserError::Failed(WEBVIEW2_RUNTIME_HELP.to_owned());
    }
    let stderr = stderr.trim();
    let detail = if stderr.is_empty() {
        format!("browser host exited before the pipe was ready (session {session_id}, {status})")
    } else {
        format!(
            "browser host exited before the pipe was ready (session {session_id}, {status}): {stderr}"
        )
    };
    AgentBrowserError::Failed(detail)
}

/// Add first-class `browser_*` tools when they are not already in the toolset.
pub fn inject_browser_tools(config: &mut xai_grok_tools::registry::types::ToolServerConfig) {
    use xai_grok_tools::implementations::grok_build as gb;
    use xai_grok_tools::registry::types::ToolConfig;
    let extras = [
        ToolConfig::from(&gb::BrowserNavigateTool),
        ToolConfig::from(&gb::BrowserSnapshotTool),
        ToolConfig::from(&gb::BrowserClickTool),
        ToolConfig::from(&gb::BrowserFillTool),
        ToolConfig::from(&gb::BrowserEvalTool),
        ToolConfig::from(&gb::BrowserScreenshotTool),
        ToolConfig::from(&gb::BrowserTabsTool),
        ToolConfig::from(&gb::BrowserWaitTool),
        ToolConfig::from(&gb::BrowserScrollTool),
        ToolConfig::from(&gb::BrowserPressKeyTool),
        ToolConfig::from(&gb::BrowserSelectTool),
        ToolConfig::from(&gb::BrowserHoverTool),
        ToolConfig::from(&gb::BrowserSetFileTool),
        ToolConfig::from(&gb::BrowserRaiseTool),
    ];
    for extra in extras {
        if !config.tools.iter().any(|tool| tool.id == extra.id) {
            config.tools.push(extra);
        }
    }
}

/// Install the process-wide `BrowserHandle` ensure hook (idempotent replace).
pub fn install_browser_ensure_hook() {
    set_browser_ensure(Arc::new(|session_id, session_folder| {
        ensure_browser_host_in(session_id, session_folder)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }));
}

/// Spawn `turbo browser-host` if the session pipe is not already up.
///
/// Waits up to 45s for the named pipe. The child is enrolled in
/// [`xai_tty_utils::ProcessGroup`] / the global process scope so leftover
/// `msedgewebview2.exe` dies with the pager. Non-Windows:
/// [`AgentBrowserError::WindowsOnly`].
pub fn ensure_browser_host(session_id: &str) -> Result<BrowserHostHandle, AgentBrowserError> {
    ensure_browser_host_in(session_id, None)
}

/// [`ensure_browser_host`], additionally telling the sidecar which folder may
/// serve `file:` URLs.
pub fn ensure_browser_host_in(
    session_id: &str,
    session_folder: Option<&std::path::Path>,
) -> Result<BrowserHostHandle, AgentBrowserError> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(AgentBrowserError::Failed(
            "browser host requires a session id".into(),
        ));
    }

    #[cfg(not(windows))]
    {
        let _ = session_id;
        return Err(AgentBrowserError::WindowsOnly);
    }

    #[cfg(windows)]
    {
        let lock = ensure_lock_for(session_id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if pipe_connectable(session_id) {
            return Ok(BrowserHostHandle {
                session_id: session_id.to_owned(),
                already_running: true,
            });
        }

        use std::io::Read;
        use std::process::Stdio;
        use std::time::Instant;

        let exe = std::env::current_exe()
            .map_err(|e| AgentBrowserError::Failed(format!("current_exe for browser-host: {e}")))?;
        let argv = browser_host_argv(session_id, session_folder);
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(&argv);
        xai_tty_utils::detach_std_command(&mut cmd);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| AgentBrowserError::Failed(format!("spawn browser-host: {e}")))?;
        let group = ProcessGroup::new()
            .and_then(|mut g| {
                g.attach_std(&child)?;
                let g = Arc::new(g);
                if !global_process_scope().register(&g) {
                    return Err(std::io::Error::other(
                        "process scope already closed; browser-host killed",
                    ));
                }
                Ok(g)
            })
            .ok();

        let deadline = Instant::now() + ENSURE_TIMEOUT;
        loop {
            if pipe_connectable(session_id) {
                // Drain stderr on its own thread. The pipe is never read on the
                // success path, so anything the host writes later (a panic, a
                // WebView2 diagnostic) would fill the ~4KB buffer and block the
                // host forever on its next write.
                if let Some(pipe) = child.stderr.take() {
                    let sid = session_id.to_owned();
                    let _ = std::thread::Builder::new()
                        .name("browser-host-stderr".into())
                        .spawn(move || {
                            use std::io::BufRead;
                            for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok)
                            {
                                tracing::debug!(session = %sid, "browser-host: {line}");
                            }
                        });
                }
                store_child(session_id, child, group);
                return Ok(BrowserHostHandle {
                    session_id: session_id.to_owned(),
                    already_running: false,
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    return Err(early_exit_error(session_id, status, &stderr));
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = child.kill();
                    return Err(AgentBrowserError::Failed(format!(
                        "wait for browser-host: {e}"
                    )));
                }
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentBrowserError::Failed(ensure_timeout_message(
                    session_id,
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// How long to wait for a graceful `browser.shutdown` before killing.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Ask the host to shut down, then kill the tracked child if it is still alive.
pub async fn shutdown_browser_host(session_id: &str) {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return;
    }
    xai_grok_tools::implementations::grok_build::browser::forget_session(session_id);
    let client = BrowserClient::new(session_id);
    // Bounded: a wedged host would otherwise hang teardown here and never
    // reach the kill below, which exists for exactly that case.
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, client.shutdown()).await;
    if let Some(mut child) = take_child(session_id) {
        match child.try_wait() {
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Ok(Some(_)) => {}
            Err(_) => {
                let _ = child.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_browser_tools_is_idempotent() {
        let mut config = xai_grok_tools::registry::types::ToolServerConfig::default();
        inject_browser_tools(&mut config);
        inject_browser_tools(&mut config);
        let ids: Vec<_> = config.tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids.iter().filter(|id| id.contains("browser_")).count(),
            14,
            "{ids:?}"
        );
        assert!(ids.contains(&"GrokBuild:browser_navigate"));
    }

    #[test]
    fn browser_host_argv_is_browser_host_and_session_id() {
        assert_eq!(
            browser_host_argv("sess-1", None),
            vec![
                "browser-host".to_string(),
                "--session-id".to_string(),
                "sess-1".to_string()
            ]
        );
    }

    /// Regression: the spawn dropped the session folder, so the host refused
    /// every `file:` URL the client-side policy had just allowed.
    #[test]
    fn browser_host_argv_carries_the_session_folder() {
        let argv = browser_host_argv("sess-1", Some(std::path::Path::new(r"H:\sessionsbc")));
        let i = argv
            .iter()
            .position(|a| a == "--session-folder")
            .expect("--session-folder must be passed to the host");
        assert_eq!(argv[i + 1], r"H:\sessionsbc");
    }

    #[test]
    fn timeout_error_message_mentions_45s_and_pipe() {
        let msg = ensure_timeout_message("abc");
        assert!(msg.contains("45s"), "{msg}");
        assert!(msg.contains(r"\\.\pipe\turbo-browser-abc"), "{msg}");
    }

    #[cfg(not(windows))]
    #[test]
    fn ensure_is_windows_only() {
        let err = ensure_browser_host("sess-unix").expect_err("non-Windows must refuse");
        assert_eq!(err, AgentBrowserError::WindowsOnly);
        assert!(err.to_string().contains("Windows-only"), "{err}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn already_running_probe_short_circuits_spawn() {
        let sid = format!("agent-browser-probe-{}", std::process::id());
        let pipe = pipe_name(&sid);
        let _server = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe)
            .expect("create probe pipe");
        // One probe only: connecting consumes the dummy instance.
        let handle = ensure_browser_host(&sid).expect("already running");
        assert!(handle.already_running);
        assert_eq!(handle.session_id, sid);
        assert!(
            take_child(&sid).is_none(),
            "already-running probe must not spawn / store a child"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pipe_connectable_is_false_for_missing_pipe() {
        assert!(!pipe_connectable("does-not-exist-agent-browser-task7"));
    }
}
