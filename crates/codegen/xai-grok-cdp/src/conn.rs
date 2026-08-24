//! JSON-RPC transport for the DevTools Protocol.
//!
//! One WebSocket carries every session (flat mode). Commands are correlated by
//! `id`; everything without an `id` is an event and is broadcast to subscribers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::error::{CdpError, Result};

/// Default per-command deadline.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Capacity of the event broadcast channel. Meeting pages are chatty; a slow
/// subscriber lags rather than blocking the reader.
const EVENT_CAPACITY: usize = 1024;

/// A DevTools event.
#[derive(Debug, Clone)]
pub struct CdpEvent {
    /// e.g. `Runtime.bindingCalled`.
    pub method: String,
    /// Event payload (`params`), or `null`.
    pub params: Value,
    /// Session the event belongs to, when flattened sessions are in use.
    pub session_id: Option<String>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

/// A live DevTools connection.
#[derive(Debug)]
pub struct Connection {
    next_id: AtomicU64,
    sink: Mutex<futures_util::stream::SplitSink<WsStream, Message>>,
    pending: Pending,
    events: broadcast::Sender<CdpEvent>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

impl Connection {
    /// Connect to a DevTools WebSocket URL and start the reader task.
    pub async fn connect(ws_url: &str) -> Result<Arc<Self>> {
        let (stream, _resp) = tokio_tungstenite::connect_async(ws_url)
            .await
            .map_err(|e| CdpError::WebSocket(e.to_string()))?;
        let (sink, mut source) = stream.split();

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);

        let conn = Arc::new(Self {
            next_id: AtomicU64::new(1),
            sink: Mutex::new(sink),
            pending: Arc::clone(&pending),
            events: events.clone(),
            reader: Mutex::new(None),
        });

        let reader = tokio::spawn(async move {
            while let Some(msg) = source.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t.as_str().to_string(),
                    Ok(Message::Binary(b)) => match String::from_utf8(b.to_vec()) {
                        Ok(s) => s,
                        Err(_) => continue,
                    },
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                dispatch(&value, &pending, &events).await;
            }
            // Connection is gone: fail every in-flight command instead of
            // leaving callers to time out one by one.
            let mut map = pending.lock().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err(String::new()));
            }
        });

        *conn.reader.lock().await = Some(reader);
        Ok(conn)
    }

    /// Subscribe to DevTools events.
    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    /// Send a command and await its result.
    ///
    /// `session_id` targets an attached session; `None` addresses the browser.
    pub async fn send(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut payload = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            payload["sessionId"] = Value::String(sid.to_string());
        }

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let text = payload.to_string();
        let send_result = {
            let mut sink = self.sink.lock().await;
            sink.send(Message::Text(text.into())).await
        };
        if let Err(e) = send_result {
            self.pending.lock().await.remove(&id);
            return Err(CdpError::WebSocket(e.to_string()));
        }

        match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(message))) => {
                if message.is_empty() {
                    Err(CdpError::ConnectionClosed)
                } else {
                    Err(CdpError::Protocol {
                        method: method.to_string(),
                        message,
                    })
                }
            }
            Ok(Err(_)) => Err(CdpError::ConnectionClosed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(CdpError::Timeout {
                    method: method.to_string(),
                    secs: COMMAND_TIMEOUT.as_secs(),
                })
            }
        }
    }

    /// Close the transport and stop the reader task.
    pub async fn close(&self) {
        {
            let mut sink = self.sink.lock().await;
            let _ = sink.close().await;
        }
        if let Some(handle) = self.reader.lock().await.take() {
            handle.abort();
        }
    }
}

/// Route one decoded DevTools frame to a pending command or to subscribers.
async fn dispatch(value: &Value, pending: &Pending, events: &broadcast::Sender<CdpEvent>) {
    if let Some(id) = value.get("id").and_then(Value::as_u64) {
        let Some(tx) = pending.lock().await.remove(&id) else {
            return;
        };
        if let Some(err) = value.get("error") {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown devtools error");
            let data = err.get("data").and_then(Value::as_str);
            let message = match data {
                Some(d) if !d.is_empty() => format!("{message}: {d}"),
                _ => message.to_string(),
            };
            // A protocol error never carries an empty message, so the empty
            // string stays reserved for "connection closed".
            let message = if message.is_empty() {
                "unknown devtools error".to_string()
            } else {
                message
            };
            let _ = tx.send(Err(message));
        } else {
            let _ = tx.send(Ok(value.get("result").cloned().unwrap_or(Value::Null)));
        }
        return;
    }

    if let Some(method) = value.get("method").and_then(Value::as_str) {
        let _ = events.send(CdpEvent {
            method: method.to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
            session_id: value
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pending_with(id: u64) -> (Pending, oneshot::Receiver<std::result::Result<Value, String>>) {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(id, tx);
        (pending, rx)
    }

    #[tokio::test]
    async fn dispatch_resolves_result_by_id() {
        let (pending, rx) = pending_with(7).await;
        let (events, _guard) = broadcast::channel(4);
        let frame = json!({ "id": 7, "result": { "ok": true } });
        dispatch(&frame, &pending, &events).await;
        let got = rx.await.unwrap().unwrap();
        assert_eq!(got, json!({ "ok": true }));
        assert!(pending.lock().await.is_empty(), "entry must be consumed");
    }

    #[tokio::test]
    async fn dispatch_reports_protocol_error_with_data() {
        let (pending, rx) = pending_with(3).await;
        let (events, _guard) = broadcast::channel(4);
        let frame = json!({
            "id": 3,
            "error": { "code": -32000, "message": "Cannot find context", "data": "id 9" }
        });
        dispatch(&frame, &pending, &events).await;
        let err = rx.await.unwrap().unwrap_err();
        assert_eq!(err, "Cannot find context: id 9");
        assert!(!err.is_empty(), "empty is reserved for connection-closed");
    }

    #[tokio::test]
    async fn dispatch_never_yields_empty_error_message() {
        let (pending, rx) = pending_with(4).await;
        let (events, _guard) = broadcast::channel(4);
        let frame = json!({ "id": 4, "error": { "message": "" } });
        dispatch(&frame, &pending, &events).await;
        assert_eq!(rx.await.unwrap().unwrap_err(), "unknown devtools error");
    }

    #[tokio::test]
    async fn dispatch_broadcasts_events_with_session() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, mut rx) = broadcast::channel(4);
        let frame = json!({
            "method": "Runtime.bindingCalled",
            "params": { "name": "turboChat", "payload": "{}" },
            "sessionId": "S1"
        });
        dispatch(&frame, &pending, &events).await;
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.method, "Runtime.bindingCalled");
        assert_eq!(ev.session_id.as_deref(), Some("S1"));
        assert_eq!(ev.params["name"], "turboChat");
    }

    #[tokio::test]
    async fn dispatch_ignores_unknown_id() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _guard) = broadcast::channel(4);
        // Must not panic when a response arrives after its caller timed out.
        dispatch(&json!({ "id": 99, "result": {} }), &pending, &events).await;
    }

    #[tokio::test]
    async fn dispatch_result_defaults_to_null() {
        let (pending, rx) = pending_with(11).await;
        let (events, _guard) = broadcast::channel(4);
        dispatch(&json!({ "id": 11 }), &pending, &events).await;
        assert_eq!(rx.await.unwrap().unwrap(), Value::Null);
    }
}
