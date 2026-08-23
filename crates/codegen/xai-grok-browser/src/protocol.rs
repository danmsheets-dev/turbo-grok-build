//! JSON-RPC 2.0 wire types, browser methods, and navigation/fill/eval policy.
//!
//! The wire format is a standard envelope (`jsonrpc`, `id`, `method`, `params`),
//! not a serde internally-tagged enum. [`BrowserRequest`] is a convenience
//! decode of `method` + `params` after the envelope is parsed.

use std::borrow::Cow;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

/// JSON-RPC 2.0 version literal.
pub const JSONRPC_VERSION: &str = "2.0";

/// Host → client notification method.
pub const METHOD_EVENT: &str = "browser.event";

/// `browser.navigate`
pub const METHOD_NAVIGATE: &str = "browser.navigate";
/// `browser.tabs`
pub const METHOD_TABS: &str = "browser.tabs";
/// `browser.downloads`
pub const METHOD_DOWNLOADS: &str = "browser.downloads";
/// `browser.new_tab`
pub const METHOD_NEW_TAB: &str = "browser.new_tab";
/// `browser.select_tab`
pub const METHOD_SELECT_TAB: &str = "browser.select_tab";
/// `browser.close_tab`
pub const METHOD_CLOSE_TAB: &str = "browser.close_tab";
/// `browser.snapshot`
pub const METHOD_SNAPSHOT: &str = "browser.snapshot";
/// `browser.click`
pub const METHOD_CLICK: &str = "browser.click";
/// `browser.fill`
pub const METHOD_FILL: &str = "browser.fill";
/// `browser.eval`
pub const METHOD_EVAL: &str = "browser.eval";
/// `browser.screenshot`
pub const METHOD_SCREENSHOT: &str = "browser.screenshot";
/// `browser.raise`
pub const METHOD_RAISE: &str = "browser.raise";
/// `browser.shutdown`
pub const METHOD_SHUTDOWN: &str = "browser.shutdown";
/// `browser.wait`
pub const METHOD_WAIT: &str = "browser.wait";
/// `browser.scroll`
pub const METHOD_SCROLL: &str = "browser.scroll";
/// `browser.press_key`
pub const METHOD_PRESS_KEY: &str = "browser.press_key";
/// `browser.select`
pub const METHOD_SELECT: &str = "browser.select";
/// `browser.hover`
pub const METHOD_HOVER: &str = "browser.hover";
/// `browser.set_file`
pub const METHOD_SET_FILE: &str = "browser.set_file";

/// Cap on `browser.eval` JSON result size (bytes), same order as MCP output.
pub const EVAL_RESULT_MAX_BYTES: usize = 20_000;

/// JSON-RPC 2.0 protocol version marker. Wire value is always `"2.0"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonRpcVersion;

impl Serialize for JsonRpcVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(JSONRPC_VERSION)
    }
}

impl<'de> Deserialize<'de> for JsonRpcVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = JsonRpcVersion;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "the literal string \"{JSONRPC_VERSION}\"")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if v == JSONRPC_VERSION {
                    Ok(JsonRpcVersion)
                } else {
                    Err(E::custom(format!(
                        "expected jsonrpc \"{JSONRPC_VERSION}\", got {v:?}"
                    )))
                }
            }
        }
        deserializer.deserialize_str(V)
    }
}

/// JSON-RPC 2.0 request/response `id` (string or number).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// Numeric id, as in the protocol examples (`"id": 1`).
    Number(i64),
    /// String id.
    String(String),
}

/// Standard JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// Correlation id.
    pub id: JsonRpcId,
    /// Method name (e.g. `browser.navigate`).
    pub method: String,
    /// Method params. Missing / null become `Value::Null`.
    #[serde(default)]
    pub params: Value,
}

impl JsonRpcRequest {
    /// Decode [`BrowserRequest`] from this envelope's `method` + `params`.
    pub fn browser_request(&self) -> Result<BrowserRequest, ProtocolError> {
        if BrowserMethod::from_str(&self.method).is_err() {
            return Err(ProtocolError::UnknownMethod(self.method.clone()));
        }
        let params = if self.params.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            self.params.clone()
        };
        let tagged = serde_json::json!({
            "method": &self.method,
            "params": params,
        });
        serde_json::from_value(tagged).map_err(|e| ProtocolError::InvalidParams(e.to_string()))
    }
}

/// Standard JSON-RPC 2.0 error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Application or spec error code.
    pub code: i64,
    /// Human-readable message.
    pub message: String,
    /// Optional structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard JSON-RPC 2.0 response envelope (`result` or `error`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Must be `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// Correlation id from the request.
    pub id: JsonRpcId,
    /// Success payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Failure payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// Standard JSON-RPC 2.0 notification (no `id`).
///
/// Browser host events use [`METHOD_EVENT`] with [`BrowserEvent`] params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcEvent {
    /// Must be `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// Notification method (`browser.event` for host lifecycle).
    pub method: String,
    /// Event payload.
    pub params: BrowserEvent,
}

impl JsonRpcEvent {
    /// Host → client browser event notification.
    pub fn browser(event: BrowserEvent) -> Self {
        Self {
            jsonrpc: JsonRpcVersion,
            method: METHOD_EVENT.to_owned(),
            params: event,
        }
    }
}

/// Known `browser.*` methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserMethod {
    /// Navigate the active tab.
    Navigate,
    /// List tabs.
    Tabs,
    /// List brokered downloads in the session folder.
    Downloads,
    /// Open a tab.
    NewTab,
    /// Focus a tab.
    SelectTab,
    /// Close a tab.
    CloseTab,
    /// Accessibility snapshot.
    Snapshot,
    /// Click a uid from the last snapshot.
    Click,
    /// Fill a uid from the last snapshot.
    Fill,
    /// Evaluate a function expression (JSON result).
    Eval,
    /// Capture a screenshot.
    Screenshot,
    /// Raise the host window.
    Raise,
    /// Shut down the host.
    Shutdown,
    /// Wait for text or a URL substring.
    Wait,
    /// Scroll the page or a uid into view.
    Scroll,
    /// Dispatch a key.
    PressKey,
    /// Choose a `<select>` option.
    Select,
    /// Hover a uid.
    Hover,
    /// Set a file input from the session folder.
    SetFile,
}

