//! JSON-RPC 2.0 wire types, browser methods, and navigation/fill/eval policy.
//!
//! The wire format is a standard envelope (`jsonrpc`, `id`, `method`, `params`),
//! not a serde internally-tagged enum. [`BrowserRequest`] is a convenience
//! decode of `method` + `params` after the envelope is parsed.

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
}

impl BrowserMethod {
    /// Wire method name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => METHOD_NAVIGATE,
            Self::Tabs => METHOD_TABS,
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
        }
    }
}

impl FromStr for BrowserMethod {
    type Err = ProtocolError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            METHOD_NAVIGATE => Ok(Self::Navigate),
            METHOD_TABS => Ok(Self::Tabs),
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
    },
    /// `browser.click`
    #[serde(rename = "browser.click")]
    Click { uid: String },
    /// `browser.fill`
    #[serde(rename = "browser.fill")]
    Fill { uid: String, value: String },
    /// `browser.eval`
    #[serde(rename = "browser.eval")]
    Eval { function: String },
    /// `browser.screenshot`
    #[serde(rename = "browser.screenshot")]
    Screenshot {},
    /// `browser.raise`
    #[serde(rename = "browser.raise")]
    Raise {},
    /// `browser.shutdown`
    #[serde(rename = "browser.shutdown")]
    Shutdown {},
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

/// Result of `browser.snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotResult {
    /// Page URL.
    pub url: String,
    /// Document title.
    pub title: String,
    /// Compact AX nodes.
    pub nodes: Vec<AxNode>,
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
    if field_looks_secret(field_name) {
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

fn check_http_family(rest: &str, https: bool) -> Result<(), UrlPolicyError> {
    let Some(after_slashes) = rest.strip_prefix("//") else {
        return Err(UrlPolicyError::Invalid);
    };
    let authority_end = after_slashes
        .find(['/', '?', '#'])
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
        let guard = test_allow_value().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(over) = guard.as_ref() {
            return Some(over.clone());
        }
        // Lib tests ignore ambient env so they cannot leak across `--test-threads`.
        return None;
    }
    #[cfg(not(test))]
    match std::env::var(GROK_BROWSER_ALLOW_ENV) {
        Ok(v) => Some(v),
        Err(_) => None,
    }
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
        .split(['/', '?', '#'])
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

#[cfg(test)]
fn test_allow_value() -> &'static std::sync::Mutex<Option<String>> {
    static VALUE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    &VALUE
}

#[cfg(test)]
fn test_allow_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

/// Install `GROK_BROWSER_ALLOW` for the duration of `f` (tests only).
#[cfg(test)]
pub(crate) fn with_browser_allow<R>(allow: &str, f: impl FnOnce() -> R) -> R {
    let _lock = test_allow_lock().lock().unwrap_or_else(|e| e.into_inner());
    {
        *test_allow_value().lock().unwrap_or_else(|e| e.into_inner()) = Some(allow.to_owned());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    {
        *test_allow_value().lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
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
    if host.is_empty() || host.contains(':') {
        return Err(UrlPolicyError::Invalid);
    }
    Ok(host.to_string())
}

fn is_allowed_http_host(host: &str) -> bool {
    if is_localhost_name(host) {
        return true;
    }
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return ip.is_loopback() || is_rfc1918(ip);
    }
    if let Ok(ip) = host.parse::<Ipv6Addr>() {
        return ip.is_loopback();
    }
    false
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
    path.starts_with(root)
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
            nodes: vec![AxNode {
                uid: "1".into(),
                role: "link".into(),
                name: "More information".into(),
                value: None,
                focused: false,
            }],
        };
        let v = serde_json::to_value(&snap).unwrap();
        let back: SnapshotResult = serde_json::from_value(v).unwrap();
        assert_eq!(back.nodes[0].uid, "1");

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
}
