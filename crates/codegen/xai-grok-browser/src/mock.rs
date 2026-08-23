//! In-process mock Agent WebView host.
//!
//! Stores the current URL/title and a canned AX tree (`uid=1` link, `uid=2`
//! textbox). Applies [`check_url`] / [`check_fill`] / [`check_eval_result`]
//! and fails closed before mutating state.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::Value;

use crate::client::{BrowserClientError, BrowserTransport};
use crate::protocol::{
    AxNode, BrowserRequest, ClickResult, DownloadsResult, JsonRpcId, JsonRpcRequest,
    JsonRpcVersion, NavigateResult, ScreenshotResult, SnapshotResult, TabInfo, TabsResult,
    WaitResult, check_eval_confirm, check_eval_result, check_fill, check_navigation_hop,
    single_tab_v1_error,
};

/// Recorded click/fill from the mock host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockAction {
    /// Last successful `browser.click`.
    Click {
        /// Snapshot uid.
        uid: String,
    },
    /// Last successful `browser.fill`.
    Fill {
        /// Snapshot uid.
        uid: String,
        /// Value that passed fill policy.
        value: String,
    },
}

#[derive(Debug)]
struct MockState {
    url: String,
    title: String,
    nodes: Vec<AxNode>,
    last_action: Option<MockAction>,
    calls: Vec<String>,
    shutdown: bool,
    session_folder: Option<std::path::PathBuf>,
}

impl MockState {
    fn fresh() -> Self {
        Self {
            url: "about:blank".into(),
            title: String::new(),
            nodes: canned_nodes(),
            last_action: None,
            calls: Vec::new(),
            shutdown: false,
            session_folder: None,
        }
    }
}

/// In-process host that implements [`BrowserTransport`].
#[derive(Debug, Clone)]
pub struct MockBrowserHost {
    inner: Arc<Mutex<MockState>>,
}

impl Default for MockBrowserHost {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::client::BrowserClient<MockBrowserHost> {
    /// Client bound to a fresh in-process mock host.
    pub fn mock(session_id: impl Into<String>) -> Self {
        Self::with_transport(session_id, MockBrowserHost::new())
    }
}

impl MockBrowserHost {
    /// New mock parked on `about:blank` with the canned AX tree.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockState::fresh())),
        }
    }

    /// Current page URL.
    pub fn url(&self) -> String {
        self.lock().url.clone()
    }

    /// Current document title.
    pub fn title(&self) -> String {
        self.lock().title.clone()
    }

    /// Last successful click/fill, if any.
    pub fn last_action(&self) -> Option<MockAction> {
        self.lock().last_action.clone()
    }

    /// Methods received by this host (empty when the client rejected first).
    pub fn call_log(&self) -> Vec<String> {
        self.lock().calls.clone()
    }

    /// Current canned (or fill-updated) AX nodes.
    pub fn nodes(&self) -> Vec<AxNode> {
        self.lock().nodes.clone()
    }

    /// Append an AX node (tests inject names like "Buy now").
    pub fn insert_node(&self, node: AxNode) {
        self.lock().nodes.push(node);
    }

    /// Session folder used to list brokered downloads in tests.
    pub fn set_session_folder(&self, folder: impl Into<std::path::PathBuf>) {
        self.lock().session_folder = Some(folder.into());
    }

    fn lock(&self) -> MutexGuard<'_, MockState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BrowserTransport for MockBrowserHost {
    async fn call(&self, method: &str, params: Value) -> Result<Value, BrowserClientError> {
        dispatch(self, method, params)
    }
}