impl BrowserMethod {
    /// Wire method name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => METHOD_NAVIGATE,
            Self::Tabs => METHOD_TABS,
            Self::Downloads => METHOD_DOWNLOADS,
            Self::NewTab => METHOD_NEW_TAB,
            Self::SelectTab => METHOD_SELECT_TAB,
            Self::CloseTab => METHOD_CLOSE_TAB,
            Self::Snapshot => METHOD_SNAPSHOT,
            Self::Click => METHOD_CLICK,
            Self::Fill => METHOD_FILL,
            Self::Eval => METHOD_EVAL,
            Self::Screenshot => METHOD_SCREENSHOT,
            Self::Raise => METHOD_RAISE,
            Self::Shutdown => METHOD_SHUTDOWN,
            Self::Wait => METHOD_WAIT,
            Self::Scroll => METHOD_SCROLL,
            Self::PressKey => METHOD_PRESS_KEY,
            Self::Select => METHOD_SELECT,
            Self::Hover => METHOD_HOVER,
            Self::SetFile => METHOD_SET_FILE,
        }
    }
}

impl FromStr for BrowserMethod {
    type Err = ProtocolError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            METHOD_NAVIGATE => Ok(Self::Navigate),
            METHOD_TABS => Ok(Self::Tabs),
            METHOD_DOWNLOADS => Ok(Self::Downloads),
            METHOD_NEW_TAB => Ok(Self::NewTab),
            METHOD_SELECT_TAB => Ok(Self::SelectTab),
            METHOD_CLOSE_TAB => Ok(Self::CloseTab),
            METHOD_SNAPSHOT => Ok(Self::Snapshot),
            METHOD_CLICK => Ok(Self::Click),
            METHOD_FILL => Ok(Self::Fill),
            METHOD_EVAL => Ok(Self::Eval),
            METHOD_SCREENSHOT => Ok(Self::Screenshot),
            METHOD_RAISE => Ok(Self::Raise),
            METHOD_SHUTDOWN => Ok(Self::Shutdown),
            METHOD_WAIT => Ok(Self::Wait),
            METHOD_SCROLL => Ok(Self::Scroll),
            METHOD_PRESS_KEY => Ok(Self::PressKey),
            METHOD_SELECT => Ok(Self::Select),
            METHOD_HOVER => Ok(Self::Hover),
            METHOD_SET_FILE => Ok(Self::SetFile),
            other => Err(ProtocolError::UnknownMethod(other.to_owned())),
        }
    }
}

/// Typed `method` + `params` for the browser host (not the wire envelope).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum BrowserRequest {
    /// `browser.navigate`
    #[serde(rename = "browser.navigate")]
    Navigate { url: String },
    /// `browser.tabs`
    #[serde(rename = "browser.tabs")]
    Tabs {},
    /// `browser.downloads`
    #[serde(rename = "browser.downloads")]
    Downloads {},
    /// `browser.new_tab`
    #[serde(rename = "browser.new_tab")]
    NewTab {
        #[serde(default)]
        url: Option<String>,
    },
    /// `browser.select_tab`
    #[serde(rename = "browser.select_tab")]
    SelectTab { tab_id: u32 },
    /// `browser.close_tab`
    #[serde(rename = "browser.close_tab")]
    CloseTab { tab_id: u32 },
    /// `browser.snapshot`
    #[serde(rename = "browser.snapshot")]
    Snapshot {
        #[serde(default)]
        verbose: bool,
        /// Include truncated main-landmark text.
        #[serde(default)]
        include_text: bool,
    },
    /// `browser.click`
    #[serde(rename = "browser.click")]
    Click { uid: String },
    /// `browser.fill`
    #[serde(rename = "browser.fill")]
    Fill { uid: String, value: String },
    /// `browser.eval`
    #[serde(rename = "browser.eval")]
    Eval {
        function: String,
        #[serde(default)]
        confirm: bool,
    },
    /// `browser.screenshot`
    #[serde(rename = "browser.screenshot")]
    Screenshot {},
    /// `browser.raise`
    #[serde(rename = "browser.raise")]
    Raise {},
    /// `browser.shutdown`
    #[serde(rename = "browser.shutdown")]
    Shutdown {},
    /// `browser.wait`
    #[serde(rename = "browser.wait")]
    Wait {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        url_substring: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// `browser.scroll`
    #[serde(rename = "browser.scroll")]
    Scroll {
        #[serde(default)]
        uid: Option<String>,
        #[serde(default)]
        dx: Option<i32>,
        #[serde(default)]
        dy: Option<i32>,
    },
    /// `browser.press_key`
    #[serde(rename = "browser.press_key")]
    PressKey {
        key: String,
        #[serde(default)]
        uid: Option<String>,
    },
    /// `browser.select`
    #[serde(rename = "browser.select")]
    Select { uid: String, value: String },
    /// `browser.hover`
    #[serde(rename = "browser.hover")]
    Hover { uid: String },
    /// `browser.set_file`
    #[serde(rename = "browser.set_file")]
    SetFile { uid: String, path: String },
}

/// Result of `browser.navigate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigateResult {
    /// Final URL after navigation.
    pub url: String,
    /// Document title.
    pub title: String,
}

/// Compact accessibility node from `browser.snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxNode {
    /// Stable uid for click/fill (also stamped as `data-turbo-uid`).
    pub uid: String,
    /// AX role (`link`, `textbox`, …).
    pub role: String,
    /// Accessible name.
    pub name: String,
    /// Current value, when applicable.
    pub value: Option<String>,
    /// Whether this node is focused.
    pub focused: bool,
}

/// Where a snapshot's nodes came from.
///
/// This is load-bearing, not metadata: only [`SnapshotSource::Dom`] uids are
/// real `data-turbo-uid` attributes. Fallback uids are numbered over a
/// *different* node set, so clicking one would hit an unrelated element.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSource {
    /// Injected collector; uids are actionable.
    #[default]
    Dom,
    /// CDP `Accessibility.getFullAXTree`; uids are **not** actionable.
    AxFallback,
}

impl SnapshotSource {
    /// Whether `click` / `fill` may use uids from this snapshot.
    pub fn uids_are_actionable(self) -> bool {
        matches!(self, Self::Dom)
    }
}

/// Result of `browser.snapshot`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotResult {
    /// Page URL.
    pub url: String,
    /// Document title.
    pub title: String,
    /// Which collector produced `nodes` (see [`SnapshotSource`]).
    #[serde(default)]
    pub source: SnapshotSource,
    /// Compact AX nodes.
    pub nodes: Vec<AxNode>,
    /// True when a dialog / modal overlay is in the snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<bool>,
    /// Truncated main-landmark text when `include_text` was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Result of `browser.click`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClickResult {
    /// Page URL after the click (and any committed navigation).
    pub url: String,
    /// Document title after the click.
    pub title: String,
}

/// Result of `browser.wait`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitResult {
    /// Page URL when the wait succeeded.
    pub url: String,
    /// Document title when the wait succeeded.
    pub title: String,
}

/// One file in the session-scoped brokered download directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadInfo {
    /// Sanitized file name.
    pub name: String,
    /// Absolute path assigned by the browser host.
    pub path: String,
    /// Current file size in bytes.
    pub bytes: u64,
    /// True when the file is a regular completed file visible to the host.
    pub completed: bool,
}

