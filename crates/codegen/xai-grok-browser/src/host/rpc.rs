//! Named-pipe JSON-RPC server (background tokio runtime).
//!
//! Receives newline-delimited requests, posts them to the UI thread via
//! `mpsc` + `PostThreadMessageW(WM_APP)`, and writes the response on the
//! same connection.

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP, WM_QUIT};

use super::{
    DecodedRpcError, HostCall, HostError, JsonRpcError, JsonRpcId, RPC_HOST_ERROR,
    decode_host_call, encode_rpc_error,
};

/// One request marshaled onto the UI thread.
pub struct UiJob {
    /// Decoded call, or a JSON-RPC error produced on the pipe thread.
    pub call: Result<(JsonRpcId, HostCall), DecodedRpcError>,
    /// Response line (including trailing `\n`).
    ///
    /// A `tokio` oneshot, not `std::sync::mpsc`: awaiting it lets the accept
    /// loop keep polling shutdown. Blocking here on a current-thread runtime
    /// stalled the whole runtime whenever the UI thread was busy, so a wedged
    /// page made the host unkillable.
    pub reply: oneshot::Sender<String>,
}

/// Handle for the background pipe server.
pub struct PipeThread {
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl PipeThread {
    /// Ask the accept loop to stop and join the worker thread.
    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PipeThread {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Bind `\\.\pipe\turbo-browser-*` and serve connections until shutdown.
pub fn spawn_pipe_thread(
    pipe: String,
    ui_thread_id: u32,
    cmd_tx: Sender<UiJob>,
    session_folder: Option<PathBuf>,
) -> Result<PipeThread, HostError> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let join = std::thread::Builder::new()
        .name("turbo-browser-pipe".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(io::Error::other(e)));
                    return;
                }
            };
            rt.block_on(async move {
                match bind_first_instance(&pipe) {
                    Ok(first) => {
                        let _ = ready_tx.send(Ok(()));
                        serve_loop(
                            pipe,
                            first,
                            ui_thread_id,
                            cmd_tx,
                            shutdown_rx,
                            session_folder,
                        )
                        .await;
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            });
        })
        .map_err(|e| HostError::Failed(format!("spawn pipe thread: {e}")))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(PipeThread {
            shutdown: Some(shutdown_tx),
            join: Some(join),
        }),
        Ok(Err(e)) => {
            let _ = shutdown_tx.send(());
            let _ = join.join();
            Err(HostError::Failed(format!("named pipe {e}")))
        }
        Err(_) => Err(HostError::Failed("pipe thread exited before ready".into())),
    }
}

fn bind_first_instance(pipe: &str) -> io::Result<NamedPipeServer> {
    ServerOptions::new().first_pipe_instance(true).create(pipe)
}

/// Spare listening instances kept alive alongside the one being served.
///
/// The client opens a fresh connection per request. With a single instance,
/// every request raced the window between `connect()` returning and the next
/// `create()`, and lost with `ERROR_PIPE_BUSY`.
const SPARE_INSTANCES: usize = 4;

/// Serve the pipe with `SPARE_INSTANCES + 1` acceptors, each awaiting its own
/// instance concurrently.
///
/// Every listening instance must be *awaiting* `connect()`, not merely created.
/// Windows hands an incoming client to any instance in the listening state, so
/// an instance nobody awaits still accepts the client — and then blocks it
/// forever, because no task will ever read its request. The previous shape
/// created five instances and awaited exactly one, which meant a fresh
/// `browser_*` call had a four-in-five chance of hanging until the session died.
async fn serve_loop(
    pipe: String,
    first: NamedPipeServer,
    ui_thread_id: u32,
    cmd_tx: Sender<UiJob>,
    shutdown: oneshot::Receiver<()>,
    session_folder: Option<PathBuf>,
) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut acceptors = JoinSet::new();

    let mut first = Some(first);
    for _ in 0..=SPARE_INSTANCES {
        let server = match first.take() {
            Some(s) => s,
            None => match ServerOptions::new().create(&pipe) {
                Ok(s) => s,
                Err(_) => break,
            },
        };
        acceptors.spawn(acceptor(
            pipe.clone(),
            server,
            ui_thread_id,
            cmd_tx.clone(),
            stop_rx.clone(),
            session_folder.clone(),
        ));
    }
    drop(stop_rx);

    tokio::select! {
        _ = shutdown => {}
        // Every acceptor exited on its own: the pipe is gone, so the host must
        // follow it down rather than idle with no way to be reached.
        _ = async { while acceptors.join_next().await.is_some() {} } => {}
    }
    let _ = stop_tx.send(true);
    acceptors.shutdown().await;

    // Pipe death (or shutdown): wake the UI pump so the host exits.
    unsafe {
        let _ = PostThreadMessageW(ui_thread_id, WM_QUIT, WPARAM::default(), LPARAM::default());
    }
}

