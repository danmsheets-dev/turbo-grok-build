//! Loopback WebSocket server that receives PCM from the meeting page.
//!
//! Audio rides its own socket rather than a CDP binding: 20 ms frames are 50
//! messages/sec and `Runtime.bindingCalled` would base64-inflate every one.
//!
//! The listener binds `127.0.0.1` only, and the URL carries a random token that
//! the handshake checks, so another local process cannot push audio into a
//! meeting transcript.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use rand::Rng;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

use crate::error::{BotError, Result};

/// A running loopback audio sink.
#[derive(Debug)]
pub struct AudioServer {
    url: String,
    frames: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl AudioServer {
    /// `ws://127.0.0.1:<port>/<token>` — hand this to the page.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Total PCM frames accepted. A stalled tap shows up as a flat counter.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// PCM frames shed because STT was not keeping up. Surfaced in
    /// `meeting_status` so a degraded transcript is visible, not silent.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Stop accepting connections.
    pub fn shutdown(&self) {
        self.task.abort();
    }
}

impl Drop for AudioServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bind a loopback listener and forward every binary frame to `pcm_tx`.
pub async fn start(pcm_tx: mpsc::Sender<Vec<u8>>) -> Result<AudioServer> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| BotError::Audio(format!("bind loopback audio port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| BotError::Audio(format!("read loopback port: {e}")))?
        .port();
    let token = random_token();
    let url = format!("ws://127.0.0.1:{port}/{token}");

    let frames = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(AtomicU64::new(0));
    let task = tokio::spawn(accept_loop(
        listener,
        format!("/{token}"),
        pcm_tx,
        Arc::clone(&frames),
        Arc::clone(&dropped),
    ));

    Ok(AudioServer {
        url,
        frames,
        dropped,
        task,
    })
}

async fn accept_loop(
    listener: TcpListener,
    expected_path: String,
    pcm_tx: mpsc::Sender<Vec<u8>>,
    frames: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        // Belt and braces: the socket is already loopback-bound.
        if !peer.ip().is_loopback() {
            continue;
        }
        let expected = expected_path.clone();
        let tx = pcm_tx.clone();
        let counter = Arc::clone(&frames);
        let drops = Arc::clone(&dropped);
        tokio::spawn(async move {
            let check = |req: &Request, resp: Response| -> std::result::Result<Response, ErrorResponse> {
                if req.uri().path() == expected {
                    Ok(resp)
                } else {
                    Err(ErrorResponse::new(Some("bad audio token".to_string())))
                }
            };
            let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
                return;
            };
            pump(ws, tx, counter, drops).await;
        });
    }
}

async fn pump<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    pcm_tx: mpsc::Sender<Vec<u8>>,
    frames: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (_sink, mut source) = ws.split();
    while let Some(Ok(msg)) = source.next().await {
        let bytes = match msg {
            Message::Binary(b) => b.to_vec(),
            Message::Close(_) => break,
            _ => continue,
        };
        if bytes.is_empty() {
            continue;
        }
        // 16-bit samples: an odd length means a torn frame, not audio.
        if !bytes.len().is_multiple_of(2) {
            tracing::warn!(len = bytes.len(), "dropping odd-length PCM frame");
            continue;
        }
        // Shed, never block. `run_stt_loop` stops draining `pcm_rx` entirely
        // while it retries auth or reconnects the STT socket; awaiting `send`
        // here would stall the reader, apply backpressure through the
        // WebSocket, and pile live meeting audio up in the browser. Local
        // WASAPI capture drops frames under the same pressure — matching that
        // keeps the transcript real-time on both transports.
        match pcm_tx.try_send(bytes) {
            Ok(()) => {
                frames.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                let n = dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n.is_multiple_of(100) {
                    tracing::warn!(dropped = n, "meeting STT is not keeping up; dropping audio");
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::SinkExt;

    #[test]
    fn tokens_are_long_and_distinct() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b, "token must not be predictable across meetings");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn url_is_loopback_only() {
        let (tx, _rx) = mpsc::channel(4);
        let server = start(tx).await.unwrap();
        assert!(server.url().starts_with("ws://127.0.0.1:"), "{}", server.url());
        assert_eq!(server.frames(), 0);
    }

    #[tokio::test]
    async fn accepts_pcm_on_the_right_token() {
        let (tx, mut rx) = mpsc::channel(8);
        let server = start(tx).await.unwrap();
        let (mut ws, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        ws.send(Message::Binary(vec![1u8, 0, 2, 0].into()))
            .await
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("frame should arrive")
            .expect("channel open");
        assert_eq!(got, vec![1u8, 0, 2, 0]);
        assert_eq!(server.frames(), 1);
    }

    #[tokio::test]
    async fn rejects_a_wrong_token() {
        let (tx, _rx) = mpsc::channel(4);
        let server = start(tx).await.unwrap();
        let port = server
            .url()
            .rsplit(':')
            .next()
            .and_then(|s| s.split('/').next())
            .unwrap()
            .to_string();
        let bad = format!("ws://127.0.0.1:{port}/deadbeef");
        assert!(
            tokio_tungstenite::connect_async(bad).await.is_err(),
            "a guessed token must not stream audio into the transcript"
        );
    }

    /// Live audio must be shed, not queued: `run_stt_loop` stops draining
    /// `pcm_rx` while it reconnects, and blocking here would back the meeting
    /// up inside the browser.
    #[tokio::test]
    async fn sheds_frames_when_stt_is_not_draining() {
        let (tx, _rx) = mpsc::channel(2);
        let server = start(tx).await.unwrap();
        let (mut ws, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        for _ in 0..50 {
            ws.send(Message::Binary(vec![1u8, 0].into())).await.unwrap();
        }
        // Never blocks; the surplus is counted as dropped rather than queued.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while server.dropped() == 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(server.dropped() > 0, "surplus audio must be shed");
        assert!(
            server.frames() <= 3,
            "only what fit in the channel is accepted, got {}",
            server.frames()
        );
    }

    #[tokio::test]
    async fn drops_odd_length_frames() {
        let (tx, mut rx) = mpsc::channel(8);
        let server = start(tx).await.unwrap();
        let (mut ws, _) = tokio_tungstenite::connect_async(server.url()).await.unwrap();
        ws.send(Message::Binary(vec![9u8].into())).await.unwrap();
        ws.send(Message::Binary(vec![1u8, 0].into())).await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("frame should arrive")
            .unwrap();
        assert_eq!(got, vec![1u8, 0], "odd frame must be skipped, not forwarded");
        assert_eq!(server.frames(), 1);
    }
}