/// Result of `browser.downloads`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadsResult {
    /// Session-scoped files under `<session>/downloads`.
    pub downloads: Vec<DownloadInfo>,
}

/// Result of `browser.screenshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotResult {
    /// PNG path on disk (session images dir).
    pub path: String,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
}

/// One tab entry from `browser.tabs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    /// Host-assigned tab id.
    pub tab_id: u32,
    /// Current URL.
    pub url: String,
    /// Document title.
    pub title: String,
    /// Whether this tab is selected.
    pub active: bool,
}

/// Result of `browser.tabs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabsResult {
    /// Open tabs (v1 may be a single entry).
    pub tabs: Vec<TabInfo>,
}

/// Host → client event kinds (`loaded`, `crashed`, `closed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BrowserEvent {
    /// Navigation finished.
    Loaded { url: String, title: String },
    /// Renderer / WebView crashed.
    Crashed { message: String },
    /// Host window closed.
    Closed,
}

/// Envelope or method-decode failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// Not a known `browser.*` method.
    #[error("unknown browser method: {0}")]
    UnknownMethod(String),
    /// Params did not match the method schema.
    #[error("invalid params: {0}")]
    InvalidParams(String),
}

/// Navigation URL policy failure (fail closed).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlPolicyError {
    /// Empty or whitespace-only URL.
    #[error("URL is empty")]
    Empty,
    /// Not a usable URL.
    #[error("invalid URL")]
    Invalid,
    /// Scheme is not on the allow list.
    #[error("URL scheme `{0}` is not allowed")]
    SchemeDenied(String),
    /// `http:` host is not loopback, RFC1918, or `*.localhost`.
    #[error("http URL host `{0}` is not loopback, RFC1918, or *.localhost")]
    HostDenied(String),
    /// `file:` is denied unless a session-folder exception applies.
    #[error("file: URLs are not allowed")]
    FileDenied,
    /// `file:` resolved outside the session folder.
    #[error("file: URL is not under the session folder")]
    FileOutsideSession,
    /// Embedded userinfo can carry secrets.
    #[error("URLs with userinfo are not allowed")]
    UserinfoDenied,
    /// Host is outside `GROK_BROWSER_ALLOW` (fail closed when the list is set).
    #[error("URL host `{0}` is not on GROK_BROWSER_ALLOW")]
    AllowlistDenied(String),
}

/// `browser.fill` policy failure (fail closed).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FillPolicyError {
    /// Digit-only 6–8 character OTP / PIN.
    #[error("fill value looks like a one-time password")]
    OtpShaped,
    /// High-complexity secret or password-named field.
    #[error("fill value looks like a password or recovery secret")]
    PasswordShaped,
    /// The target field itself is a credential input, whatever the value is.
    #[error(
        "target field is a {kind} input; the human signs in themselves in the Agent Browser window"
    )]
    SecretField {
        /// `password`, `one-time-code`, or `payment`.
        kind: String,
    },
}

/// What the page says the fill target is, from the injected `lookup`.
///
/// `secret` is authoritative: `<input type="password">` is a credential field
/// even when it has no label and the value looks innocuous.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FillTarget<'a> {
    /// Accessible name, if the page exposes one.
    pub name: Option<&'a str>,
    /// `password` / `one-time-code` / `payment` from `type` + `autocomplete`.
    pub secret: Option<&'a str>,
}

/// `browser.eval` policy failure (fail closed).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvalPolicyError {
    /// Result exceeded [`EVAL_RESULT_MAX_BYTES`].
    #[error("eval result exceeds {EVAL_RESULT_MAX_BYTES} bytes ({len} bytes)")]
    ResultTooLarge {
        /// Observed size in bytes.
        len: usize,
    },
    /// Mutating script without `confirm=true`.
    #[error(
        "eval writes to the page (click / submit / navigate / assign); retry with confirm=true"
    )]
    NeedsConfirm,
}

/// Optional comma-separated host allowlist (`example.com` allows `www.example.com`).
///
/// Unset or empty keeps the default scheme policy (https + local http + `about:blank`).
/// Non-empty is fail-closed for `http:` / `https:` hosts outside the list.
pub const GROK_BROWSER_ALLOW_ENV: &str = "GROK_BROWSER_ALLOW";

/// Allow `https:`, local `http:`, and `about:blank`. Deny `file:` by default.
pub fn check_url(url: &str) -> Result<(), UrlPolicyError> {
    check_url_in_session(url, None)
}

/// Fail-closed hop check used by `NavigationStarting`, `FrameNavigationStarting`,
/// `NewWindowRequested`, and `browser.navigate`.
///
/// A missing or empty URI is cancelled: failing open when WebView2 cannot
/// report the destination would let a redirect, iframe, or click walk out of
/// the allowlist. All three hop kinds share this function so a gap in one
/// event cannot bypass the others.
pub fn check_navigation_hop(
    uri: Option<&str>,
    session_folder: Option<&Path>,
) -> Result<(), UrlPolicyError> {
    let Some(url) = uri.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(UrlPolicyError::Empty);
    };
    check_url_in_session(url, session_folder)
}

/// JSON-RPC / client error when a multi-tab method is requested.
pub fn single_tab_v1_error(method: &str) -> String {
    format!("{method} is not implemented (v1 is a single tab)")
}

/// Like [`check_url`], but `file:` under `session_folder` may be allowed.
pub fn check_url_in_session(
    url: &str,
    session_folder: Option<&Path>,
) -> Result<(), UrlPolicyError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(UrlPolicyError::Empty);
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(UrlPolicyError::Invalid);
    }

    let Some((scheme, rest)) = split_scheme(url) else {
        return Err(UrlPolicyError::Invalid);
    };
    let scheme_lc = scheme.to_ascii_lowercase();

    match scheme_lc.as_str() {
        "https" => check_http_family(rest, true),
        "http" => check_http_family(rest, false),
        "about" => check_about(rest),
        "file" => check_file(rest, session_folder),
        other => Err(UrlPolicyError::SchemeDenied(other.to_owned())),
    }
}

/// Reject OTP-shaped and obvious password-shaped fill values.
pub fn check_fill_value(value: &str) -> Result<(), FillPolicyError> {
    check_fill(value, None)
}

/// Fill policy, optionally using the target field's accessible name / role.
pub fn check_fill(value: &str, field_name: Option<&str>) -> Result<(), FillPolicyError> {
    check_fill_target(
        value,
        &FillTarget {
            name: field_name,
            secret: None,
        },
    )
}

