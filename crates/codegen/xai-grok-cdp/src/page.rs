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
        deny_downloads(&conn).await;
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

/// Refuse downloads for the whole browser.
///
/// A meeting page has no business downloading anything, and Teams' desktop-app
/// launcher tries: `directDl=true` pulls an installer, which on Windows can
/// surface as a file-manager window the operator never asked for.
///
/// Best-effort by design. This crate pins no DevTools protocol version, so a
/// future rename must degrade to "downloads are allowed", never to "the
/// notetaker cannot start".
async fn deny_downloads(conn: &Connection) {
    let params = json!({ "behavior": "deny", "eventsEnabled": true });
    if let Err(e) = conn.send("Browser.setDownloadBehavior", params, None).await {
        tracing::debug!(error = %e, "browser download policy not applied");
    }
}

/// A navigation the page performed or attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Navigation {
    /// CDP method that reported it, e.g. `Page.frameNavigated`.
    pub method: String,
    /// Target URL, when the event carried one.
    pub url: String,
}

/// CDP events that describe a navigation.
///
/// `frameRequestedNavigation` is marked experimental upstream and
/// `downloadWillBegin` only fires once a download policy sets `eventsEnabled`,
/// so both are observed opportunistically and neither may carry control flow.
const NAVIGATION_METHODS: &[&str] = &[
    "Page.frameNavigated",
    "Page.navigatedWithinDocument",
    "Page.frameRequestedNavigation",
    "Page.downloadWillBegin",
];

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

    /// Subscribe to this page's navigations.
    ///
    /// `Page.enable` is already sent in [`Browser::new_page`], so the redirect
    /// chain is *already* flowing through the connection and being discarded.
    /// This just stops throwing it away: reconstructing a failed join from the
    /// browser profile's History file afterwards is not a diagnostic story.
    pub fn navigation_stream(&self) -> NavigationStream {
        NavigationStream {
            rx: self.conn.subscribe(),
            session_id: self.session_id.clone(),
        }
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

/// Navigations reported for one page's session.
#[derive(Debug)]
pub struct NavigationStream {
    rx: broadcast::Receiver<CdpEvent>,
    session_id: String,
}

impl NavigationStream {
    /// Await the next navigation. `None` once the connection closes.
    ///
    /// Lag is skipped rather than fatal, exactly as [`BindingStream`] does: a
    /// second subscriber must never be able to starve the audio-critical
    /// binding stream, and a missed log line is not worth ending the stream.
    pub async fn next(&mut self) -> Option<Navigation> {
        loop {
            match self.rx.recv().await {
                Ok(ev) => {
                    if !NAVIGATION_METHODS.contains(&ev.method.as_str()) {
                        continue;
                    }
                    if ev.session_id.as_deref() != Some(self.session_id.as_str()) {
                        continue;
                    }
                    return Some(Navigation {
                        method: ev.method,
                        url: navigation_url(&ev.params),
                    });
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(skipped = n, "cdp navigation stream lagged");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// Pull the URL out of whichever navigation event shape this is.
fn navigation_url(params: &Value) -> String {
    for pointer in ["/frame/url", "/url"] {
        if let Some(u) = params.pointer(pointer).and_then(Value::as_str) {
            return u.to_string();
        }
    }
    String::new()
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
    async fn navigation_stream_filters_to_page_events_for_this_session() {
        let (tx, rx) = broadcast::channel(8);
        let mut nav = NavigationStream {
            rx,
            session_id: "S1".into(),
        };
        // Not a navigation.
        let _ = tx.send(CdpEvent {
            method: "Runtime.bindingCalled".into(),
            params: json!({ "name": "x" }),
            session_id: Some("S1".into()),
        });
        // Right method, wrong session.
        let _ = tx.send(CdpEvent {
            method: "Page.frameNavigated".into(),
            params: json!({ "frame": { "url": "https://other/" } }),
            session_id: Some("S2".into()),
        });
        // The real one, in the nested `frame` shape.
        let _ = tx.send(CdpEvent {
            method: "Page.frameNavigated".into(),
            params: json!({ "frame": { "url": "https://teams.microsoft.com/dl/launcher/launcher.html" } }),
            session_id: Some("S1".into()),
        });
        assert_eq!(
            nav.next().await,
            Some(Navigation {
                method: "Page.frameNavigated".into(),
                url: "https://teams.microsoft.com/dl/launcher/launcher.html".into(),
            })
        );
    }

    /// The four navigation events do not agree on where the URL lives.
    #[test]
    fn navigation_url_reads_both_event_shapes() {
        assert_eq!(
            navigation_url(&json!({ "frame": { "url": "https://a/" } })),
            "https://a/"
        );
        assert_eq!(navigation_url(&json!({ "url": "https://b/" })), "https://b/");
        assert_eq!(navigation_url(&json!({})), "");
    }

    /// A download is the `directDl=true` half of the Teams launcher hop.
    #[test]
    fn download_events_count_as_navigations() {
        assert!(NAVIGATION_METHODS.contains(&"Page.downloadWillBegin"));
        assert!(NAVIGATION_METHODS.contains(&"Page.frameNavigated"));
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