/// Own one pipe instance for the host's lifetime: await a client, serve it, then
/// re-arm a fresh instance so the listening count stays constant.
async fn acceptor(
    pipe: String,
    mut server: NamedPipeServer,
    ui_thread_id: u32,
    cmd_tx: Sender<UiJob>,
    mut stop: watch::Receiver<bool>,
    session_folder: Option<PathBuf>,
) {
    let folder = session_folder.as_deref();
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            connected = server.connect() => {
                if connected.is_err() {
                    return;
                }
                tokio::select! {
                    _ = stop.changed() => return,
                    _ = handle_connection(server, ui_thread_id, &cmd_tx, folder) => {}
                }
                server = match ServerOptions::new().create(&pipe) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
        }
    }
}

/// Ceiling on how long the pipe side waits for the UI thread before answering
/// on its own.
///
/// Three budgets, deliberately ordered. `webview::NAV_TIMEOUT` (60s) bounds one
/// navigation and reports the real reason. This one bounds *everything else*:
/// queue wait behind a slow job, a modal loop, a blocking COM call — none of
/// which `pump_until` can see. `client::CALL_TIMEOUT` (75s) is last and should
/// now only fire when the host process is genuinely dead.
///
/// rc2 had no middle rung, so a request the UI thread never reached left the
/// connection open with nothing written on it, and the agent saw a bare
/// "host did not respond within 75s" transport error instead of a host error.
const REPLY_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(super::webview::NAV_TIMEOUT.as_secs() + 6);

/// Request id and a human label for one decoded call, taken *before* the job is
/// handed to the UI thread — once `call` moves into the `UiJob` the id is gone,
/// and a timeout still has to answer on the caller's own id.
fn describe_call(call: &Result<(JsonRpcId, HostCall), DecodedRpcError>) -> (JsonRpcId, &'static str) {
    match call {
        Ok((id, host_call)) => (id.clone(), method_label(host_call)),
        Err(err) => (
            err.id.clone().unwrap_or(JsonRpcId::Number(0)),
            "browser request",
        ),
    }
}

fn method_label(call: &HostCall) -> &'static str {
    match call {
        HostCall::Navigate { .. } => "browser.navigate",
        HostCall::Screenshot => "browser.screenshot",
        HostCall::Tabs => "browser.tabs",
        HostCall::Raise => "browser.raise",
        HostCall::Shutdown => "browser.shutdown",
        HostCall::Snapshot { .. } => "browser.snapshot",
        HostCall::Click { .. } => "browser.click",
        HostCall::Fill { .. } => "browser.fill",
        HostCall::Eval { .. } => "browser.eval",
    }
}

/// The response written when the UI thread misses [`REPLY_BUDGET`].
fn timeout_response(id: JsonRpcId, what: &str, budget: std::time::Duration) -> String {
    encode_rpc_error(
        id,
        JsonRpcError {
            code: RPC_HOST_ERROR,
            message: format!(
                "{what}: the browser host did not answer within {}s. The page is still                  loading or hung. The window is still open — retry, or navigate elsewhere.",
                budget.as_secs()
            ),
            data: None,
        },
    )
}