fn dispatch(
    host: &MockBrowserHost,
    method: &str,
    params: Value,
) -> Result<Value, BrowserClientError> {
    let env = JsonRpcRequest {
        jsonrpc: JsonRpcVersion,
        id: JsonRpcId::Number(0),
        method: method.to_owned(),
        params,
    };
    let request = env.browser_request()?;

    let mut state = host.lock();
    state.calls.push(method.to_owned());
    if state.shutdown {
        return Err(BrowserClientError::Transport(
            "mock browser host is shut down".into(),
        ));
    }

    match request {
        BrowserRequest::Navigate { url } => {
            check_navigation_hop(Some(&url), None)?;
            apply_navigate(&mut state, url);
            to_value(NavigateResult {
                url: state.url.clone(),
                title: state.title.clone(),
            })
        }
        BrowserRequest::Tabs {} => to_value(current_tabs(&state)),
        BrowserRequest::NewTab { .. } => Err(BrowserClientError::Rpc {
            code: -32601,
            message: single_tab_v1_error("browser.new_tab"),
        }),
        BrowserRequest::SelectTab { .. } => Err(BrowserClientError::Rpc {
            code: -32601,
            message: single_tab_v1_error("browser.select_tab"),
        }),
        BrowserRequest::CloseTab { .. } => Err(BrowserClientError::Rpc {
            code: -32601,
            message: single_tab_v1_error("browser.close_tab"),
        }),
        BrowserRequest::Snapshot {
            verbose: _,
            include_text,
        } => {
            let text = include_text.then(|| "Example Domain".to_owned());
            to_value(SnapshotResult {
                url: state.url.clone(),
                title: state.title.clone(),
                source: crate::protocol::SnapshotSource::Dom,
                nodes: state.nodes.clone(),
                overlay: None,
                text,
            })
        }
        BrowserRequest::Click { uid } => {
            find_node(&state.nodes, &uid)?;
            state.last_action = Some(MockAction::Click { uid });
            to_value(ClickResult {
                url: state.url.clone(),
                title: state.title.clone(),
            })
        }
        BrowserRequest::Fill { uid, value } => {
            let field_name = find_node(&state.nodes, &uid)?.name.clone();
            check_fill(&value, Some(&field_name))?;
            if let Some(node) = state.nodes.iter_mut().find(|n| n.uid == uid) {
                node.value = Some(value.clone());
            }
            state.last_action = Some(MockAction::Fill { uid, value });
            Ok(empty_object())
        }
        BrowserRequest::Eval { function, confirm } => {
            check_eval_confirm(&function, confirm)?;
            let payload = serde_json::json!({ "title": state.title });
            let serialized =
                serde_json::to_string(&payload).map_err(BrowserClientError::from_json)?;
            check_eval_result(&serialized)?;
            Ok(payload)
        }
        BrowserRequest::Screenshot {} => to_value(ScreenshotResult {
            path: "images/browser-1.png".into(),
            width: 1280,
            height: 800,
        }),
        BrowserRequest::Downloads {} => {
            let folder = state.session_folder.clone();
            match folder.as_deref() {
                Some(folder) => {
                    let listed = crate::host::download::list_brokered_downloads(
                        folder,
                        &std::collections::HashSet::new(),
                    )
                    .map_err(BrowserClientError::Transport)?;
                    to_value(listed)
                }
                None => to_value(DownloadsResult::default()),
            }
        }
        BrowserRequest::Raise {} => Ok(empty_object()),
        BrowserRequest::Shutdown {} => {
            state.shutdown = true;
            Ok(empty_object())
        }
        BrowserRequest::Wait {
            text: _,
            url_substring,
            timeout_ms: _,
        } => {
            if let Some(sub) = url_substring {
                if !state.url.contains(&sub) {
                    return Err(BrowserClientError::Rpc {
                        code: -32000,
                        message: format!("wait timed out for url containing {sub:?}"),
                    });
                }
            }
            to_value(WaitResult {
                url: state.url.clone(),
                title: state.title.clone(),
            })
        }
        BrowserRequest::Scroll { .. }
        | BrowserRequest::PressKey { .. }
        | BrowserRequest::Hover { .. }
        | BrowserRequest::SetFile { .. } => Ok(empty_object()),
        BrowserRequest::Select { uid, value: _ } => {
            find_node(&state.nodes, &uid)?;
            Ok(empty_object())
        }
    }
}

