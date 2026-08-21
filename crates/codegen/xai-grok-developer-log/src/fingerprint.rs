//! Fingerprint computation for incident deduplication.

use sha2::{Digest, Sha256};

use crate::schema::{ErrorClass, ReportRequest};

/// Build a stable fingerprint string used as the dedup key.
///
/// Format: `sha256(error_class|sorted_components|optional_extra)[:16]`
/// when no explicit fingerprint is provided. Explicit fingerprints are
/// normalized (lowercase, whitespace collapsed) but not hashed further
/// so operators can use human-readable keys in tests and docs.
pub fn compute_fingerprint(req: &ReportRequest) -> String {
    if let Some(ref explicit) = req.fingerprint {
        let cleaned = normalize_token(explicit);
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    let mut parts: Vec<String> = Vec::with_capacity(4);
    parts.push(req.error_class.as_str().to_string());

    let mut components = req.component.clone();
    components.sort();
    components.dedup();
    if !components.is_empty() {
        parts.push(components.join("+"));
    }

    // Light title token for feature gaps / docs (same class can have many intents).
    if matches!(
        req.error_class,
        ErrorClass::FeatureGap | ErrorClass::DocsGap | ErrorClass::Unknown
    ) {
        let title_key = normalize_token(&req.title);
        let short: String = title_key.chars().take(48).collect();
        if !short.is_empty() {
            parts.push(short);
        }
    }

    // Provider-ish classes include provider slug when present.
    if matches!(
        req.error_class,
        ErrorClass::Provider400
            | ErrorClass::Provider429
            | ErrorClass::ProviderAuth
            | ErrorClass::ProtocolDeser
            | ErrorClass::CatalogStale
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
    format!("{}-{}", req.error_class.as_str(), &hex[..16])
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
    use crate::schema::{Environment, ErrorClass, ReportRequest};

    #[test]
    fn same_class_and_components_same_fingerprint() {
        let a = ReportRequest {
            title: "Worktree gone".into(),
            summary: "path missing".into(),
            error_class: ErrorClass::WorktreeTombstone,
            component: vec!["subagent".into(), "worktree".into()],
            ..Default::default()
        };
        let mut b = a.clone();
        b.component = vec!["worktree".into(), "subagent".into()];
        assert_eq!(compute_fingerprint(&a), compute_fingerprint(&b));
    }

    #[test]
    fn provider_included_for_provider_errors() {
        let mut a = ReportRequest {
            title: "400".into(),
            summary: "bad request".into(),
            error_class: ErrorClass::Provider400,
            environment: Environment {
                provider: Some("platform/nvidia".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut b = a.clone();
        b.environment.provider = Some("xai".into());
        assert_ne!(compute_fingerprint(&a), compute_fingerprint(&b));
        a.environment.provider = b.environment.provider.clone();
        assert_eq!(compute_fingerprint(&a), compute_fingerprint(&b));
    }

    #[test]
    fn explicit_fingerprint_normalized() {
        let req = ReportRequest {
            title: "x".into(),
            summary: "y".into(),
            fingerprint: Some("  Foo Bar  ".into()),
            ..Default::default()
        };
        assert_eq!(compute_fingerprint(&req), "foo_bar");
    }
}