async fn handle_connection(
    server: NamedPipeServer,
    ui_thread_id: u32,
    cmd_tx: &Sender<UiJob>,
    session_folder: Option<&std::path::Path>,
) {
    let mut reader = BufReader::new(server);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if line.trim().is_empty() {
            continue;
        }

        let call = decode_host_call(&line, session_folder);
        let (id, what) = describe_call(&call);
        let (reply_tx, reply_rx) = oneshot::channel();
        if cmd_tx
            .send(UiJob {
                call,
                reply: reply_tx,
            })
            .is_err()
        {
            break;
        }
        wake_ui_thread(ui_thread_id);

        let response = match tokio::time::timeout(REPLY_BUDGET, reply_rx).await {
            Ok(Ok(line)) => line,
            // UI thread dropped the reply channel: the host is going away.
            Ok(Err(_)) => break,
            Err(_) => {
                eprintln!(
                    "turbo browser-host: {what} unanswered after {}s; failing the call closed",
                    REPLY_BUDGET.as_secs()
                );
                timeout_response(id, what, REPLY_BUDGET)
            }
        };

        let pipe = reader.get_mut();
        if pipe.write_all(response.as_bytes()).await.is_err() {
            break;
        }
        if pipe.flush().await.is_err() {
            break;
        }
    }
}