/// Fill policy against a resolved page field (see [`FillTarget`]).
pub fn check_fill_target(value: &str, target: &FillTarget<'_>) -> Result<(), FillPolicyError> {
    if let Some(kind) = target.secret.map(str::trim).filter(|k| !k.is_empty()) {
        return Err(FillPolicyError::SecretField {
            kind: kind.to_ascii_lowercase(),
        });
    }
    if field_looks_secret(target.name) {
        return Err(FillPolicyError::PasswordShaped);
    }
    let trimmed = value.trim();
    if is_otp_shaped(trimmed) {
        return Err(FillPolicyError::OtpShaped);
    }
    if is_password_shaped(trimmed) {
        return Err(FillPolicyError::PasswordShaped);
    }
    Ok(())
}

/// Fail closed when an eval JSON result exceeds [`EVAL_RESULT_MAX_BYTES`].
pub fn check_eval_result(result: &str) -> Result<(), EvalPolicyError> {
    check_eval_result_len(result.len())
}

/// Same as [`check_eval_result`] from a raw byte length.
pub fn check_eval_result_len(len: usize) -> Result<(), EvalPolicyError> {
    if len > EVAL_RESULT_MAX_BYTES {
        Err(EvalPolicyError::ResultTooLarge { len })
    } else {
        Ok(())
    }
}

/// Exact https origins allowed to keep a real WebView2 OAuth popup.
///
/// Substring matching (`ux_mode=popup`, `/oauth/authorize` anywhere) opened an
/// unpolicied window for attacker-controlled URLs. Host must match exactly.
const OAUTH_POPUP_HOSTS: &[&str] = &[
    "accounts.google.com",
    "login.microsoftonline.com",
    "login.live.com",
    "appleid.apple.com",
];

/// OAuth / Google Identity Services popup URLs that must not replace the only tab.
///
/// Exact-origin only: `https://accounts.google.com/...` is a popup;
/// `https://evil.test/accounts.google.com/gsi` is not.
pub fn is_oauth_popup_url(url: &str) -> bool {
    oauth_popup_host(url).is_some()
}

/// Host of an allowlisted https OAuth popup origin, if `url` is one.
pub fn oauth_popup_host(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = split_scheme(url)?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let after = strip_authority_slashes(rest)?;
    let authority_end = after.find(AUTHORITY_END).unwrap_or(after.len());
    let authority = &after[..authority_end];
    if authority.contains('@') || authority.contains('%') {
        return None;
    }
    let host = parse_http_host(authority).ok()?;
    let host_lc = host.trim_end_matches('.').to_ascii_lowercase();
    OAUTH_POPUP_HOSTS
        .iter()
        .copied()
        .find(|allowed| host_lc == *allowed)
        .map(str::to_owned)
}

/// True when `path` is the session folder or a descendant after canonicalize.
///
/// Fail closed: unresolvable paths are outside. Windows compare is
/// case-insensitive and uses path components so `C:\work` does not match
/// `C:\work-evil`.
pub fn path_is_under_session_folder(path: &Path, session_folder: &Path) -> bool {
    let Ok(folder) = dunce::canonicalize(session_folder) else {
        return false;
    };
    let Ok(path) = dunce::canonicalize(path) else {
        return false;
    };
    path_components_under(&path, &folder)
}

fn path_components_under(path: &Path, root: &Path) -> bool {
    let path_c: Vec<Cow<'_, str>> = path.components().map(component_key).collect();
    let root_c: Vec<Cow<'_, str>> = root.components().map(component_key).collect();
    path_c.starts_with(&root_c)
}

fn component_key(component: Component<'_>) -> Cow<'_, str> {
    let raw = component.as_os_str().to_string_lossy();
    if cfg!(windows) {
        Cow::Owned(raw.to_ascii_lowercase())
    } else {
        raw
    }
}

/// Whether a `browser.eval` function expression looks like it mutates the page.
///
/// Assignment / call forms only: a read of `location.href` is not a write.
pub fn eval_looks_mutating(function: &str) -> bool {
    let f = function.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        ".click(",
        ".click (",
        "['click']",
        "[\"click\"]",
        ".submit(",
        ".submit (",
        ".focus(",
        "location =",
        "location=",
        "location.href=",
        "location.href =",
        "location.replace",
        "location.assign",
        "window.open",
        ".value =",
        ".value=",
        ".value+=",
        ".value +=",
        ".src=",
        ".src =",
        "setattribute",
        "removeattribute",
        "innerhtml",
        "outerhtml",
        "innertext =",
        "textcontent =",
        ".remove(",
        ".appendchild",
        ".insertadjacent",
        "dispatchevent",
        "requestsubmit",
        "fetch(",
        "xmlhttprequest",
        "navigator.sendbeacon",
        "localstorage",
        "sessionstorage",
        "document.cookie",
        "document.write",
        "history.pushstate",
        "history.replacestate",
        "history.back",
        "history.go",
        "history.forward",
        "['submit']",
        "[\"submit\"]",
        "new function",
        "function(\"",
        "function('",
        "function(`",
    ];
    NEEDLES.iter().any(|needle| f.contains(needle))
}

/// Require `confirm=true` for a mutating `browser.eval` expression.
pub fn check_eval_confirm(function: &str, confirm: bool) -> Result<(), EvalPolicyError> {
    if confirm || !eval_looks_mutating(function) {
        Ok(())
    } else {
        Err(EvalPolicyError::NeedsConfirm)
    }
}

fn split_scheme(url: &str) -> Option<(&str, &str)> {
    let idx = url.find(':')?;
    let scheme = &url[..idx];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '+' || c == '.' || c == '-')
    {
        return None;
    }
    Some((scheme, &url[idx + 1..]))
}

fn check_about(rest: &str) -> Result<(), UrlPolicyError> {
    let rest_lc = rest.to_ascii_lowercase();
    if rest_lc == "blank" || rest_lc.starts_with("blank#") {
        Ok(())
    } else {
        Err(UrlPolicyError::SchemeDenied("about".to_owned()))
    }
}

/// Authority terminators for a special scheme.
///
/// WHATWG treats `\` exactly like `/` here, so `https://evil.test\.example.com/`
/// has host `evil.test` — not `evil.test\.example.com`. Splitting on `/` alone
/// let a backslash smuggle an allowlisted suffix past [`check_host_allowlist`]
/// and an `.localhost` suffix past [`is_allowed_http_host`].
const AUTHORITY_END: [char; 4] = ['/', '?', '#', '\\'];

/// Strip the `//` after a special scheme. WHATWG accepts any mix of `/` and
/// `\` (and tolerates extra ones), so `https:\\host` is `https://host`.
fn strip_authority_slashes(rest: &str) -> Option<&str> {
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    let (_, second) = chars.next()?;
    if !matches!(first, '/' | '\\') || !matches!(second, '/' | '\\') {
        return None;
    }
    Some(rest[2..].trim_start_matches(['/', '\\']))
}

