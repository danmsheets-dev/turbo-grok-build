//! Runtime auto-detectors that file structured product incidents.

use crate::schema::{
    Environment, ErrorClass, Evidence, IncidentKind, Repro, ReproConfidence, ReportRequest,
    ReportResult, ReporterKind, Severity, Source,
};
use crate::store::{DeveloperLogStore, report_best_effort};

/// Context for worktree dispose-time checks.
#[derive(Debug, Clone, Default)]
pub struct WorktreeDisposeSignal {
    pub subagent_id: String,
    pub parent_session_id: Option<String>,
    pub session_id: Option<String>,
    pub worktree_path: Option<String>,
    pub worktree_removed: bool,
    pub worktree_state: Option<String>,
    pub snapshot_ref: Option<String>,
    pub patch_path: Option<String>,
    pub meta_path: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
}

/// Whether dispose should file a work-loss incident.
pub fn worktree_dispose_is_risky(signal: &WorktreeDisposeSignal) -> bool {
    if !signal.worktree_removed {
        return false;
    }
    let has_snapshot = signal
        .snapshot_ref
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let has_patch = signal
        .patch_path
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    !has_snapshot && !has_patch
}

fn worktree_dispose_request(signal: &WorktreeDisposeSignal) -> ReportRequest {
    ReportRequest {
        title: "Subagent worktree removed without recoverable snapshot".into(),
        summary: format!(
            "Subagent `{}` worktree was deleted after completion/failure but no `snapshot_ref` or `changes.patch` was persisted. Supervisors cannot recover agent edits.",
            signal.subagent_id
        ),
        kind: Some(IncidentKind::Bug),
        severity: Some(Severity::P0),
        error_class: ErrorClass::WorkLostRisk,
        component: vec![
            "subagent".into(),
            "worktree".into(),
            "lifecycle".into(),
        ],
        environment: Environment {
            session_id: signal.session_id.clone(),
            parent_session_id: signal.parent_session_id.clone(),
            subagent_id: Some(signal.subagent_id.clone()),
            model: signal.model.clone(),
            provider: signal.provider.clone(),
            ..Default::default()
        },
        repro: Repro {
            steps: vec![
                "spawn_subagent with isolation=worktree".into(),
                "let subagent complete or fail".into(),
                "observe worktree_path removed without snapshot_ref/patch".into(),
            ],
            expected: Some(
                "Always leave snapshot_ref and/or changes.patch before deleting the worktree"
                    .into(),
            ),
            actual: Some(format!(
                "worktree_removed=true snapshot_ref={:?} patch_path={:?} state={:?}",
                signal.snapshot_ref, signal.patch_path, signal.worktree_state
            )),
            confidence: ReproConfidence::High,
        },
        evidence: Evidence {
            session_ref: signal.session_id.clone(),
            meta_path: signal.meta_path.clone(),
            snapshot_ref: signal.snapshot_ref.clone(),
            patch_path: signal.patch_path.clone(),
            related_events: vec!["subagent.worktree.dispose".into()],
            notes: signal.worktree_path.clone().map(|p| format!("last_path={p}")),
            ..Default::default()
        },
        suggested_fix: Some(
            "Never delete a subagent worktree without first writing snapshot_ref + changes.patch; prefer retain_until_land."
                .into(),
        ),
        source: Source {
            reporter: ReporterKind::Runtime,
            auto: true,
            detector: Some("worktree_dispose".into()),
            tool: None,
            reporter_model: signal.model.clone(),
        },
        tags: vec!["auto".into(), "worktree".into()],
        fingerprint: Some("work_lost_risk|dispose_without_artifacts".into()),
    }
}

/// Detect work-loss risk / tombstone-style dispose issues.
///
/// Fires when the live worktree was removed **without** a snapshot_ref and
/// without a changes.patch — the classic "work disappeared" failure mode.
pub fn detect_worktree_dispose(signal: &WorktreeDisposeSignal) -> Option<ReportResult> {
    if !worktree_dispose_is_risky(signal) {
        return None;
    }
    report_best_effort(worktree_dispose_request(signal))
}