fn apply_navigate(state: &mut MockState, url: String) {
    state.title = title_for_url(&url);
    state.url = url;
    state.nodes = canned_nodes();
    state.last_action = None;
}

fn current_tabs(state: &MockState) -> TabsResult {
    TabsResult {
        tabs: vec![TabInfo {
            tab_id: 1,
            url: state.url.clone(),
            title: state.title.clone(),
            active: true,
        }],
    }
}

fn canned_nodes() -> Vec<AxNode> {
    vec![
        AxNode {
            uid: "1-1".into(),
            role: "link".into(),
            name: "More information".into(),
            value: None,
            focused: false,
        },
        AxNode {
            uid: "1-2".into(),
            role: "textbox".into(),
            name: "Search".into(),
            value: Some(String::new()),
            focused: false,
        },
    ]
}

fn find_node<'a>(nodes: &'a [AxNode], uid: &str) -> Result<&'a AxNode, BrowserClientError> {
    nodes
        .iter()
        .find(|n| n.uid == uid)
        .ok_or_else(|| BrowserClientError::UnknownUid(uid.to_owned()))
}

fn title_for_url(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if host.is_empty() {
        url.to_owned()
    } else {
        host.to_owned()
    }
}

fn to_value<T: Serialize>(value: T) -> Result<Value, BrowserClientError> {
    serde_json::to_value(value).map_err(BrowserClientError::from_json)
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FillPolicyError, METHOD_FILL, METHOD_NAVIGATE, UrlPolicyError};

    #[tokio::test]
    async fn mock_rejects_otp_without_mutating() {
        let host = MockBrowserHost::new();
        let err = host
            .call(
                METHOD_FILL,
                serde_json::json!({ "uid": "1-2", "value": "123456" }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, BrowserClientError::Fill(FillPolicyError::OtpShaped)),
            "{err:?}"
        );
        assert_eq!(host.last_action(), None);
        assert_eq!(host.nodes()[1].value.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn mock_rejects_file_url_without_mutating() {
        let host = MockBrowserHost::new();
        let err = host
            .call(
                METHOD_NAVIGATE,
                serde_json::json!({ "url": "file:///C:/Windows/notepad.exe" }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, BrowserClientError::Url(UrlPolicyError::FileDenied)),
            "{err:?}"
        );
        assert_eq!(host.url(), "about:blank");
    }

    #[tokio::test]
    async fn mock_fill_updates_textbox() {
        let host = MockBrowserHost::new();
        host.call(
            METHOD_FILL,
            serde_json::json!({ "uid": "1-2", "value": "hello" }),
        )
        .await
        .unwrap();
        assert_eq!(
            host.last_action(),
            Some(MockAction::Fill {
                uid: "1-2".into(),
                value: "hello".into(),
            })
        );
        assert_eq!(host.nodes()[1].value.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn mock_rejects_mutating_eval_without_confirm() {
        let host = MockBrowserHost::new();
        let err = host
            .call(
                crate::protocol::METHOD_EVAL,
                serde_json::json!({
                    "function": "() => location.assign('https://evil.test')",
                    "confirm": false
                }),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                BrowserClientError::Eval(crate::protocol::EvalPolicyError::NeedsConfirm)
            ),
            "{err:?}"
        );
        host.call(
            crate::protocol::METHOD_EVAL,
            serde_json::json!({
                "function": "() => location.assign('https://evil.test')",
                "confirm": true
            }),
        )
        .await
        .expect("confirmed mutating eval");
    }

    #[tokio::test]
    async fn mock_new_tab_is_honest_single_tab() {
        let host = MockBrowserHost::new();
        let err = host
            .call(crate::protocol::METHOD_NEW_TAB, serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("v1 is a single tab"), "{err}");
        assert_eq!(host.url(), "about:blank");
    }
}
