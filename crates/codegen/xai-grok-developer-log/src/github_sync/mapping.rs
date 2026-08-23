//! Issue body, labels, and status mapping (pure; no `gh` / network).

use std::borrow::Cow;

use serde_json::Value;

use crate::feature_request::schema::{FeatureRequest, RequestStatus};
use crate::redact::sanitize_incident;
use crate::schema::{Incident, IncidentStatus};

/// Marker baked into every uploaded issue body.
pub const MARKER_PREFIX: &str = "<!-- turbo-log v1 ";

/// GitHub issue body size budget (GitHub hard cap is 65536).
const BODY_MAX: usize = 58_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Incident,
    Feature,
}

impl LogKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incident => "incident",
            Self::Feature => "feature",
        }
    }

    pub fn type_label(self) -> &'static str {
        match self {
            Self::Incident => "type:incident",
            Self::Feature => "type:feature",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "incident" | "incidents" => Some(Self::Incident),
            "feature" | "features" => Some(Self::Feature),
            _ => None,
        }
    }
}

/// Hidden HTML comment used as the durable upsert key.
pub fn marker_comment(fingerprint: &str, kind: LogKind) -> String {
    format!(
        "<!-- turbo-log v1 fingerprint={fingerprint} kind={} -->",
        kind.as_str()
    )
}

/// Parse `fingerprint` + `kind` from a GitHub issue body.
pub fn parse_marker(body: &str) -> Option<(String, LogKind)> {
    let idx = body.find(MARKER_PREFIX)?;
    let rest = &body[idx + MARKER_PREFIX.len()..];
    let end = rest.find("-->")?;
    let attrs = rest[..end].trim();
    let mut fingerprint = None;
    let mut kind = None;
    for part in attrs.split_whitespace() {
        if let Some(v) = part.strip_prefix("fingerprint=") {
            if !v.is_empty() {
                fingerprint = Some(v.to_string());
            }
        } else if let Some(v) = part.strip_prefix("kind=") {
            kind = LogKind::parse(v);
        }
    }
    Some((fingerprint?, kind.unwrap_or(LogKind::Incident)))
}

/// Fingerprint from the dedicated `fp:` label, if present.
pub fn fingerprint_from_labels(labels: &[String]) -> Option<String> {
    labels.iter().find_map(|l| {
        l.strip_prefix("fp:")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    })
}

/// Labels Turbo manages (safe to add/remove on sync). Human labels are left alone.
pub fn is_managed_label(name: &str) -> bool {
    matches!(
        name,
        "type:incident"
            | "type:feature"
            | "p0"
            | "p1"
            | "p2"
            | "p3"
            | "must_have"
            | "should_have"
            | "nice_to_have"
            | "exploratory"
            | "acknowledged"
            | "planned"
            | "resolved"
            | "shipped"
            | "declined"
    ) || name.starts_with("class:")
        || name.starts_with("component:")
        || name.starts_with("fp:")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteState {
    Open,
    Closed,
}

/// Desired GitHub state + status label for a local incident.
pub fn incident_remote_status(status: IncidentStatus) -> (RemoteState, Option<&'static str>) {
    match status {
        IncidentStatus::Open => (RemoteState::Open, None),
        IncidentStatus::Acknowledged => (RemoteState::Open, Some("acknowledged")),
        IncidentStatus::Resolved => (RemoteState::Closed, Some("resolved")),
        IncidentStatus::Wontdo => (RemoteState::Closed, Some("declined")),
    }
}

/// Desired GitHub state + status label for a local feature request.
pub fn feature_remote_status(status: RequestStatus) -> (RemoteState, Option<&'static str>) {
    match status {
        RequestStatus::Open => (RemoteState::Open, None),
        RequestStatus::Acknowledged => (RemoteState::Open, Some("acknowledged")),
        RequestStatus::Planned => (RemoteState::Open, Some("planned")),
        RequestStatus::Shipped => (RemoteState::Closed, Some("shipped")),
        RequestStatus::Declined => (RemoteState::Closed, Some("declined")),
    }
}

/// Map a closed GitHub issue's labels back onto a local incident status.
pub fn incident_status_from_remote(
    state: RemoteState,
    labels: &[String],
) -> Option<IncidentStatus> {
    if state != RemoteState::Closed {
        return None;
    }
    if labels.iter().any(|l| l == "declined") {
        Some(IncidentStatus::Wontdo)
    } else if labels.iter().any(|l| l == "resolved" || l == "shipped") {
        Some(IncidentStatus::Resolved)
    } else {
        None
    }
}

/// Map a closed GitHub issue's labels back onto a local feature-request status.
pub fn feature_status_from_remote(state: RemoteState, labels: &[String]) -> Option<RequestStatus> {
    if state != RemoteState::Closed {
        return None;
    }
    if labels.iter().any(|l| l == "declined") {
        Some(RequestStatus::Declined)
    } else if labels.iter().any(|l| l == "shipped" || l == "resolved") {
        Some(RequestStatus::Shipped)
    } else {
        None
    }
}

fn sanitize_label_value(raw: &str, max: usize) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        return None;
    }
    let out: String = cleaned.chars().take(max).collect();
    if out.starts_with('-') {
        return None;
    }
    Some(out)
}