fn check_http_family(rest: &str, https: bool) -> Result<(), UrlPolicyError> {
    let Some(after_slashes) = strip_authority_slashes(rest) else {
        return Err(UrlPolicyError::Invalid);
    };
    let authority_end = after_slashes
        .find(AUTHORITY_END)
        .unwrap_or(after_slashes.len());
    let authority = &after_slashes[..authority_end];
    if authority.is_empty() {
        return Err(UrlPolicyError::Invalid);
    }
    if authority.contains('@') {
        return Err(UrlPolicyError::UserinfoDenied);
    }
    if authority.contains('%') {
        return Err(UrlPolicyError::Invalid);
    }
    let host = parse_http_host(authority)?;
    if !https && !is_allowed_http_host(&host) {
        return Err(UrlPolicyError::HostDenied(host));
    }
    check_host_allowlist(&host)
}

/// Parsed `GROK_BROWSER_ALLOW` entries (lowercase, trimmed). Empty = no extra filter.
pub fn browser_allowlist() -> Vec<String> {
    parse_browser_allow(&browser_allow_raw().unwrap_or_default())
}

fn browser_allow_raw() -> Option<String> {
    #[cfg(test)]
    {
        // Lib tests ignore ambient env so they cannot leak across `--test-threads`.
        return TEST_ALLOW.with(|cell| cell.borrow().clone());
    }
    #[cfg(not(test))]
    std::env::var(GROK_BROWSER_ALLOW_ENV).ok()
}

fn parse_browser_allow(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|part| {
            let entry = normalize_allow_entry(part);
            if entry.is_empty() { None } else { Some(entry) }
        })
        .collect()
}

fn normalize_allow_entry(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed.as_str());
    let host_port = without_scheme
        .split(AUTHORITY_END)
        .next()
        .unwrap_or(without_scheme);
    let host_port = host_port.strip_prefix("*.").unwrap_or(host_port);
    let host = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            h
        }
        _ => host_port,
    };
    host.trim_end_matches('.').to_owned()
}

fn check_host_allowlist(host: &str) -> Result<(), UrlPolicyError> {
    let allow = browser_allowlist();
    if allow.is_empty() {
        return Ok(());
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host_matches_allowlist(&host, &allow) {
        Ok(())
    } else {
        Err(UrlPolicyError::AllowlistDenied(host))
    }
}

fn host_matches_allowlist(host: &str, allow: &[String]) -> bool {
    allow.iter().any(|entry| {
        host == entry
            || host
                .strip_suffix(entry.as_str())
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

// Thread-local, not a global behind a lock: the harness runs tests on separate
// threads, and a global override leaks into every test that reads the allowlist
// concurrently — a lock around the writer does not stop unlocked readers.
#[cfg(test)]
thread_local! {
    static TEST_ALLOW: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Install `GROK_BROWSER_ALLOW` for the duration of `f` on this thread (tests only).
#[cfg(test)]
pub(crate) fn with_browser_allow<R>(allow: &str, f: impl FnOnce() -> R) -> R {
    TEST_ALLOW.with(|cell| *cell.borrow_mut() = Some(allow.to_owned()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    TEST_ALLOW.with(|cell| *cell.borrow_mut() = None);
    match result {
        Ok(r) => r,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn parse_http_host(authority: &str) -> Result<String, UrlPolicyError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or(UrlPolicyError::Invalid)?;
        let host = rest[..end].to_string();
        if host.is_empty() {
            return Err(UrlPolicyError::Invalid);
        }
        let after = &rest[end + 1..];
        if !after.is_empty() {
            let Some(port) = after.strip_prefix(':') else {
                return Err(UrlPolicyError::Invalid);
            };
            if port.parse::<u16>().is_err() {
                return Err(UrlPolicyError::Invalid);
            }
        }
        return Ok(host);
    }
    let host = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            if p.parse::<u16>().is_err() {
                return Err(UrlPolicyError::Invalid);
            }
            h
        }
        _ => authority,
    };
    if host.is_empty() || host.contains(':') || host.contains('\\') {
        return Err(UrlPolicyError::Invalid);
    }
    Ok(host.to_string())
}

fn is_allowed_http_host(host: &str) -> bool {
    if is_localhost_name(host) {
        return true;
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return is_allowed_v4(ip);
    }
    if let Ok(ip) = host.parse::<Ipv6Addr>() {
        if ip.is_loopback() {
            return true;
        }
        // `::ffff:127.0.0.1` is loopback to every resolver but not to
        // `Ipv6Addr::is_loopback`.
        if let Some(v4) = ip.to_ipv4_mapped() {
            return is_allowed_v4(v4);
        }
        return false;
    }
    false
}

fn is_allowed_v4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || is_rfc1918(ip)
}

fn is_localhost_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost")
}

fn is_rfc1918(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168)
}

fn check_file(rest: &str, session_folder: Option<&Path>) -> Result<(), UrlPolicyError> {
    let Some(folder) = session_folder else {
        return Err(UrlPolicyError::FileDenied);
    };
    let path = file_url_to_path(rest).ok_or(UrlPolicyError::Invalid)?;
    let path = normalize_lexically(&path);
    let folder = normalize_lexically(folder);
    if path_is_within(&path, &folder) {
        Ok(())
    } else {
        Err(UrlPolicyError::FileOutsideSession)
    }
}

fn file_url_to_path(after_scheme: &str) -> Option<PathBuf> {
    let without_qf = after_scheme
        .split_once(['?', '#'])
        .map(|(p, _)| p)
        .unwrap_or(after_scheme);
    let path_part = if let Some(rest) = without_qf.strip_prefix("//") {
        if let Some(abs) = rest.strip_prefix('/') {
            // file:///C:/... or file:///tmp/...
            format!("/{abs}")
        } else {
            let slash = rest.find('/')?;
            let host = &rest[..slash];
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return None;
            }
            rest[slash..].to_owned()
        }
    } else if without_qf.starts_with('/') {
        without_qf.to_owned()
    } else {
        return None;
    };
    let decoded = percent_decode(&path_part)?;
    if decoded.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(windows_or_unix_path(&decoded))
}

fn windows_or_unix_path(decoded: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let trimmed = decoded.strip_prefix('/').unwrap_or(decoded);
        if trimmed.len() >= 2 {
            let b = trimmed.as_bytes();
            if b[0].is_ascii_alphabetic() && b[1] == b':' {
                return PathBuf::from(trimmed.replace('/', "\\"));
            }
        }
        PathBuf::from(decoded.replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(decoded)
    }
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = normalize_lexically(path);
    let root = normalize_lexically(root);
    #[cfg(windows)]
    {
        // Windows paths are case-insensitive; `Path::starts_with` is not. A
        // lowercase drive letter from WebView2 must not read as an escape.
        let mut p = path.components();
        for r in root.components() {
            let Some(seg) = p.next() else {
                return false;
            };
            if !seg.as_os_str().eq_ignore_ascii_case(r.as_os_str()) {
                return false;
            }
        }
        true
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

fn field_looks_secret(field_name: Option<&str>) -> bool {
    let Some(name) = field_name else {
        return false;
    };
    let n = name.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "password",
        "passwd",
        "passcode",
        "secret",
        "otp",
        "totp",
        "2fa",
        "two-factor",
        "two factor",
        "recovery",
        "one-time",
    ];
    NEEDLES.iter().any(|k| n.contains(k))
}

fn is_otp_shaped(value: &str) -> bool {
    let digits: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (6..=8).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit())
}

fn is_password_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.contains("password") || lower.contains("p@ssw") || lower.contains("passwd") {
        return true;
    }
    if looks_like_email(value) || looks_like_http_url(value) {
        return false;
    }
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    if value.len() < 8 || value.len() > 128 {
        return false;
    }
    let has_upper = value.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = value.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = value.chars().any(|c| c.is_ascii_digit());
    let has_special = value.chars().any(|c| !c.is_ascii_alphanumeric());
    has_upper && has_lower && has_digit && has_special
}

