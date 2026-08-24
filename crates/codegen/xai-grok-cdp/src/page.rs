//! High-level browser and page handles over [`Connection`].

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::conn::{CdpEvent, Connection};
use crate::error::{CdpError, Result};
use crate::launch::{LaunchOptions, LaunchedBrowser, launch};

/// A launched browser plus its DevTools connection.
///
/// Dropping this kills the browser process (`kill_on_drop`), so a panicking
/// meeting never leaves an orphaned headless Edge behind.
#[derive(Debug)]
pub struct Browser {
    conn: Arc<Connection>,
    _process: LaunchedBrowser,
}

impl Browser {
    /// Launch a browser and connect to it.
    pub async fn launch(opts: &LaunchOptions) -> Result<Self> {
        let process = launch(opts).await?;
        let conn = Connection::connect(&process.ws_url).await?;
        Ok(Self {
            conn,
            _process: process,
        })
    }

    /// Open a new tab and attach a flattened session to it.
    pub async fn new_page(&self) -> Result<Page> {
        let created = self
            .conn
            .send(
                "Target.createTarget",
                json!({ "url": "about:blank" }),
                None,
            )
            .await?;
        let target_id = created
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or(CdpError::MalformedResponse {
                method: "Target.createTarget".to_string(),
                field: "targetId",
            })?
            .to_string();

        let attached = self
            .conn
            .send(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or(CdpError::MalformedResponse {
                method: "Target.attachToTarget".to_string(),
                field: "sessionId",
            })?
            .to_string();

        let page = Page {
            conn: Arc::clone(&self.conn),
            session_id,
            target_id,
        };
        page.send("Page.enable", json!({})).await?;
        page.send("Runtime.enable", json!({})).await?;
        Ok(page)
    }

    /// Close the DevTools connection. The process dies with this value.
    pub async fn close(self) {
        self.conn.close().await;
    }
}

/// One tab.
#[derive(Debug, Clone)]
pub struct Page {
    conn: Arc<Connection>,
    session_id: String,
    target_id: String,
}

impl Page {
    /// The DevTools target id backing this page.
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Send a command scoped to this page's session.
    pub async fn send(&self, method: &str, params: Value) -> Result<Value> {
        self.conn
            .send(method, params, Some(&self.session_id))
            .await
    }

    /// Navigate and wait for the commit.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        let res = self.send("Page.navigate", json!({ "url": url })).await?;
        if let Some(err) = res
            .get("errorText")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty())
        {
            return Err(CdpError::Protocol {
                method: "Page.navigate".to_string(),
                message: err.to_string(),
            });
        }
        Ok(())
    }

    /// Evaluate an expression, awaiting promises, returning the value by value.
    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        let res = self
            .send(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(details) = res.get("exceptionDetails") {
            return Err(CdpError::JavaScript(describe_exception(details)));
        }
        Ok(res
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Install a script that runs before any page script on every navigation.
    pub async fn add_init_script(&self, source: &str) -> Result<()> {
        self.send(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source }),
        )
        .await
        .map(|_| ())
    }

    /// Expose `window.<name>(payload)` so page JS can push data to Rust.
    ///
    /// Calls arrive as `Runtime.bindingCalled` events; see [`Page::binding_stream`].
    pub async fn expose_binding(&self, name: &str) -> Result<()> {
        self.send("Runtime.addBinding", json!({ "name": name }))
            .await
            .map(|_| ())
    }

    /// Subscribe to every event for this page's session.
    pub fn events(&self) -> broadcast::Receiver<CdpEvent> {
        self.conn.subscribe()
    }

    /// Subscribe to payloads pushed through one exposed binding.
    pub fn binding_stream(&self, name: &str) -> BindingStream {
        BindingStream {
            rx: self.conn.subscribe(),
            session_id: self.session_id.clone(),
            name: name.to_string(),
        }
    }

    /// Poll a JavaScript boolean expression until it is true or the deadline passes.
    ///
    /// Returns `Ok(true)` when the expression became true, `Ok(false)` on timeout.
    /// Evaluation errors are treated as "not yet" so a page mid-navigation does
    /// not abort the wait.
    pub async fn wait_for_expression(
        &self,
        expression: &str,
        timeout: Duration,
        poll: Duration,
    ) -> Result<bool> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Ok(Value::Bool(true)) = self.evaluate(expression).await {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// Close this tab.
    pub async fn close(&self) -> Result<()> {
        self.conn
            .send(
                "Target.closeTarget",
                json!({ "targetId": self.target_id }),
                None,
            )
            .await
            .map(|_| ())
    }
}