fn push_label(out: &mut Vec<String>, name: String) {
    if name.is_empty() || name.len() > 50 || name.starts_with('-') {
        return;
    }
    if !out.iter().any(|e| e == &name) {
        out.push(name);
    }
}

/// Labels for an incident (type, class, severity, components, fingerprint, status).
pub fn incident_labels(inc: &Incident) -> Vec<String> {
    let mut labels = Vec::new();
    push_label(&mut labels, LogKind::Incident.type_label().into());
    push_label(&mut labels, format!("class:{}", inc.error_class.as_str()));
    push_label(&mut labels, inc.severity.as_str().into());
    if let Some(s) = incident_remote_status(inc.status).1 {
        push_label(&mut labels, s.into());
    }
    for c in &inc.component {
        if let Some(v) = sanitize_label_value(c, 40) {
            push_label(&mut labels, format!("component:{v}"));
        }
    }
    if let Some(fp) = fingerprint_label(&inc.fingerprint) {
        push_label(&mut labels, fp);
    }
    labels
}

/// Labels for a feature request.
pub fn feature_labels(fr: &FeatureRequest) -> Vec<String> {
    let mut labels = Vec::new();
    push_label(&mut labels, LogKind::Feature.type_label().into());
    push_label(&mut labels, format!("class:{}", fr.request_class.as_str()));
    push_label(&mut labels, fr.priority.as_str().into());
    if let Some(s) = feature_remote_status(fr.status).1 {
        push_label(&mut labels, s.into());
    }
    for c in &fr.component {
        if let Some(v) = sanitize_label_value(c, 40) {
            push_label(&mut labels, format!("component:{v}"));
        }
    }
    if let Some(fp) = fingerprint_label(&fr.fingerprint) {
        push_label(&mut labels, fp);
    }
    labels
}

fn fingerprint_label(fp: &str) -> Option<String> {
    let v = sanitize_label_value(fp, 47)?;
    let name = format!("fp:{v}");
    (name.len() <= 50).then_some(name)
}

/// True when a JSON document still contains a known secret shape that
/// [`xai_grok_secrets::redact_secrets`] would replace with `[REDACTED_SECRET]`.
pub fn json_has_unredacted_secrets(value: &Value) -> bool {
    let mut hit = false;
    xai_grok_secrets::walk_json_strings(&mut value.clone(), &mut |s| {
        if secret_count(s) < secret_count(xai_grok_secrets::redact_secrets(s).as_ref()) {
            hit = true;
        }
    });
    hit
}

fn secret_count(s: &str) -> usize {
    s.matches("[REDACTED_SECRET]").count()
}

