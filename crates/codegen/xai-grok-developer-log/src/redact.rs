//! Redaction for free-form incident fields.

use std::borrow::Cow;

use crate::schema::{Evidence, Incident, ReportRequest, Repro};

/// Redact secrets and user home paths in a free-form string.
pub fn redact_text(input: &str) -> String {
    let secrets = xai_grok_secrets::redact_secrets(input);
    match xai_grok_secrets::redact_user_paths(secrets.as_ref()) {
        Cow::Owned(s) => s,
        Cow::Borrowed(s) => s.to_string(),
    }
}

/// Cap free-form field length to keep incidents reviewable.
pub fn truncate_field(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

const TITLE_MAX: usize = 200;
const SUMMARY_MAX: usize = 4_000;
const STEP_MAX: usize = 500;
const STEPS_MAX: usize = 20;
const NOTE_MAX: usize = 2_000;

/// Sanitize a report request before persist.
pub fn sanitize_request(mut req: ReportRequest) -> ReportRequest {
    req.title = truncate_field(&redact_text(&req.title), TITLE_MAX);
    req.summary = truncate_field(&redact_text(&req.summary), SUMMARY_MAX);
    if let Some(fix) = req.suggested_fix.take() {
        req.suggested_fix = Some(truncate_field(&redact_text(&fix), SUMMARY_MAX));
    }
    req.repro = sanitize_repro(req.repro);
    req.evidence = sanitize_evidence(req.evidence);
    req.component = req
        .component
        .into_iter()
        .map(|c| truncate_field(&redact_text(&c), 64))
        .filter(|c| !c.is_empty())
        .take(16)
        .collect();
    req.tags = req
        .tags
        .into_iter()
        .map(|t| truncate_field(&redact_text(&t), 64))
        .filter(|t| !t.is_empty())
        .take(16)
        .collect();
    // Model/provider ids are intentional; still scrub accidental secrets.
    if let Some(m) = req.environment.model.as_mut() {
        *m = truncate_field(&redact_text(m), 128);
    }
    if let Some(p) = req.environment.provider.as_mut() {
        *p = truncate_field(&redact_text(p), 128);
    }
    if let Some(m) = req.source.reporter_model.as_mut() {
        *m = truncate_field(&redact_text(m), 128);
    }
    req
}

fn sanitize_repro(mut repro: Repro) -> Repro {
    repro.steps = repro
        .steps
        .into_iter()
        .map(|s| truncate_field(&redact_text(&s), STEP_MAX))
        .filter(|s| !s.is_empty())
        .take(STEPS_MAX)
        .collect();
    if let Some(e) = repro.expected.as_mut() {
        *e = truncate_field(&redact_text(e), SUMMARY_MAX);
    }
    if let Some(a) = repro.actual.as_mut() {
        *a = truncate_field(&redact_text(a), SUMMARY_MAX);
    }
    repro
}

fn sanitize_evidence(mut evidence: Evidence) -> Evidence {
    if let Some(n) = evidence.notes.as_mut() {
        *n = truncate_field(&redact_text(n), NOTE_MAX);
    }
    evidence.related_events = evidence
        .related_events
        .into_iter()
        .map(|e| truncate_field(&redact_text(&e), 128))
        .take(32)
        .collect();
    evidence.attachments = evidence
        .attachments
        .into_iter()
        .map(|a| truncate_field(&redact_text(&a), 512))
        .take(16)
        .collect();
    // Paths may contain usernames — redact path segments.
    if let Some(p) = evidence.meta_path.as_mut() {
        *p = redact_text(p);
    }
    if let Some(p) = evidence.patch_path.as_mut() {
        *p = redact_text(p);
    }
    if let Some(p) = evidence.session_ref.as_mut() {
        *p = redact_text(p);
    }
    evidence
}

/// Re-sanitize an existing incident document (e.g. before export).
pub fn sanitize_incident(mut inc: Incident) -> Incident {
    inc.title = truncate_field(&redact_text(&inc.title), TITLE_MAX);
    inc.summary = truncate_field(&redact_text(&inc.summary), SUMMARY_MAX);
    if let Some(fix) = inc.suggested_fix.as_mut() {
        *fix = truncate_field(&redact_text(fix), SUMMARY_MAX);
    }
    inc.repro = sanitize_repro(inc.repro);
    inc.evidence = sanitize_evidence(inc.evidence);
    inc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bearer_tokens() {
        let out = redact_text("Authorization: Bearer sk-CANARYabcdefghij1234567890");
        assert!(!out.contains("CANARY"), "secret survived: {out}");
    }

    #[test]
    fn truncates_long_title() {
        let long = "x".repeat(500);
        let out = truncate_field(&long, 10);
        assert!(out.chars().count() <= 10);
        assert!(out.ends_with('…'));
    }
}
