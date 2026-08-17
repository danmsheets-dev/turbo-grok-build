//! Accessibility snapshot compaction and uid helpers.
//!
//! Live snapshot prefers the injected [`TURBO_AX_JS`] collector (tagged DOM).
//! [`compact_ax_tree`] is a pure function over a CDP
//! `Accessibility.getFullAXTree` dump (used in tests and as a fallback).
//!
//! Uids are 1-based decimal strings (`"1"`, `"2"`, …) in document / tree
//! order over the same node set the tagger stamps as `data-turbo-uid`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::protocol::{AxNode, check_eval_result};

/// Default `browser.snapshot` node cap.
pub const SNAPSHOT_NODE_CAP: usize = 200;
/// `browser.snapshot` cap when `verbose=true`.
pub const SNAPSHOT_NODE_CAP_VERBOSE: usize = 800;

/// Source collector (`include_str!`). Prefer [`turbo_ax_js_injected`] at
/// inject sites so a CRLF checkout cannot ship `\r` into WebView2.
pub(crate) const TURBO_AX_JS: &str = include_str!("../../assets/turbo_ax.js");

/// JS actually shipped into WebView2. Strips `\r` as defense in depth.
pub(crate) fn turbo_ax_js_injected() -> Cow<'static, str> {
    strip_injected_cr(TURBO_AX_JS)
}

fn strip_injected_cr(js: &str) -> Cow<'_, str> {
    if js.as_bytes().contains(&b'\r') {
        Cow::Owned(js.replace('\r', ""))
    } else {
        Cow::Borrowed(js)
    }
}

/// Cap for `verbose` (800) vs compact (200).
pub fn snapshot_cap(verbose: bool) -> usize {
    if verbose {
        SNAPSHOT_NODE_CAP_VERBOSE
    } else {
        SNAPSHOT_NODE_CAP
    }
}

/// JSON-RPC / host error text for a missing or invalid snapshot uid.
pub fn unknown_uid_message(uid: &str) -> String {
    format!("unknown_uid: {uid}")
}

/// Accept the injected uid scheme: non-empty decimal without leading zeros.
pub fn resolve_uid(uid: &str) -> Result<&str, String> {
    if is_turbo_uid(uid) {
        Ok(uid)
    } else {
        Err(unknown_uid_message(uid))
    }
}

fn is_turbo_uid(uid: &str) -> bool {
    !uid.is_empty() && !uid.starts_with('0') && uid.chars().all(|c| c.is_ascii_digit())
}

/// Compact a CDP `Accessibility.getFullAXTree` JSON dump into [`AxNode`]s.
///
/// Skips `ignored` nodes and `RootWebArea` containers in the output, but
/// still walks `childIds` of ignored wrappers (real CDP dumps nest heading
/// / link / textbox nodes under them). Assigns sequential `"1"`… uids in
/// tree order (childIds) over heading + interactive roles so they can match
/// `data-turbo-uid` document order on simple pages.
pub fn compact_ax_tree(json: &str, verbose: bool) -> Result<Vec<AxNode>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| format!("AX tree JSON: {e}"))?;
    let nodes_val = match &value {
        Value::Array(arr) => arr.clone(),
        Value::Object(_) => value
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| "AX tree missing nodes".to_owned())?,
        _ => return Err("AX tree JSON is not an object or array".into()),
    };
    let nodes: Vec<CdpAxNode> = serde_json::from_value(Value::Array(nodes_val))
        .map_err(|e| format!("AX node decode: {e}"))?;

    let by_id: HashMap<&str, &CdpAxNode> = nodes
        .iter()
        .filter(|n| !n.node_id.is_empty())
        .map(|n| (n.node_id.as_str(), n))
        .collect();
    let listed_as_child: HashSet<&str> = nodes
        .iter()
        .flat_map(|n| n.child_ids.iter().map(String::as_str))
        .collect();
    let roots: Vec<&CdpAxNode> = nodes
        .iter()
        .filter(|n| !n.node_id.is_empty() && !listed_as_child.contains(n.node_id.as_str()))
        .collect();

    let cap = snapshot_cap(verbose);
    let mut out = Vec::new();
    let has_edges = nodes.iter().any(|n| !n.child_ids.is_empty());

    if has_edges && !roots.is_empty() {
        let mut stack: Vec<&CdpAxNode> = roots.into_iter().rev().collect();
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(node) = stack.pop() {
            if !seen.insert(node.node_id.as_str()) {
                continue;
            }
            if !node.ignored {
                maybe_push_compact(node, verbose, cap, &mut out);
                if out.len() >= cap {
                    break;
                }
            }
            for child_id in node.child_ids.iter().rev() {
                if let Some(child) = by_id.get(child_id.as_str()) {
                    stack.push(*child);
                }
            }
        }
    } else {
        for node in &nodes {
            if node.ignored {
                continue;
            }
            maybe_push_compact(node, verbose, cap, &mut out);
            if out.len() >= cap {
                break;
            }
        }
    }
    Ok(out)
}