/// Same as [`detect_worktree_dispose`] but writes to an explicit store (tests).
pub fn detect_worktree_dispose_in(
    store: &DeveloperLogStore,
    signal: &WorktreeDisposeSignal,
) -> Option<ReportResult> {
    if !worktree_dispose_is_risky(signal) {
        return None;
    }
    store.report(worktree_dispose_request(signal)).ok()
}

/// Provider / protocol failure signal.
#[derive(Debug, Clone, Default)]
pub struct ProviderFailureSignal {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub subagent_id: Option<String>,
    pub status_code: Option<u16>,
    pub error_class: ErrorClass,
    pub message: String,
    pub fingerprint_extra: Option<String>,
}

/// File a provider/protocol incident (deser, 400, auth).
pub fn detect_provider_failure(signal: &ProviderFailureSignal) -> Option<ReportResult> {
    if signal.message.trim().is_empty() {
        return None;
    }
    let class = signal.error_class;
    if !matches!(
        class,
        ErrorClass::ProtocolDeser
            | ErrorClass::Provider400
            | ErrorClass::ProviderAuth
            | ErrorClass::CatalogStale
    ) {
        return None;
    }
    let title = match class {
        ErrorClass::ProtocolDeser => "Provider stream/tool deserialization failure",
        ErrorClass::Provider400 => "Provider rejected request (HTTP 400)",
        ErrorClass::ProviderAuth => "Provider authentication failure",
        ErrorClass::CatalogStale => "Catalog model missing or EOL",
        _ => "Provider failure",
    };
    let fp = format!(
        "{}|{}|{}",
        class.as_str(),
        signal.provider.as_deref().unwrap_or("unknown"),
        signal
            .fingerprint_extra
            .as_deref()
            .unwrap_or(signal.status_code.map(|c| c.to_string()).as_deref().unwrap_or("x"))
    );
    let req = ReportRequest {
        title: title.into(),
        summary: signal.message.clone(),
        kind: Some(IncidentKind::ProviderCompat),
        severity: Some(class.default_severity()),
        error_class: class,
        component: vec!["provider".into(), "sampling".into()],
        environment: Environment {
            provider: signal.provider.clone(),
            model: signal.model.clone(),
            session_id: signal.session_id.clone(),
            subagent_id: signal.subagent_id.clone(),
            ..Default::default()
        },
        repro: Repro {
            steps: vec!["run tool-using turn against this provider/model".into()],
            expected: Some("successful tool/stream parse and accepted request".into()),
            actual: Some(signal.message.clone()),
            confidence: ReproConfidence::Medium,
        },
        evidence: Evidence {
            related_events: vec!["provider.failure".into()],
            notes: signal.status_code.map(|c| format!("http_status={c}")),
            ..Default::default()
        },
        suggested_fix: None,
        source: Source {
            reporter: ReporterKind::Runtime,
            auto: true,
            detector: Some("provider_failure".into()),
            ..Default::default()
        },
        tags: vec!["auto".into(), "provider".into()],
        fingerprint: Some(fp),
    };
    report_best_effort(req)
}

/// Subagent stall / timeout signal.
#[derive(Debug, Clone, Default)]
pub struct StallSignal {
    pub subagent_id: String,
    pub session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub duration_ms: Option<u64>,
    pub last_tool: Option<String>,
    pub reason: String,
}

/// File a subagent stall incident.
pub fn detect_subagent_stall(signal: &StallSignal) -> Option<ReportResult> {
    if signal.subagent_id.trim().is_empty() {
        return None;
    }
    let req = ReportRequest {
        title: "Subagent stalled or timed out without progress".into(),
        summary: format!(
            "Subagent `{}` stalled: {}. duration_ms={:?} last_tool={:?}",
            signal.subagent_id, signal.reason, signal.duration_ms, signal.last_tool
        ),
        kind: Some(IncidentKind::Bug),
        severity: Some(Severity::P1),
        error_class: ErrorClass::SubagentStall,
        component: vec!["subagent".into(), "lifecycle".into()],
        environment: Environment {
            session_id: signal.session_id.clone(),
            parent_session_id: signal.parent_session_id.clone(),
            subagent_id: Some(signal.subagent_id.clone()),
            model: signal.model.clone(),
            provider: signal.provider.clone(),
            ..Default::default()
        },
        repro: Repro {
            steps: vec![
                "spawn_subagent".into(),
                "observe no progress beyond timeout/stall detector".into(),
            ],
            expected: Some("progress heartbeats or hard timeout with snapshot".into()),
            actual: Some(signal.reason.clone()),
            confidence: ReproConfidence::Medium,
        },
        evidence: Evidence {
            related_events: vec!["subagent.stall".into()],
            notes: signal.last_tool.clone().map(|t| format!("last_tool={t}")),
            ..Default::default()
        },
        suggested_fix: Some(
            "Enforce timeout_ms / stall_timeout_ms; always snapshot on cancel/timeout.".into(),
        ),
        source: Source {
            reporter: ReporterKind::Runtime,
            auto: true,
            detector: Some("subagent_stall".into()),
            reporter_model: signal.model.clone(),
            ..Default::default()
        },
        tags: vec!["auto".into(), "stall".into()],
        fingerprint: Some(format!(
            "subagent_stall|{}",
            signal.provider.as_deref().unwrap_or("any")
        )),
    };
    report_best_effort(req)
}

