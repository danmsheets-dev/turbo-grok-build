//! Fingerprint computation for feature-request deduplication.

use sha2::{Digest, Sha256};

use super::schema::{FeatureRequestReport, RequestClass};

/// Build a stable fingerprint used as the dedup key.
///
/// Format: `{request_class}-{sha256(class|components|title)[:16]}`
/// Explicit fingerprints are normalized but not re-hashed.
pub fn compute_fr_fingerprint(req: &FeatureRequestReport) -> String {
    if let Some(ref explicit) = req.fingerprint {
        let cleaned = normalize_token(explicit);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    let mut parts: Vec<String> = Vec::with_capacity(4);
    parts.push(req.request_class.as_str().to_string());

    let mut components = req.component.clone();
    components.sort();
    components.dedup();
    if !components.is_empty() {
        parts.push(components.join("+"));
    }

    // Title is always part of FR fingerprints — same class, many intents.
    let title_key = normalize_token(&req.title);
    let short: String = title_key.chars().take(64).collect();
    if !short.is_empty() {
        parts.push(short);
    }

    // Provider-facing requests include provider slug when present.
    if matches!(
        req.request_class,
        RequestClass::ProviderModel | RequestClass::McpIntegration
    ) {
        if let Some(ref p) = req.environment.provider {
            let p = normalize_token(p);
            if !p.is_empty() {
                parts.push(p);
            }
        }
    }

    let material = parts.join("|");
    let digest = Sha256::digest(material.as_bytes());
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{}-{}", req.request_class.as_str(), &hex[..16])
}

fn normalize_token(s: &str) -> String {
    s.trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '|')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature_request::schema::{FeatureRequestReport, RequestClass};
    use crate::schema::Environment;

    #[test]
    fn same_class_components_title_same_fp() {
        let a = FeatureRequestReport {
            title: "Keep-N art workers".into(),
            summary: "need continuous art agents".into(),
            request_class: RequestClass::Scheduler,
            component: vec!["scheduler".into(), "subagent".into()],
            ..Default::default()
        };
        let mut b = a.clone();
        b.component = vec!["subagent".into(), "scheduler".into()];
        assert_eq!(compute_fr_fingerprint(&a), compute_fr_fingerprint(&b));
    }

    #[test]
    fn different_titles_differ() {
        let a = FeatureRequestReport {
            title: "Feature A".into(),
            summary: "s".into(),
            request_class: RequestClass::ToolSurface,
            ..Default::default()
        };
        let mut b = a.clone();
        b.title = "Feature B".into();
        assert_ne!(compute_fr_fingerprint(&a), compute_fr_fingerprint(&b));
    }

    #[test]
    fn provider_included_for_provider_class() {
        let a = FeatureRequestReport {
            title: "Better routing".into(),
            summary: "s".into(),
            request_class: RequestClass::ProviderModel,
            environment: Environment {
                provider: Some("platform/nvidia".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut b = a.clone();
        b.environment.provider = Some("xai".into());
        assert_ne!(compute_fr_fingerprint(&a), compute_fr_fingerprint(&b));
    }

    #[test]
    fn explicit_fingerprint_normalized() {
        let req = FeatureRequestReport {
            title: "x".into(),
            summary: "y".into(),
            fingerprint: Some("  Keep N Workers  ".into()),
            ..Default::default()
        };
        assert_eq!(compute_fr_fingerprint(&req), "keep_n_workers");
    }
}