/// Redact JSON string values; error if a token shape remains afterwards.
pub fn prepare_upload_json(mut value: Value) -> Result<Value, UnresolvedRedact> {
    xai_grok_secrets::redact_json_string_values(&mut value);
    if json_has_unredacted_secrets(&value) {
        return Err(UnresolvedRedact);
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnresolvedRedact;

/// Build the GitHub issue body for an already-local incident.
///
/// Sanitizes, then refuses to emit a body that still has unresolved secret
/// shapes. The fenced JSON is the redacted document.
pub fn incident_issue_body(inc: &Incident) -> Result<String, UnresolvedRedact> {
    let sanitized = sanitize_incident(inc.clone());
    let value = serde_json::to_value(&sanitized).map_err(|_| UnresolvedRedact)?;
    let value = prepare_upload_json(value)?;
    render_body(
        LogKind::Incident,
        &sanitized.fingerprint,
        &sanitized.title,
        &sanitized.summary,
        &sanitized.incident_id,
        sanitized.error_class.as_str(),
        sanitized.status.as_str(),
        sanitized.occurrence_count,
        Some(sanitized.severity.as_str()),
        None,
        &value,
    )
}

/// Build the GitHub issue body for a feature request.
pub fn feature_issue_body(fr: &FeatureRequest) -> Result<String, UnresolvedRedact> {
    let sanitized = crate::feature_request::store::sanitize_feature_request(fr.clone());
    let value = serde_json::to_value(&sanitized).map_err(|_| UnresolvedRedact)?;
    let value = prepare_upload_json(value)?;
    render_body(
        LogKind::Feature,
        &sanitized.fingerprint,
        &sanitized.title,
        &sanitized.summary,
        &sanitized.request_id,
        sanitized.request_class.as_str(),
        sanitized.status.as_str(),
        sanitized.occurrence_count,
        None,
        Some(sanitized.priority.as_str()),
        &value,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_body(
    kind: LogKind,
    fingerprint: &str,
    title: &str,
    summary: &str,
    id: &str,
    class: &str,
    status: &str,
    occurrences: u32,
    severity: Option<&str>,
    priority: Option<&str>,
    json: &Value,
) -> Result<String, UnresolvedRedact> {
    let mut json_value = json.clone();
    let pretty = compact_json_for_body(&mut json_value)?;
    let mut md = String::new();
    md.push_str(&marker_comment(fingerprint, kind));
    md.push_str("\n\n");
    md.push_str("# ");
    md.push_str(title);
    md.push_str("\n\n");
    md.push_str(summary);
    md.push_str("\n\n");
    md.push_str("| Field | Value |\n| --- | --- |\n");
    md.push_str(&format!("| id | `{id}` |\n"));
    md.push_str(&format!("| fingerprint | `{fingerprint}` |\n"));
    md.push_str(&format!("| class | `{class}` |\n"));
    md.push_str(&format!("| status | `{status}` |\n"));
    md.push_str(&format!("| occurrences | {occurrences} |\n"));
    if let Some(sev) = severity {
        md.push_str(&format!("| severity | `{sev}` |\n"));
    }
    if let Some(pri) = priority {
        md.push_str(&format!("| priority | `{pri}` |\n"));
    }
    md.push_str("\n```json\n");
    md.push_str(&pretty.replace("```", "`\\`\\`"));
    md.push_str("\n```\n");
    if md.len() > BODY_MAX {
        return Err(UnresolvedRedact);
    }
    // Whole-body pass: leftover token shapes after sanitizers → refuse.
    let redacted = xai_grok_secrets::redact_secrets(&md);
    if secret_count(redacted.as_ref()) > secret_count(&md) {
        return Err(UnresolvedRedact);
    }
    Ok(md)
}

fn compact_json_for_body(value: &mut Value) -> Result<String, UnresolvedRedact> {
    let pretty = serde_json::to_string_pretty(value).map_err(|_| UnresolvedRedact)?;
    if pretty.len() <= 45_000 {
        return Ok(pretty);
    }
    if let Some(obj) = value.as_object_mut() {
        obj.remove("evidence");
        obj.remove("environment");
        obj.remove("repro");
    }
    let pretty = serde_json::to_string_pretty(value).map_err(|_| UnresolvedRedact)?;
    if pretty.len() > 45_000 {
        return Err(UnresolvedRedact);
    }
    Ok(pretty)
}

/// Comment posted when `occurrence_count` increases on an existing issue.
pub fn seen_comment(count: u32) -> String {
    format!("seen {count}x")
}

/// Comment posted when a proving git sha is recorded.
pub fn proving_sha_comment(kind: LogKind, sha: &str, note: Option<&str>) -> String {
    let verb = match kind {
        LogKind::Incident => "Resolved",
        LogKind::Feature => "Shipped",
    };
    let mut s = format!("{verb} at `{sha}`.");
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        s.push(' ');
        s.push_str(n);
    }
    match xai_grok_secrets::redact_secrets(&s) {
        Cow::Borrowed(_) => s,
        Cow::Owned(r) => r,
    }
}

/// `add` / `remove` so the remote managed labels match `desired`.
pub fn label_diff(current: &[String], desired: &[String]) -> (Vec<String>, Vec<String>) {
    let add: Vec<String> = desired
        .iter()
        .filter(|d| !current.iter().any(|c| c == *d))
        .cloned()
        .collect();
    let remove: Vec<String> = current
        .iter()
        .filter(|c| is_managed_label(c) && !desired.iter().any(|d| d == *c))
        .cloned()
        .collect();
    (add, remove)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ErrorClass, IncidentKind, Severity};
    use chrono::Utc;

    fn fixture(parts: &[&str]) -> String {
        parts.concat()
    }

    fn sample_incident() -> Incident {
        Incident {
            schema_version: 1,
            incident_id: "inc_test".into(),
            fingerprint: "worktree_tombstone-aaaaaaaaaaaaaaaa".into(),
            kind: IncidentKind::Bug,
            title: "Worktree path unusable after complete".into(),
            summary: "meta still points at deleted worktree".into(),
            severity: Severity::P0,
            status: IncidentStatus::Open,
            component: vec!["worktree".into(), "subagent".into()],
            error_class: ErrorClass::WorktreeTombstone,
            occurrence_count: 2,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            environment: Default::default(),
            repro: Default::default(),
            evidence: Default::default(),
            suggested_fix: None,
            source: Default::default(),
            tags: vec![],
            resolution_sha: None,
            resolution_note: None,
        }
    }

    #[test]
    fn marker_roundtrip() {
        let fp = "worktree_tombstone-aaaaaaaaaaaaaaaa";
        let body = marker_comment(fp, LogKind::Incident);
        let (got, kind) = parse_marker(&body).expect("marker");
        assert_eq!(got, fp);
        assert_eq!(kind, LogKind::Incident);
    }

    #[test]
    fn incident_labels_include_type_class_sev_fp() {
        let labels = incident_labels(&sample_incident());
        assert!(labels.contains(&"type:incident".into()));
        assert!(labels.contains(&"class:worktree_tombstone".into()));
        assert!(labels.contains(&"p0".into()));
        assert!(labels.contains(&"component:worktree".into()));
        assert!(labels.iter().any(|l| l.starts_with("fp:")));
        assert!(!labels.iter().any(|l| l == "resolved"));
    }

    #[test]
    fn resolved_maps_to_closed_resolved() {
        assert_eq!(
            incident_remote_status(IncidentStatus::Resolved),
            (RemoteState::Closed, Some("resolved"))
        );
        assert_eq!(
            incident_remote_status(IncidentStatus::Acknowledged),
            (RemoteState::Open, Some("acknowledged"))
        );
        assert_eq!(
            feature_remote_status(RequestStatus::Planned),
            (RemoteState::Open, Some("planned"))
        );
        assert_eq!(
            feature_remote_status(RequestStatus::Shipped),
            (RemoteState::Closed, Some("shipped"))
        );
        assert_eq!(
            incident_status_from_remote(RemoteState::Closed, &["resolved".into()]),
            Some(IncidentStatus::Resolved)
        );
        assert_eq!(
            incident_status_from_remote(RemoteState::Open, &["resolved".into()]),
            None
        );
    }

    #[test]
    fn body_contains_marker_and_fenced_json() {
        let body = incident_issue_body(&sample_incident()).expect("body");
        assert!(body.contains("<!-- turbo-log v1 fingerprint="));
        assert!(body.contains("kind=incident"));
        assert!(body.contains("```json"));
        assert!(body.contains("worktree_tombstone-aaaaaaaaaaaaaaaa"));
        assert!(body.contains("\"occurrence_count\": 2"));
    }

    #[test]
    fn redact_strips_secrets_from_gh_body() {
        let ghp = fixture(&["ghp_f", "akefakefakefakefakefakefake"]);
        let mut inc = sample_incident();
        inc.summary = format!("leaked {ghp} in summary");
        let body = incident_issue_body(&inc).expect("sanitized body");
        assert!(
            !body.contains("ghp_f"),
            "secret survived into GH body: {body}"
        );
        assert!(body.contains("[REDACTED_SECRET]"));
    }

    #[test]
    fn unredacted_json_is_detected() {
        let ghp = fixture(&["ghp_f", "akefakefakefakefakefakefake"]);
        let raw = serde_json::json!({ "summary": ghp });
        assert!(json_has_unredacted_secrets(&raw));
        let prepared = prepare_upload_json(raw).expect("redact then accept");
        assert!(!json_has_unredacted_secrets(&prepared));
    }

    #[test]
    fn label_diff_preserves_human_labels() {
        let current = vec!["type:incident".into(), "help-wanted".into(), "p3".into()];
        let desired = vec!["type:incident".into(), "p0".into()];
        let (add, remove) = label_diff(&current, &desired);
        assert!(add.contains(&"p0".into()));
        assert!(remove.contains(&"p3".into()));
        assert!(!remove.iter().any(|l| l == "help-wanted"));
    }

    #[test]
    fn seen_and_sha_comments() {
        assert_eq!(seen_comment(4), "seen 4x");
        let c = proving_sha_comment(LogKind::Incident, "abc1234", Some("landed keep-N"));
        assert!(c.contains("`abc1234`"));
        assert!(c.contains("landed keep-N"));
    }
}
