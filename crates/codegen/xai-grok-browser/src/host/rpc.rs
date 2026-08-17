//! Named-pipe JSON-RPC server (background tokio runtime).
//!
//! Receives newline-delimited requests, posts them to the UI thread via
//! `mpsc` + `PostThreadMessageW(WM_APP)`, and writes the response on the
//! same connection.

use std::io;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::oneshot;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP, WM_QUIT};

use super::{DecodedRpcError, HostCall, HostError, JsonRpcId, decode_host_call};

/// One request marshaled onto the UI thread.
pub struct UiJob {
    /// Decoded call, or a JSON-RPC error produced on the pipe thread.
    pub call: Result<(JsonRpcId, HostCall), DecodedRpcError>,
    /// Response line (including trailing `\n`).
    pub reply: Sender<String>,
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
) -> Result<PipeThread, HostError> {
    let (ready_tx, ready_rx) = mpsc::channel();
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
                        serve_loop(pipe, first, ui_thread_id, cmd_tx, shutdown_rx).await;
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

async fn serve_loop(
    pipe: String,
    first: NamedPipeServer,
    ui_thread_id: u32,
    cmd_tx: Sender<UiJob>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut next = Some(first);
    loop {
        let server = match next.take() {
            Some(s) => s,
            None => match ServerOptions::new().create(&pipe) {
                Ok(s) => s,
                Err(_) => break,
            },
        };

        tokio::select! {
            _ = &mut shutdown => break,
            connected = server.connect() => {
                match connected {
                    Ok(()) => {
                        next = ServerOptions::new().create(&pipe).ok();
                        tokio::select! {
                            _ = &mut shutdown => break,
                            _ = handle_connection(server, ui_thread_id, &cmd_tx) => {}
                        }
                    }
                    Err(_) => {
                        // Drop the failed instance and retry unless we are
                        // shutting down.
                    }
                }
            }
        }
    }
    // Pipe death (or shutdown): wake the UI pump so the host exits.
    unsafe {
        let _ = PostThreadMessageW(ui_thread_id, WM_QUIT, WPARAM::default(), LPARAM::default());
    }
}

async fn handle_connection(server: NamedPipeServer, ui_thread_id: u32, cmd_tx: &Sender<UiJob>) {
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

        let call = decode_host_call(&line);
        let (reply_tx, reply_rx) = mpsc::channel();
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

        let response = match reply_rx.recv() {
            Ok(line) => line,
            Err(_) => break,
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
    use crate::protocol::{METHOD_EVAL, METHOD_NAVIGATE, METHOD_TABS};

    #[test]
    fn pipe_thread_decode_navigate_roundtrip() {
        let line = encode_rpc_request(
            JsonRpcId::Number(9),
            METHOD_NAVIGATE,
            serde_json::json!({ "url": "https://example.com/" }),
        )
        .unwrap();
        let (id, call) = decode_host_call(&line).unwrap();
        assert_eq!(id, JsonRpcId::Number(9));
        assert_eq!(
            call,
            HostCall::Navigate {
                url: "https://example.com/".into()
            }
        );
    }

    #[test]
    fn pipe_thread_decode_tabs_and_eval() {
        let tabs =
            encode_rpc_request(JsonRpcId::Number(1), METHOD_TABS, serde_json::json!({})).unwrap();
        assert!(matches!(decode_host_call(&tabs).unwrap().1, HostCall::Tabs));

        let eval = encode_rpc_request(
            JsonRpcId::Number(2),
            METHOD_EVAL,
            serde_json::json!({ "function": "() => 1" }),
        )
        .unwrap();
        let err = decode_host_call(&eval).unwrap_err();
        assert_eq!(err.error.code, RPC_METHOD_NOT_FOUND);
        assert!(err.error.message.contains("not implemented in Task 4"));
    }
}