fn looks_like_email(value: &str) -> bool {
    let Some((user, domain)) = value.split_once('@') else {
        return false;
    };
    !user.is_empty() && domain.contains('.') && !domain.contains(' ')
}

fn looks_like_http_url(value: &str) -> bool {
    let v = value.to_ascii_lowercase();
    v.starts_with("https://") || v.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigate_roundtrip() {
        let v = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "browser.navigate",
            "params": { "url": "https://example.com/" }
        });
        let env: JsonRpcRequest = serde_json::from_value(v).unwrap();
        assert_eq!(env.method, "browser.navigate");
        match env.browser_request().unwrap() {
            BrowserRequest::Navigate { url } => assert_eq!(url, "https://example.com/"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn url_policy_allows_https_and_local_http() {
        for url in [
            "https://example.com/",
            "HTTPS://Example.COM/path?q=1",
            "http://127.0.0.1/",
            "http://127.0.0.1:8080/status",
            "http://localhost",
            "http://localhost:3000/",
            "http://app.localhost/dash",
            "http://10.0.0.5/",
            "http://192.168.1.1/admin",
            "http://172.16.0.2/",
            "http://172.31.255.1/",
            "http://[::1]/",
            "about:blank",
            "about:blank#ready",
        ] {
            assert!(check_url(url).is_ok(), "expected allow: {url}");
        }
    }

    #[test]
    fn url_policy_denies_file_and_javascript() {
        for url in [
            "file:///C:/Windows/notepad.exe",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "JAVASCRIPT:void(0)",
            "data:text/html,hi",
            "data:text/html;base64,AAAA",
        ] {
            assert!(check_url(url).is_err(), "expected deny: {url}");
        }
    }

    #[test]
    fn empty_allow_env_allows_https_example() {
        assert!(check_url("https://example.com").is_ok());
        assert!(browser_allowlist().is_empty());
    }

    #[test]
    fn grok_browser_allow_is_fail_closed() {
        with_browser_allow("example.com", || {
            assert!(check_url("https://example.com/").is_ok());
            assert!(check_url("https://www.example.com/path").is_ok());
            assert_eq!(
                check_url("https://evil.test/"),
                Err(UrlPolicyError::AllowlistDenied("evil.test".into()))
            );
            // Public http is still denied by the local-http rule first.
            assert!(matches!(
                check_url("http://example.com/"),
                Err(UrlPolicyError::HostDenied(_))
            ));
        });
    }

    #[test]
    fn allowlist_matches_subdomains_not_suffix_spoofs() {
        assert!(host_matches_allowlist(
            "www.example.com",
            &["example.com".into()]
        ));
        assert!(!host_matches_allowlist(
            "evil-example.com",
            &["example.com".into()]
        ));
        assert!(!host_matches_allowlist(
            "example.com.evil.test",
            &["example.com".into()]
        ));
    }

    #[test]
    fn backslash_in_authority_cannot_smuggle_a_host() {
        // WHATWG parses `\` as `/`, so the real host of each of these is the
        // segment BEFORE the backslash. Splitting only on `/` used to hand the
        // whole string to the allowlist / localhost checks.
        assert!(matches!(
            check_url(r"http://evil.test\.localhost/"),
            Err(UrlPolicyError::HostDenied(_))
        ));
        assert!(matches!(
            check_url(r"http://evil.test\.example.localhost/x"),
            Err(UrlPolicyError::HostDenied(_))
        ));
        with_browser_allow("example.com", || {
            assert_eq!(
                check_url(r"https://evil.test\.example.com/"),
                Err(UrlPolicyError::AllowlistDenied("evil.test".into())),
                "backslash must not smuggle an allowlisted suffix"
            );
            assert_eq!(
                check_url(r"https://evil.test\@example.com/"),
                Err(UrlPolicyError::AllowlistDenied("evil.test".into()))
            );
            // Backslash-form slashes still resolve to the same real host.
            assert!(check_url(r"https:\\example.com\path").is_ok());
        });
    }

    #[test]
    fn ipv4_mapped_loopback_is_local_http() {
        assert!(check_url("http://[::ffff:127.0.0.1]/").is_ok());
        assert!(check_url("http://[::ffff:192.168.1.4]/").is_ok());
        assert!(matches!(
            check_url("http://[::ffff:8.8.8.8]/"),
            Err(UrlPolicyError::HostDenied(_))
        ));
    }

    #[test]
    fn fill_refuses_credential_fields_whatever_the_value() {
        for kind in ["password", "one-time-code", "payment"] {
            let err = check_fill_target(
                "just some text",
                &FillTarget {
                    name: Some("Login"),
                    secret: Some(kind),
                },
            )
            .unwrap_err();
            assert_eq!(
                err,
                FillPolicyError::SecretField {
                    kind: kind.to_owned()
                }
            );
        }
        // An unlabeled password box used to pass: no secret-shaped name, and
        // `hunter2hunter2` has no uppercase or symbol.
        assert!(check_fill_value("hunter2hunter2").is_ok());
        assert!(
            check_fill_target(
                "hunter2hunter2",
                &FillTarget {
                    name: None,
                    secret: Some("password")
                }
            )
            .is_err()
        );
        assert!(
            check_fill_target(
                "acme search query",
                &FillTarget {
                    name: Some("Search"),
                    secret: None
                }
            )
            .is_ok()
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_session_folder_compare_is_case_insensitive() {
        let folder = PathBuf::from(r"C:\tmp\Session-ABC");
        assert!(
            check_url_in_session("file:///c:/TMP/session-abc/page.html", Some(&folder)).is_ok(),
            "drive-letter / segment case must not read as an escape"
        );
        assert_eq!(
            check_url_in_session("file:///c:/tmp/session-abc-evil/page.html", Some(&folder)),
            Err(UrlPolicyError::FileOutsideSession)
        );
    }

    #[test]
    fn url_policy_denies_public_http_and_userinfo() {
        assert!(matches!(
            check_url("http://example.com/"),
            Err(UrlPolicyError::HostDenied(_))
        ));
        assert!(matches!(
            check_url("http://172.15.0.1/"),
            Err(UrlPolicyError::HostDenied(_))
        ));
        assert_eq!(
            check_url("http://user:pass@127.0.0.1/"),
            Err(UrlPolicyError::UserinfoDenied)
        );
        assert!(check_url("about:srcdoc").is_err());
        assert!(check_url("blob:https://example.com/uuid").is_err());
        assert!(check_url("ws://127.0.0.1/").is_err());
    }

    #[test]
    fn url_policy_file_session_folder_exception() {
        let folder = if cfg!(windows) {
            PathBuf::from(r"C:\tmp\session-abc")
        } else {
            PathBuf::from("/tmp/session-abc")
        };
        let ok = if cfg!(windows) {
            "file:///C:/tmp/session-abc/page.html"
        } else {
            "file:///tmp/session-abc/page.html"
        };
        let escape = if cfg!(windows) {
            "file:///C:/tmp/session-abc/../Windows/notepad.exe"
        } else {
            "file:///tmp/session-abc/../etc/passwd"
        };
        let sibling = if cfg!(windows) {
            "file:///C:/tmp/session-abc-evil/page.html"
        } else {
            "file:///tmp/session-abc-evil/page.html"
        };
        assert!(
            check_url_in_session(ok, Some(&folder)).is_ok(),
            "in-folder file should be allowed"
        );
        assert_eq!(check_url(ok), Err(UrlPolicyError::FileDenied));
        assert_eq!(
            check_url_in_session(escape, Some(&folder)),
            Err(UrlPolicyError::FileOutsideSession)
        );
        assert_eq!(
            check_url_in_session(sibling, Some(&folder)),
            Err(UrlPolicyError::FileOutsideSession)
        );
    }

    #[test]
    fn fill_rejects_otp_and_password_shaped_values() {
        assert_eq!(check_fill_value("123456"), Err(FillPolicyError::OtpShaped));
        assert_eq!(
            check_fill_value("84729183"),
            Err(FillPolicyError::OtpShaped)
        );
        assert_eq!(
            check_fill_value("12 34 56"),
            Err(FillPolicyError::OtpShaped)
        );
        assert_eq!(
            check_fill_value("P@ssw0rd!"),
            Err(FillPolicyError::PasswordShaped)
        );
        assert_eq!(
            check_fill_value("MyPassword1"),
            Err(FillPolicyError::PasswordShaped)
        );
        assert!(check_fill_value("The quick brown fox jumps.").is_ok());
        assert!(check_fill_value("user@example.com").is_ok());
        assert!(check_fill_value("Acme search query").is_ok());
        assert_eq!(
            check_fill("hello", Some("Password")),
            Err(FillPolicyError::PasswordShaped)
        );
    }

    #[test]
    fn eval_result_cap_is_20000_bytes() {
        assert_eq!(EVAL_RESULT_MAX_BYTES, 20_000);
        assert!(check_eval_result(&"x".repeat(20_000)).is_ok());
        assert!(matches!(
            check_eval_result(&"x".repeat(20_001)),
            Err(EvalPolicyError::ResultTooLarge { len: 20_001 })
        ));
    }

    #[test]
    fn event_roundtrip_kinds() {
        let loaded = JsonRpcEvent::browser(BrowserEvent::Loaded {
            url: "https://example.com/".into(),
            title: "Example".into(),
        });
        let v = serde_json::to_value(&loaded).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["method"], "browser.event");
        assert_eq!(v["kind"], serde_json::Value::Null);
        assert_eq!(v["params"]["kind"], "loaded");
        assert!(!v.as_object().unwrap().contains_key("id"));

        let crashed: JsonRpcEvent = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "browser.event",
            "params": { "kind": "crashed", "message": "oom" }
        }))
        .unwrap();
        assert_eq!(
            crashed.params,
            BrowserEvent::Crashed {
                message: "oom".into()
            }
        );

        let closed: JsonRpcEvent = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "browser.event",
            "params": { "kind": "closed" }
        }))
        .unwrap();
        assert_eq!(closed.params, BrowserEvent::Closed);
    }

    #[test]
    fn result_types_roundtrip() {
        let snap = SnapshotResult {
            url: "https://example.com/".into(),
            title: "Example".into(),
            source: SnapshotSource::Dom,
            nodes: vec![AxNode {
                uid: "1-1".into(),
                role: "link".into(),
                name: "More information".into(),
                value: None,
                focused: false,
            }],
            ..Default::default()
        };
        let v = serde_json::to_value(&snap).unwrap();
        let back: SnapshotResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.nodes[0].uid, "1-1");

        let downloads = DownloadsResult {
            downloads: vec![DownloadInfo {
                name: "report.pdf".into(),
                path: "downloads/report.pdf".into(),
                bytes: 12,
                completed: true,
            }],
        };
        let v = serde_json::to_value(&downloads).unwrap();
        let back: DownloadsResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.downloads[0].name, "report.pdf");

        let shot = ScreenshotResult {
            path: "images/browser-1.png".into(),
            width: 1280,
            height: 800,
        };
        let v = serde_json::to_value(&shot).unwrap();
        assert_eq!(v["width"], 1280);
    }

    #[test]
    fn all_browser_methods_decode() {
        let cases = [
            (
                "browser.navigate",
                serde_json::json!({"url": "https://example.com/"}),
            ),
            ("browser.tabs", serde_json::json!({})),
            ("browser.downloads", serde_json::json!({})),
            ("browser.new_tab", serde_json::json!({"url": null})),
            ("browser.select_tab", serde_json::json!({"tab_id": 1})),
            ("browser.close_tab", serde_json::json!({"tab_id": 1})),
            ("browser.snapshot", serde_json::json!({"verbose": false})),
            ("browser.click", serde_json::json!({"uid": "1"})),
            (
                "browser.fill",
                serde_json::json!({"uid": "2", "value": "hello"}),
            ),
            (
                "browser.eval",
                serde_json::json!({"function": "() => document.title"}),
            ),
            ("browser.screenshot", serde_json::json!({})),
            ("browser.raise", serde_json::json!({})),
            ("browser.shutdown", serde_json::json!({})),
            (
                "browser.wait",
                serde_json::json!({"text": "jobs", "timeout_ms": 5000}),
            ),
            ("browser.scroll", serde_json::json!({"dy": 400})),
            ("browser.press_key", serde_json::json!({"key": "Enter"})),
            (
                "browser.select",
                serde_json::json!({"uid": "1-1", "value": "Remote"}),
            ),
            ("browser.hover", serde_json::json!({"uid": "1-1"})),
            (
                "browser.set_file",
                serde_json::json!({"uid": "1-2", "path": "resume.pdf"}),
            ),
        ];
        for (method, params) in cases {
            let env = JsonRpcRequest {
                jsonrpc: JsonRpcVersion,
                id: JsonRpcId::Number(1),
                method: method.to_owned(),
                params,
            };
            env.browser_request()
                .unwrap_or_else(|e| panic!("{method}: {e}"));
            assert_eq!(BrowserMethod::from_str(method).unwrap().as_str(), method);
        }
    }

    #[test]
    fn new_tab_empty_or_omitted_params_decode_to_no_url() {
        let omitted: JsonRpcRequest = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "browser.new_tab"
        }))
        .unwrap();
        assert_eq!(omitted.params, Value::Null);
        assert_eq!(
            omitted.browser_request().unwrap(),
            BrowserRequest::NewTab { url: None }
        );

        let empty: JsonRpcRequest = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "browser.new_tab",
            "params": {}
        }))
        .unwrap();
        assert_eq!(
            empty.browser_request().unwrap(),
            BrowserRequest::NewTab { url: None }
        );
    }

    #[test]
    fn navigation_starting_cancels_disallowed_hops() {
        // Redirect, iframe, and click-driven hops all share this check.
        for url in [
            "javascript:alert(1)",
            "data:text/html,hi",
            "http://example.com/",
            "file:///C:/Windows/notepad.exe",
            "blob:https://example.com/uuid",
            "about:srcdoc",
        ] {
            assert!(
                check_navigation_hop(Some(url), None).is_err(),
                "hop must cancel: {url}"
            );
        }
        assert_eq!(
            check_navigation_hop(None, None),
            Err(UrlPolicyError::Empty),
            "missing URI must fail closed, not allow the hop"
        );
        assert_eq!(
            check_navigation_hop(Some(""), None),
            Err(UrlPolicyError::Empty)
        );
        assert_eq!(
            check_navigation_hop(Some("   "), None),
            Err(UrlPolicyError::Empty)
        );
        assert!(check_navigation_hop(Some("https://example.com/next"), None).is_ok());
        assert!(check_navigation_hop(Some("about:blank"), None).is_ok());
        assert!(check_navigation_hop(Some("http://127.0.0.1/"), None).is_ok());
        with_browser_allow("example.com", || {
            assert!(check_navigation_hop(Some("https://example.com/"), None).is_ok());
            assert_eq!(
                check_navigation_hop(Some("https://evil.test/"), None),
                Err(UrlPolicyError::AllowlistDenied("evil.test".into()))
            );
        });
    }

    #[test]
    fn location_href_read_is_not_mutating() {
        assert!(!eval_looks_mutating(
            "() => ({ url: location.href, title: document.title })"
        ));
        assert!(!eval_looks_mutating("() => location.href"));
        assert!(eval_looks_mutating(
            "() => location.href = 'https://evil.test'"
        ));
        assert!(eval_looks_mutating(
            "() => location.href='https://evil.test'"
        ));
        assert!(eval_looks_mutating(
            "() => location.replace('https://evil.test')"
        ));
        assert!(eval_looks_mutating("() => el['click']()"));
        assert!(eval_looks_mutating("() => document.write('x')"));
        assert!(eval_looks_mutating(
            "() => img.src = 'https://evil.test/x.png'"
        ));
        assert!(!eval_looks_mutating(
            "() => document.querySelectorAll('a').length"
        ));
        assert!(eval_looks_mutating("() => document.forms[0]['submit']()"));
        assert!(eval_looks_mutating("() => history.back()"));
        assert!(eval_looks_mutating(
            "() => location.assign('https://evil.test')"
        ));
        assert!(check_eval_confirm("() => document.title", false).is_ok());
        assert_eq!(
            check_eval_confirm("() => document.forms[0].submit()", false),
            Err(EvalPolicyError::NeedsConfirm)
        );
        assert!(check_eval_confirm("() => document.forms[0].submit()", true).is_ok());
    }

    #[test]
    fn oauth_popup_urls_are_detected() {
        assert!(is_oauth_popup_url(
            "https://accounts.google.com/gsi/select?ux_mode=popup&origin=https://www.linkedin.com/"
        ));
        assert!(is_oauth_popup_url(
            "https://accounts.google.com/o/oauth2/v2/auth?client_id=1"
        ));
        assert!(is_oauth_popup_url(
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
        ));
        assert!(is_oauth_popup_url(
            "https://login.live.com/oauth20_authorize.srf"
        ));
        assert!(is_oauth_popup_url(
            "https://appleid.apple.com/auth/authorize"
        ));
        assert!(!is_oauth_popup_url("https://www.linkedin.com/login"));
        assert!(!is_oauth_popup_url("https://www.indeed.com/"));
    }

    #[test]
    fn oauth_popup_requires_exact_https_origin() {
        assert!(!is_oauth_popup_url(
            "https://evil.test/accounts.google.com/gsi/select?ux_mode=popup"
        ));
        assert!(!is_oauth_popup_url(
            "https://accounts.google.com.evil.test/o/oauth2/v2/auth"
        ));
        assert!(!is_oauth_popup_url(
            "https://evil.test/?redirect=https://login.microsoftonline.com/oauth2/authorize"
        ));
        assert!(!is_oauth_popup_url("http://accounts.google.com/gsi"));
        assert!(!is_oauth_popup_url(
            "https://not-google.test/oauth/authorize?ux_mode=popup"
        ));
        assert_eq!(
            oauth_popup_host("https://accounts.google.com/gsi"),
            Some("accounts.google.com".into())
        );
        assert_eq!(oauth_popup_host("https://evil.test/oauth/authorize"), None);
    }

    #[test]
    fn path_under_session_folder_is_component_prefix() {
        let tmp = std::env::temp_dir().join(format!(
            "turbo-session-folder-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session = tmp.join("session");
        let uploads = session.join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let file = uploads.join("resume.pdf");
        std::fs::write(&file, b"%PDF").unwrap();
        std::fs::create_dir_all(tmp.join("session-evil")).unwrap();
        let escape = tmp.join("session-evil").join("x.pdf");
        std::fs::write(&escape, b"no").unwrap();
        assert!(path_is_under_session_folder(&file, &session));
        assert!(path_is_under_session_folder(&uploads, &session));
        assert!(!path_is_under_session_folder(&escape, &session));
        assert!(!path_is_under_session_folder(
            Path::new("/no/such/file"),
            &session
        ));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