fn wake_ui_thread(thread_id: u32) {
    // SAFETY: WM_APP is an ignorable thread message used only to wake
    // GetMessageW on the UI thread (see webview2-com sample).
    unsafe {
        let _ = PostThreadMessageW(thread_id, WM_APP, WPARAM::default(), LPARAM::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::encode_rpc_request;
    use crate::host::{HostCall, RPC_METHOD_NOT_FOUND};
    use crate::protocol::{
        METHOD_EVAL, METHOD_NAVIGATE, METHOD_NEW_TAB, METHOD_SNAPSHOT, METHOD_TABS,
    };

    #[test]
    fn pipe_thread_decode_navigate_roundtrip() {
        let line = encode_rpc_request(
            JsonRpcId::Number(9),
            METHOD_NAVIGATE,
            serde_json::json!({ "url": "https://example.com/" }),
        )
        .unwrap();
        let (id, call) = decode_host_call(&line, None).unwrap();
        assert_eq!(id, JsonRpcId::Number(9));
        assert_eq!(
            call,
            HostCall::Navigate {
                url: "https://example.com/".into()
            }
        );
    }

    #[test]
    fn pipe_thread_decode_tabs_snapshot_and_eval() {
        let tabs =
            encode_rpc_request(JsonRpcId::Number(1), METHOD_TABS, serde_json::json!({})).unwrap();
        assert!(matches!(
            decode_host_call(&tabs, None).unwrap().1,
            HostCall::Tabs
        ));

        let snap = encode_rpc_request(
            JsonRpcId::Number(2),
            METHOD_SNAPSHOT,
            serde_json::json!({ "verbose": false }),
        )
        .unwrap();
        assert!(matches!(
            decode_host_call(&snap, None).unwrap().1,
            HostCall::Snapshot { verbose: false }
        ));

        let eval = encode_rpc_request(
            JsonRpcId::Number(3),
            METHOD_EVAL,
            serde_json::json!({ "function": "() => 1" }),
        )
        .unwrap();
        match decode_host_call(&eval, None).unwrap().1 {
            HostCall::Eval { function } => assert_eq!(function, "() => 1"),
            other => panic!("{other:?}"),
        }

        let new_tab =
            encode_rpc_request(JsonRpcId::Number(4), METHOD_NEW_TAB, serde_json::json!({}))
                .unwrap();
        let err = decode_host_call(&new_tab, None).unwrap_err();
        assert_eq!(err.error.code, RPC_METHOD_NOT_FOUND);
        assert!(err.error.message.contains("not implemented"));
    }
}

#[cfg(test)]
mod pipe_concurrency_tests {
    use super::*;

    /// The three budgets must stay ordered. rc2 shipped with no pipe-side rung,
    /// so a `browser_navigate` the UI thread did not reach in time produced no
    /// line at all and the agent saw a bare 75s transport timeout.
    #[test]
    fn reply_budget_sits_between_nav_timeout_and_client_timeout() {
        assert!(
            REPLY_BUDGET > crate::host::webview::NAV_TIMEOUT,
            "REPLY_BUDGET must not pre-empt navigate's own, more specific error"
        );
        assert!(
            REPLY_BUDGET < crate::client::CALL_TIMEOUT,
            "REPLY_BUDGET must fire before the client gives up on the transport"
        );
    }

    /// A missed reply is a JSON-RPC error on the caller's id, never silence.
    #[test]
    fn ui_thread_timeout_is_a_jsonrpc_error_on_the_same_id() {
        let line = timeout_response(
            JsonRpcId::Number(7),
            "browser.navigate",
            std::time::Duration::from_secs(66),
        );
        assert!(line.ends_with('\n'), "responses are newline framed: {line:?}");
        let v: serde_json::Value = serde_json::from_str(line.trim()).expect("valid JSON");
        assert_eq!(v["id"], 7, "must answer on the caller's id");
        assert_eq!(v["error"]["code"], RPC_HOST_ERROR);
        assert!(
            v["error"]["message"].as_str().unwrap().contains("browser.navigate"),
            "the agent must be told which call failed: {v}"
        );
    }

    use std::io::{BufRead, BufReader as StdBufReader, Write};
    use std::time::Duration;
    use windows::Win32::System::Threading::GetCurrentThreadId;

    /// One client round-trip, bounded so a stranded pipe instance shows up as a
    /// timeout instead of hanging the test run forever.
    fn round_trip(pipe: &str, id: u32) -> Result<String, String> {
        let pipe = pipe.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let res = (|| -> std::io::Result<String> {
                let mut f = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&pipe)?;
                writeln!(
                    f,
                    r#"{{"jsonrpc":"2.0","id":{id},"method":"browser.tabs","params":{{}}}}"#
                )?;
                f.flush()?;
                let mut reader = StdBufReader::new(f);
                let mut line = String::new();
                reader.read_line(&mut line)?;
                Ok(line)
            })();
            let _ = tx.send(res.map_err(|e| e.to_string()));
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(r) => r,
            Err(_) => Err("timed out — instance created but never awaited".into()),
        }
    }

    /// Regression: the server used to create `SPARE_INSTANCES + 1` pipe instances
    /// but `connect()` on only one of them. Windows hands an incoming client to
    /// any *listening* instance, so four out of five clients were accepted by an
    /// instance nobody awaited and blocked forever — the agent saw a
    /// `browser_*` call that never returned and an empty WebView window.
    #[test]
    fn every_listening_instance_serves_a_client() {
        let pipe = crate::profile::pipe_name(&format!("test-{}", std::process::id()));
        let (tx, rx) = std::sync::mpsc::channel::<UiJob>();
        let ui_thread_id = unsafe { GetCurrentThreadId() };

        let servicer = std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let id = match &job.call {
                    Ok((id, _)) => format!("{id:?}"),
                    Err(_) => "null".to_string(),
                };
                let _ = job
                    .reply
                    .send(format!("{{\"served\":true,\"id_dbg\":\"{id}\"}}\n"));
            }
        });

        let server = spawn_pipe_thread(pipe.clone(), ui_thread_id, tx, None)
            .expect("pipe thread must bind");

        // More clients than there are instances, so every slot is exercised and
        // re-armed at least once.
        for i in 0..(SPARE_INSTANCES as u32 + 3) {
            match round_trip(&pipe, i) {
                Ok(line) => assert!(
                    line.contains("\"served\":true"),
                    "client {i} got an unexpected reply: {line}"
                ),
                Err(e) => panic!("client {i} failed: {e}"),
            }
        }

        server.shutdown();
        drop(servicer);
    }
}