fn maybe_push_compact(node: &CdpAxNode, verbose: bool, cap: usize, out: &mut Vec<AxNode>) {
    if out.len() >= cap {
        return;
    }
    let role = ax_value_string(&node.role);
    let name = ax_value_string(&node.name);
    if !include_role(&role, &name, verbose) {
        return;
    }
    let value = {
        let v = ax_value_string(&node.value);
        if v.is_empty() { None } else { Some(v) }
    };
    out.push(AxNode {
        uid: (out.len() + 1).to_string(),
        role,
        name,
        value,
        focused: ax_focused(&node.properties),
    });
}

fn include_role(role: &str, name: &str, verbose: bool) -> bool {
    let r = role.trim();
    if r.is_empty() || eq_ci(r, "RootWebArea") || eq_ci(r, "none") || eq_ci(r, "LineBreak") {
        return false;
    }
    const CORE: &[&str] = &[
        "heading",
        "link",
        "button",
        "textbox",
        "searchbox",
        "combobox",
        "checkbox",
        "radio",
        "switch",
        "tab",
        "menuitem",
        "menuitemcheckbox",
        "menuitemradio",
        "slider",
        "spinbutton",
        "listbox",
        "option",
        "image",
        "img",
    ];
    if CORE.iter().any(|k| eq_ci(r, k)) {
        return true;
    }
    verbose && !name.trim().is_empty() && !eq_ci(r, "Iframe")
}

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn ax_value_string(v: &Option<CdpAxValue>) -> String {
    let Some(v) = v else {
        return String::new();
    };
    match &v.value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn ax_focused(props: &[CdpAxProperty]) -> bool {
    props.iter().any(|p| {
        p.name.eq_ignore_ascii_case("focused")
            && p.value
                .as_ref()
                .and_then(|v| v.value.as_ref())
                .is_some_and(|v| v.as_bool() == Some(true))
    })
}

#[derive(Debug, Deserialize)]
struct CdpAxNode {
    #[serde(rename = "nodeId", default)]
    node_id: String,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    role: Option<CdpAxValue>,
    #[serde(default)]
    name: Option<CdpAxValue>,
    #[serde(default)]
    value: Option<CdpAxValue>,
    #[serde(default)]
    properties: Vec<CdpAxProperty>,
    #[serde(rename = "childIds", default)]
    child_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CdpAxValue {
    #[serde(default)]
    value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CdpAxProperty {
    name: String,
    #[serde(default)]
    value: Option<CdpAxValue>,
}

/// Parse the injected collector's JSON array into [`AxNode`]s (capped).
pub(crate) fn parse_ax_nodes_json(json: &str, cap: usize) -> Result<Vec<AxNode>, String> {
    let nodes: Vec<CollectedNode> =
        serde_json::from_str(json.trim()).map_err(|e| format!("snapshot JSON: {e}"))?;
    Ok(nodes
        .into_iter()
        .filter(|n| resolve_uid(&n.uid).is_ok())
        .take(cap)
        .map(|n| AxNode {
            uid: n.uid,
            role: n.role,
            name: n.name,
            value: n.value.filter(|v| !v.is_empty()),
            focused: n.focused,
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct CollectedNode {
    uid: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    focused: bool,
}

/// Interpret a click/fill/lookup script result (`{ok, error, …}`).
pub(crate) fn interpret_uid_action(uid: &str, json: &str) -> Result<Value, String> {
    let trimmed = json.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Err("ax script returned null".into());
    }
    let v: Value = serde_json::from_str(trimmed).map_err(|e| format!("ax script JSON: {e}"))?;
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(v);
    }
    let err = v
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("action failed");
    if err.contains("unknown_uid") {
        return Err(unknown_uid_message(uid));
    }
    Err(err.to_owned())
}

/// Decode CDP `Runtime.evaluate` JSON (JSON.stringify wrap + `returnByValue`).
pub(crate) fn parse_eval_cdp(json: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("eval CDP JSON: {e}"))?;
    if let Some(exc) = v.get("exceptionDetails") {
        let msg = exc
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(Value::as_str)
            .or_else(|| exc.get("text").and_then(Value::as_str))
            .unwrap_or("evaluation failed");
        return Err(format!("eval exception: {msg}"));
    }
    let result = v
        .get("result")
        .ok_or_else(|| "eval missing result".to_owned())?;
    let typ = result.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(typ, "undefined" | "function" | "symbol") {
        return Err("eval result is not JSON".into());
    }
    if let Some(s) = result.get("value").and_then(Value::as_str) {
        check_eval_result(s).map_err(|e| e.to_string())?;
        let parsed: Value =
            serde_json::from_str(s).map_err(|_| "eval result is not JSON".to_owned())?;
        let serialized =
            serde_json::to_string(&parsed).map_err(|e| format!("eval re-encode: {e}"))?;
        check_eval_result(&serialized).map_err(|e| e.to_string())?;
        return Ok(parsed);
    }
    if result.get("value").is_some_and(Value::is_null) {
        return Err("eval result is not JSON".into());
    }
    if let Some(val) = result.get("value") {
        let serialized = serde_json::to_string(val).map_err(|e| e.to_string())?;
        check_eval_result(&serialized).map_err(|e| e.to_string())?;
        return Ok(val.clone());
    }
    Err("eval result is not JSON".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::EVAL_RESULT_MAX_BYTES;

    const AX_EXAMPLE: &str = include_str!("../../tests/fixtures/ax_example.json");

    #[test]
    fn compact_ax_example_fixture() {
        let nodes = compact_ax_tree(AX_EXAMPLE, false).expect("compact fixture");
        assert_eq!(nodes.len(), 3);
        // Heading is a child of the ignored generic wrapper; walking
        // ignored `childIds` is what assigns it uid "1".
        assert_eq!(nodes[0].uid, "1");
        assert_eq!(nodes[0].role, "heading");
        assert_eq!(nodes[0].name, "Example Domain");
        assert!(!nodes[0].focused);
        assert!(
            nodes.iter().any(|n| n.uid == "1" && n.role == "heading"),
            "descendant of ignored wrapper must still get a uid"
        );
        assert_eq!(nodes[1].uid, "2");
        assert_eq!(nodes[1].role, "link");
        assert_eq!(nodes[1].name, "More information...");
        assert_eq!(nodes[2].uid, "3");
        assert_eq!(nodes[2].role, "textbox");
        assert_eq!(nodes[2].name, "Search");
        assert_eq!(nodes[2].value.as_deref(), Some("query"));
        assert!(nodes[2].focused);
        assert!(nodes.iter().all(|n| n.role != "RootWebArea"));
        assert!(nodes.iter().all(|n| n.role != "generic"));
        assert!(nodes.iter().all(|n| n.role != "StaticText"));
    }

    #[test]
    fn compact_ax_verbose_includes_named_static_text() {
        let nodes = compact_ax_tree(AX_EXAMPLE, true).expect("verbose compact");
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].role, "heading");
        assert_eq!(nodes[1].role, "StaticText");
        assert!(nodes[1].name.contains("illustrative"));
        assert_eq!(nodes[2].role, "link");
        assert_eq!(nodes[3].role, "textbox");
        assert_eq!(nodes[1].uid, "2");
        assert_eq!(nodes[2].uid, "3");
    }

    #[test]
    fn compact_ax_respects_caps() {
        let json = synthetic_links(250);
        let compact = compact_ax_tree(&json, false).unwrap();
        assert_eq!(compact.len(), SNAPSHOT_NODE_CAP);
        assert_eq!(compact[0].uid, "1");
        assert_eq!(compact[199].uid, "200");
        let verbose = compact_ax_tree(&json, true).unwrap();
        assert_eq!(verbose.len(), 250);
        assert!(verbose.len() <= SNAPSHOT_NODE_CAP_VERBOSE);
    }

    #[test]
    fn resolve_uid_accepts_decimal_scheme() {
        assert_eq!(resolve_uid("1").unwrap(), "1");
        assert_eq!(resolve_uid("42").unwrap(), "42");
        for bad in ["", "0", "01", "1a", "-1", "uid-1", " 1"] {
            let err = resolve_uid(bad).unwrap_err();
            assert!(err.contains("unknown_uid"), "{bad}: {err}");
        }
    }

    #[test]
    fn interpret_unknown_uid_action() {
        let err = interpret_uid_action("99", r#"{"ok":false,"error":"unknown_uid"}"#).unwrap_err();
        assert!(err.contains("unknown_uid"));
        assert!(err.contains("99"));
        assert!(interpret_uid_action("1", r#"{"ok":true}"#).is_ok());
    }

    #[test]
    fn parse_eval_stringify_and_policy() {
        assert_eq!(
            parse_eval_cdp(r#"{"result":{"type":"string","value":"1"}}"#).unwrap(),
            serde_json::json!(1)
        );
        assert_eq!(
            parse_eval_cdp(r#"{"result":{"type":"string","value":"{\"title\":\"Example\"}"}}"#)
                .unwrap()["title"],
            "Example"
        );
        let huge = "x".repeat(EVAL_RESULT_MAX_BYTES + 1);
        let encoded = serde_json::to_string(&huge).unwrap();
        let json = serde_json::json!({
            "result": { "type": "string", "value": encoded }
        })
        .to_string();
        let err = parse_eval_cdp(&json).unwrap_err();
        assert!(err.contains("exceeds") || err.contains("20000"), "{err}");

        let exc = parse_eval_cdp(
            r#"{"result":{"type":"object"},"exceptionDetails":{"text":"Uncaught"}}"#,
        )
        .unwrap_err();
        assert!(exc.contains("eval exception"), "{exc}");

        assert!(
            parse_eval_cdp(r#"{"result":{"type":"undefined"}}"#)
                .unwrap_err()
                .contains("not JSON")
        );
    }

    #[test]
    fn turbo_ax_js_is_tiny_and_tags_uid() {
        let injected = turbo_ax_js_injected();
        assert!(
            injected.len() < 4_096,
            "keep turbo_ax.js tiny ({})",
            injected.len()
        );
        assert!(injected.contains("data-turbo-uid"));
        assert!(injected.contains("a[href]"));
        assert!(injected.contains("unknown_uid"));
        assert!(!injected.contains('\r'), "injected JS must be LF");
        assert_eq!(strip_injected_cr("a\r\nb").as_ref(), "a\nb");
        assert!(matches!(strip_injected_cr("a\nb"), Cow::Borrowed(_)));
    }

    fn synthetic_links(n: usize) -> String {
        let child_ids: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
        let mut nodes = vec![serde_json::json!({
            "nodeId": "0",
            "ignored": false,
            "role": { "type": "role", "value": "RootWebArea" },
            "name": { "type": "computedString", "value": "" },
            "childIds": child_ids,
        })];
        for i in 1..=n {
            nodes.push(serde_json::json!({
                "nodeId": i.to_string(),
                "ignored": false,
                "role": { "type": "role", "value": "link" },
                "name": { "type": "computedString", "value": format!("L{i}") },
                "parentId": "0",
                "childIds": []
            }));
        }
        serde_json::json!({ "nodes": nodes }).to_string()
    }
}