/// Isolation fell back to shared cwd after worktree create failed.
#[derive(Debug, Clone, Default)]
pub struct IsolationFallbackSignal {
    pub subagent_id: String,
    pub session_id: Option<String>,
    pub reason: String,
}

pub fn detect_isolation_fallback(signal: &IsolationFallbackSignal) -> Option<ReportResult> {
    if signal.subagent_id.trim().is_empty() {
        return None;
    }
    let req = ReportRequest {
        title: "Subagent isolation fell back to shared workspace".into(),
        summary: format!(
            "Subagent `{}` could not create an isolated worktree and fell back to the parent cwd. Reason: {}",
            signal.subagent_id, signal.reason
        ),
        kind: Some(IncidentKind::Bug),
        severity: Some(Severity::P1),
        error_class: ErrorClass::IsolationFallback,
        component: vec!["subagent".into(), "worktree".into(), "isolation".into()],
        environment: Environment {
            session_id: signal.session_id.clone(),
            subagent_id: Some(signal.subagent_id.clone()),
            ..Default::default()
        },
        repro: Repro {
            steps: vec!["spawn_subagent isolation=worktree when worktree create fails".into()],
            expected: Some("fail closed or retry; never silent shared-cwd writes".into()),
            actual: Some(signal.reason.clone()),
            confidence: ReproConfidence::High,
        },
        evidence: Evidence {
            related_events: vec!["subagent.isolation_fallback".into()],
            ..Default::default()
        },
        suggested_fix: Some(
            "Fail spawn when isolation=worktree cannot be created; never silently share parent tree."
                .into(),
        ),
        source: Source {
            reporter: ReporterKind::Runtime,
            auto: true,
            detector: Some("isolation_fallback".into()),
            ..Default::default()
        },
        tags: vec!["auto".into(), "isolation".into()],
        fingerprint: Some("isolation_fallback|worktree_create".into()),
    };
    report_best_effort(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DeveloperLogStore, ListFilter};

    #[test]
    fn dispose_without_artifacts_files_incident() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeveloperLogStore::new(dir.path().to_path_buf());
        let result = detect_worktree_dispose_in(
            &store,
            &WorktreeDisposeSignal {
                subagent_id: "sa-test".into(),
                worktree_removed: true,
                snapshot_ref: None,
                patch_path: None,
                worktree_state: Some("cleaned".into()),
                ..Default::default()
            },
        );
        assert!(result.is_some());
        let list = store
            .list(&ListFilter {
                include_closed: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].error_class, "work_lost_risk");
    }

    #[test]
    fn dispose_with_snapshot_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let store = DeveloperLogStore::new(dir.path().to_path_buf());
        let result = detect_worktree_dispose_in(
            &store,
            &WorktreeDisposeSignal {
                subagent_id: "sa-ok".into(),
                worktree_removed: true,
                snapshot_ref: Some("refs/grok/subagents/sa-ok".into()),
                patch_path: Some("changes.patch".into()),
                ..Default::default()
            },
        );
        assert!(result.is_none());
        assert!(!worktree_dispose_is_risky(&WorktreeDisposeSignal {
            subagent_id: "sa-ok".into(),
            worktree_removed: true,
            snapshot_ref: Some("refs/grok/subagents/sa-ok".into()),
            ..Default::default()
        }));
    }
}