/// Payloads pushed from page JS through one binding.
#[derive(Debug)]
pub struct BindingStream {
    rx: broadcast::Receiver<CdpEvent>,
    session_id: String,
    name: String,
}

impl BindingStream {
    /// Await the next payload for this binding.
    ///
    /// Returns `None` when the connection closed. Lagged events are skipped
    /// rather than ending the stream: a burst of chat must not kill the tap.
    pub async fn next(&mut self) -> Option<String> {
        loop {
            match self.rx.recv().await {
                Ok(ev) => {
                    if ev.method != "Runtime.bindingCalled" {
                        continue;
                    }
                    if ev.session_id.as_deref() != Some(self.session_id.as_str()) {
                        continue;
                    }
                    if ev.params.get("name").and_then(Value::as_str) != Some(self.name.as_str()) {
                        continue;
                    }
                    return Some(
                        ev.params
                            .get("payload")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    );
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, binding = %self.name, "cdp binding lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// Render a `Runtime.exceptionDetails` object as one line.
fn describe_exception(details: &Value) -> String {
    let desc = details
        .pointer("/exception/description")
        .and_then(Value::as_str);
    if let Some(d) = desc {
        return d.lines().next().unwrap_or(d).to_string();
    }
    details
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("unknown page exception")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_prefers_description_first_line() {
        let details = json!({
            "text": "Uncaught",
            "exception": { "description": "TypeError: x is not a function\n    at <anonymous>:1:1" }
        });
        assert_eq!(
            describe_exception(&details),
            "TypeError: x is not a function"
        );
    }

    #[test]
    fn exception_falls_back_to_text() {
        assert_eq!(
            describe_exception(&json!({ "text": "Uncaught SyntaxError" })),
            "Uncaught SyntaxError"
        );
        assert_eq!(
            describe_exception(&json!({})),
            "unknown page exception"
        );
    }

    #[tokio::test]
    async fn binding_stream_filters_by_session_and_name() {
        let (tx, rx) = broadcast::channel(8);
        let mut stream = BindingStream {
            rx,
            session_id: "S1".into(),
            name: "turboChat".into(),
        };
        // Wrong method, wrong session, wrong name, then the real one.
        let _ = tx.send(CdpEvent {
            method: "Runtime.consoleAPICalled".into(),
            params: json!({ "name": "turboChat", "payload": "no" }),
            session_id: Some("S1".into()),
        });
        let _ = tx.send(CdpEvent {
            method: "Runtime.bindingCalled".into(),
            params: json!({ "name": "turboChat", "payload": "other-session" }),
            session_id: Some("S2".into()),
        });
        let _ = tx.send(CdpEvent {
            method: "Runtime.bindingCalled".into(),
            params: json!({ "name": "turboAudio", "payload": "other-binding" }),
            session_id: Some("S1".into()),
        });
        let _ = tx.send(CdpEvent {
            method: "Runtime.bindingCalled".into(),
            params: json!({ "name": "turboChat", "payload": "{\"text\":\"hi\"}" }),
            session_id: Some("S1".into()),
        });
        assert_eq!(stream.next().await.as_deref(), Some("{\"text\":\"hi\"}"));
    }

    #[tokio::test]
    async fn binding_stream_ends_when_channel_closes() {
        let (tx, rx) = broadcast::channel::<CdpEvent>(2);
        let mut stream = BindingStream {
            rx,
            session_id: "S1".into(),
            name: "n".into(),
        };
        drop(tx);
        assert_eq!(stream.next().await, None);
    }

    #[tokio::test]
    async fn binding_stream_payload_defaults_to_empty() {
        let (tx, rx) = broadcast::channel(2);
        let mut stream = BindingStream {
            rx,
            session_id: "S1".into(),
            name: "n".into(),
        };
        let _ = tx.send(CdpEvent {
            method: "Runtime.bindingCalled".into(),
            params: json!({ "name": "n" }),
            session_id: Some("S1".into()),
        });
        assert_eq!(stream.next().await.as_deref(), Some(""));
    }
}
