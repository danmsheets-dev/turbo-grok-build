//! Push / pull orchestration. Local JSON remains the write-ahead log.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::mapping::{
    LogKind, feature_issue_body, feature_labels, feature_remote_status,
    feature_status_from_remote, fingerprint_from_labels, incident_issue_body, incident_labels,
    incident_remote_status, incident_status_from_remote, label_diff, parse_marker,
    proving_sha_comment, seen_comment,
};
use super::{GithubTransport, IssueDraft, RemoteIssue, SyncDirection, SyncError};
use crate::feature_request::schema::RequestStatus;
use crate::feature_request::store::{FeatureRequestStore, FrListFilter};
use crate::schema::IncidentStatus;
use crate::store::{DeveloperLogStore, ListFilter};

/// Operator-facing options for a CLI sync.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub repo: String,
    pub direction: SyncDirection,
}

/// Result of a push/pull pass.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub repo: String,
    pub is_private: bool,
    pub private_warning: bool,
    pub created: u32,
    pub updated: u32,
    pub skipped: u32,
    pub skipped_reasons: Vec<String>,
    pub pulled: u32,
    pub comments: u32,
}

impl SyncReport {
    pub fn human_summary(&self, kind: &str) -> String {
        let vis = if self.is_private { "private" } else { "public" };
        let mut s = format!(
            "GitHub sync ({kind}) → {} ({vis})\n  pushed:  {} created, {} updated, {} skipped\n  pulled:  {} status close(s)\n  comments: {}",
            self.repo, self.created, self.updated, self.skipped, self.pulled, self.comments
        );
        if self.private_warning {
            s.push_str(
                "\nWarning: this is a private GitHub repository. Already-redacted JSON is uploaded as Issues. Disable with github_sync = \"off\".",
            );
        }
        for r in &self.skipped_reasons {
            s.push('\n');
            s.push_str("  skip: ");
            s.push_str(r);
        }
        s
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct GithubIndex {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    private_warned: bool,
    #[serde(default)]
    items: HashMap<String, GithubIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GithubIndexEntry {
    number: u64,
    #[serde(default)]
    url: String,
    #[serde(default)]
    occurrence_count: u32,
    #[serde(default)]
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proving_sha: Option<String>,
}

fn index_path(root: &Path) -> PathBuf {
    root.join("github-index.json")
}

fn load_index(root: &Path, repo: &str) -> GithubIndex {
    let path = index_path(root);
    let mut idx = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(GithubIndex::default);
    if idx.repo != repo {
        idx = GithubIndex {
            repo: repo.to_string(),
            private_warned: false,
            items: HashMap::new(),
        };
    }
    idx
}

fn save_index(root: &Path, idx: &GithubIndex) -> Result<(), SyncError> {
    let path = index_path(root);
    let tmp = path.with_extension("json.tmp");
    let pretty = serde_json::to_string_pretty(idx)?;
    std::fs::write(&tmp, pretty)?;
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn lookup_remote(
    gh: &dyn GithubTransport,
    repo: &str,
    fingerprint: &str,
    listed: &HashMap<String, RemoteIssue>,
    idx: &GithubIndex,
) -> Result<Option<RemoteIssue>, SyncError> {
    if let Some(r) = listed.get(fingerprint) {
        return Ok(Some(r.clone()));
    }
    if let Some(e) = idx.items.get(fingerprint) {
        if let Some(r) = listed.values().find(|r| r.number == e.number) {
            return Ok(Some(r.clone()));
        }
        // Index hit but not in the labeled list — still try search.
    }
    gh.search_fingerprint(repo, fingerprint)
}

fn remote_map(issues: Vec<RemoteIssue>) -> HashMap<String, RemoteIssue> {
    let mut map = HashMap::new();
    for issue in issues {
        if let Some(fp) = fingerprint_from_labels(&issue.labels)
            .or_else(|| parse_marker(&issue.body).map(|(fp, _)| fp))
        {
            map.entry(fp).or_insert(issue);
        }
    }
    map
}

fn title_for_github(title: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        "untitled".into()
    } else if t.starts_with('-') {
        format!(".{t}")
    } else {
        t.chars().take(200).collect()
    }
}

/// Push + optional pull for all incidents in `store`.
pub fn sync_incidents(
    store: &DeveloperLogStore,
    gh: &dyn GithubTransport,
    opts: &SyncOptions,
) -> Result<SyncReport, SyncError> {
    gh.ensure_gh_and_auth()?;
    let meta = gh.repo_meta(&opts.repo)?;
    let mut report = SyncReport {
        repo: meta.name_with_owner.clone(),
        is_private: meta.is_private,
        ..Default::default()
    };
    let mut idx = load_index(store.root(), &opts.repo);
    if meta.is_private && !idx.private_warned {
        report.private_warning = true;
        idx.private_warned = true;
    }
    let listed = if matches!(opts.direction, SyncDirection::Push | SyncDirection::Both)
        || matches!(opts.direction, SyncDirection::Pull | SyncDirection::Both)
    {
        remote_map(gh.list_issues(&opts.repo, LogKind::Incident)?)
    } else {
        HashMap::new()
    };

    if matches!(opts.direction, SyncDirection::Push | SyncDirection::Both) {
        let entries = store
            .list(&ListFilter {
                include_closed: true,
                ..Default::default()
            })
            .map_err(|e| SyncError::Store(e.to_string()))?;
        for e in entries {
            match push_incident(
                store,
                gh,
                &opts.repo,
                &e.fingerprint,
                &listed,
                &mut idx,
                &mut report,
            ) {
                Ok(()) => {}
                Err(SyncError::RedactUnresolved(fp)) => {
                    report.skipped += 1;
                    report
                        .skipped_reasons
                        .push(format!("{fp}: unresolved secret shape after redaction"));
                }
                Err(SyncError::RateLimited(msg)) => return Err(SyncError::RateLimited(msg)),
                Err(err) => {
                    report.skipped += 1;
                    report
                        .skipped_reasons
                        .push(format!("{}: {err}", e.fingerprint));
                }
            }
        }
    }

    if matches!(opts.direction, SyncDirection::Pull | SyncDirection::Both) {
        pull_incidents(store, &listed, &mut report)?;
    }

    let _ = save_index(store.root(), &idx);
    Ok(report)
}

/// Push + optional pull for all feature requests in `store`.
pub fn sync_features(
    store: &FeatureRequestStore,
    gh: &dyn GithubTransport,
    opts: &SyncOptions,
) -> Result<SyncReport, SyncError> {
    gh.ensure_gh_and_auth()?;
    let meta = gh.repo_meta(&opts.repo)?;
    let mut report = SyncReport {
        repo: meta.name_with_owner.clone(),
        is_private: meta.is_private,
        ..Default::default()
    };
    let mut idx = load_index(store.root(), &opts.repo);
    if meta.is_private && !idx.private_warned {
        report.private_warning = true;
        idx.private_warned = true;
    }
    let listed = remote_map(gh.list_issues(&opts.repo, LogKind::Feature)?);

    if matches!(opts.direction, SyncDirection::Push | SyncDirection::Both) {
        let entries = store
            .list(&FrListFilter {
                include_closed: true,
                ..Default::default()
            })
            .map_err(|e| SyncError::Store(e.to_string()))?;
        for e in entries {
            match push_feature(
                store,
                gh,
                &opts.repo,
                &e.fingerprint,
                &listed,
                &mut idx,
                &mut report,
            ) {
                Ok(()) => {}
                Err(SyncError::RedactUnresolved(fp)) => {
                    report.skipped += 1;
                    report
                        .skipped_reasons
                        .push(format!("{fp}: unresolved secret shape after redaction"));
                }
                Err(SyncError::RateLimited(msg)) => return Err(SyncError::RateLimited(msg)),
                Err(err) => {
                    report.skipped += 1;
                    report
                        .skipped_reasons
                        .push(format!("{}: {err}", e.fingerprint));
                }
            }
        }
    }

    if matches!(opts.direction, SyncDirection::Pull | SyncDirection::Both) {
        pull_features(store, &listed, &mut report)?;
    }

    let _ = save_index(store.root(), &idx);
    Ok(report)
}

/// Push a single incident (used by on-file). Auth is still required.
pub fn sync_one_incident(
    store: &DeveloperLogStore,
    gh: &dyn GithubTransport,
    repo: &str,
    fingerprint: &str,
) -> Result<(), SyncError> {
    gh.ensure_gh_and_auth()?;
    let mut idx = load_index(store.root(), repo);
    let listed = remote_map(gh.list_issues(repo, LogKind::Incident).unwrap_or_default());
    let mut report = SyncReport::default();
    push_incident(store, gh, repo, fingerprint, &listed, &mut idx, &mut report)?;
    let _ = save_index(store.root(), &idx);
    Ok(())
}

/// Push a single feature request (used by on-file).
pub fn sync_one_feature(
    store: &FeatureRequestStore,
    gh: &dyn GithubTransport,
    repo: &str,
    fingerprint: &str,
) -> Result<(), SyncError> {
    gh.ensure_gh_and_auth()?;
    let mut idx = load_index(store.root(), repo);
    let listed = remote_map(gh.list_issues(repo, LogKind::Feature).unwrap_or_default());
    let mut report = SyncReport::default();
    push_feature(store, gh, repo, fingerprint, &listed, &mut idx, &mut report)?;
    let _ = save_index(store.root(), &idx);
    Ok(())
}

fn push_incident(
    store: &DeveloperLogStore,
    gh: &dyn GithubTransport,
    repo: &str,
    fingerprint: &str,
    listed: &HashMap<String, RemoteIssue>,
    idx: &mut GithubIndex,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    let inc = store
        .get(fingerprint)
        .map_err(|e| SyncError::Store(e.to_string()))?;
    let body =
        incident_issue_body(&inc).map_err(|_| SyncError::RedactUnresolved(fingerprint.into()))?;
    let labels = incident_labels(&inc);
    let (state, _) = incident_remote_status(inc.status);
    let draft = IssueDraft {
        title: title_for_github(&inc.title),
        body,
        labels: labels.clone(),
        state,
    };
    let existing = lookup_remote(gh, repo, fingerprint, listed, idx)?;
    let remote = if let Some(cur) = existing {
        let (add, remove) = label_diff(&cur.labels, &labels);
        let updated = gh.update_issue(repo, cur.number, &draft, &add, &remove)?;
        report.updated += 1;
        maybe_comment_incident(gh, repo, &inc, &cur, idx, report)?;
        updated
    } else {
        let created = gh.create_issue(repo, &draft)?;
        report.created += 1;
        if let Some(sha) = inc.resolution_sha.as_deref() {
            let c = proving_sha_comment(LogKind::Incident, sha, inc.resolution_note.as_deref());
            if gh.comment(repo, created.number, &c).is_ok() {
                report.comments += 1;
            }
        }
        created
    };
    idx.items.insert(
        fingerprint.to_string(),
        GithubIndexEntry {
            number: remote.number,
            url: remote.url,
            occurrence_count: inc.occurrence_count,
            status: inc.status.as_str().to_string(),
            proving_sha: inc.resolution_sha.clone(),
        },
    );
    Ok(())
}

fn push_feature(
    store: &FeatureRequestStore,
    gh: &dyn GithubTransport,
    repo: &str,
    fingerprint: &str,
    listed: &HashMap<String, RemoteIssue>,
    idx: &mut GithubIndex,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    let fr = store
        .get(fingerprint)
        .map_err(|e| SyncError::Store(e.to_string()))?;
    let body =
        feature_issue_body(&fr).map_err(|_| SyncError::RedactUnresolved(fingerprint.into()))?;
    let labels = feature_labels(&fr);
    let (state, _) = feature_remote_status(fr.status);
    let draft = IssueDraft {
        title: title_for_github(&fr.title),
        body,
        labels: labels.clone(),
        state,
    };
    let existing = lookup_remote(gh, repo, fingerprint, listed, idx)?;
    let remote = if let Some(cur) = existing {
        let (add, remove) = label_diff(&cur.labels, &labels);
        let updated = gh.update_issue(repo, cur.number, &draft, &add, &remove)?;
        report.updated += 1;
        maybe_comment_feature(gh, repo, &fr, &cur, idx, report)?;
        updated
    } else {
        let created = gh.create_issue(repo, &draft)?;
        report.created += 1;
        if let Some(sha) = fr.ship_sha.as_deref() {
            let c = proving_sha_comment(LogKind::Feature, sha, fr.ship_note.as_deref());
            if gh.comment(repo, created.number, &c).is_ok() {
                report.comments += 1;
            }
        }
        created
    };
    idx.items.insert(
        fingerprint.to_string(),
        GithubIndexEntry {
            number: remote.number,
            url: remote.url,
            occurrence_count: fr.occurrence_count,
            status: fr.status.as_str().to_string(),
            proving_sha: fr.ship_sha.clone(),
        },
    );
    Ok(())
}

fn maybe_comment_incident(
    gh: &dyn GithubTransport,
    repo: &str,
    inc: &crate::schema::Incident,
    cur: &RemoteIssue,
    idx: &GithubIndex,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    let prev = idx.items.get(&inc.fingerprint);
    let prev_count = prev.map(|p| p.occurrence_count).unwrap_or(0);
    if inc.occurrence_count > prev_count && prev_count > 0 {
        if gh
            .comment(repo, cur.number, &seen_comment(inc.occurrence_count))
            .is_ok()
        {
            report.comments += 1;
        }
    }
    if let Some(sha) = inc.resolution_sha.as_deref() {
        let already = prev
            .and_then(|p| p.proving_sha.as_deref())
            .is_some_and(|s| s == sha);
        if !already {
            let c = proving_sha_comment(LogKind::Incident, sha, inc.resolution_note.as_deref());
            if gh.comment(repo, cur.number, &c).is_ok() {
                report.comments += 1;
            }
        }
    }
    Ok(())
}

fn maybe_comment_feature(
    gh: &dyn GithubTransport,
    repo: &str,
    fr: &crate::feature_request::schema::FeatureRequest,
    cur: &RemoteIssue,
    idx: &GithubIndex,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    let prev = idx.items.get(&fr.fingerprint);
    let prev_count = prev.map(|p| p.occurrence_count).unwrap_or(0);
    if fr.occurrence_count > prev_count && prev_count > 0 {
        if gh
            .comment(repo, cur.number, &seen_comment(fr.occurrence_count))
            .is_ok()
        {
            report.comments += 1;
        }
    }
    if let Some(sha) = fr.ship_sha.as_deref() {
        let already = prev
            .and_then(|p| p.proving_sha.as_deref())
            .is_some_and(|s| s == sha);
        if !already {
            let c = proving_sha_comment(LogKind::Feature, sha, fr.ship_note.as_deref());
            if gh.comment(repo, cur.number, &c).is_ok() {
                report.comments += 1;
            }
        }
    }
    Ok(())
}

fn pull_incidents(
    store: &DeveloperLogStore,
    listed: &HashMap<String, RemoteIssue>,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    for (fp, remote) in listed {
        let Some(status) = incident_status_from_remote(remote.state, &remote.labels) else {
            continue;
        };
        let Ok(local) = store.get(fp) else {
            continue;
        };
        if local.status == status {
            continue;
        }
        if !matches!(status, IncidentStatus::Resolved | IncidentStatus::Wontdo) {
            continue;
        }
        store
            .set_status(fp, status)
            .map_err(|e| SyncError::Store(e.to_string()))?;
        report.pulled += 1;
    }
    Ok(())
}

fn pull_features(
    store: &FeatureRequestStore,
    listed: &HashMap<String, RemoteIssue>,
    report: &mut SyncReport,
) -> Result<(), SyncError> {
    for (fp, remote) in listed {
        let Some(status) = feature_status_from_remote(remote.state, &remote.labels) else {
            continue;
        };
        let Ok(local) = store.get(fp) else {
            continue;
        };
        if local.status == status {
            continue;
        }
        if !matches!(status, RequestStatus::Shipped | RequestStatus::Declined) {
            continue;
        }
        store
            .set_status(fp, status)
            .map_err(|e| SyncError::Store(e.to_string()))?;
        report.pulled += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature_request::schema::{FeatureRequestReport, RequestClass};
    use crate::github_sync::{IssueDraft, RepoMeta};
    use crate::schema::{ErrorClass, ReportRequest};
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryGithub {
        inner: Mutex<MemInner>,
    }

    #[derive(Default)]
    struct MemInner {
        authed: bool,
        missing: bool,
        private: bool,
        issues: Vec<RemoteIssue>,
        comments: Vec<(u64, String)>,
        next_number: u64,
    }

    impl MemoryGithub {
        fn new(_repo: &str, private: bool) -> Self {
            Self {
                inner: Mutex::new(MemInner {
                    authed: true,
                    missing: false,
                    private,
                    issues: Vec::new(),
                    comments: Vec::new(),
                    next_number: 1,
                }),
            }
        }

        fn issue_count(&self) -> usize {
            self.inner.lock().unwrap().issues.len()
        }

        fn issue_bodies(&self) -> Vec<String> {
            self.inner
                .lock()
                .unwrap()
                .issues
                .iter()
                .map(|i| i.body.clone())
                .collect()
        }

        fn comments(&self) -> Vec<(u64, String)> {
            self.inner.lock().unwrap().comments.clone()
        }
    }

    impl GithubTransport for MemoryGithub {
        fn ensure_gh_and_auth(&self) -> Result<(), SyncError> {
            let g = self.inner.lock().unwrap();
            if g.missing {
                return Err(SyncError::GhMissing);
            }
            if !g.authed {
                return Err(SyncError::GhUnauthenticated("not logged in".into()));
            }
            Ok(())
        }

        fn repo_meta(&self, repo: &str) -> Result<RepoMeta, SyncError> {
            let g = self.inner.lock().unwrap();
            Ok(RepoMeta {
                name_with_owner: repo.to_string(),
                is_private: g.private,
                url: format!("https://github.com/{repo}"),
            })
        }

        fn list_issues(&self, _repo: &str, kind: LogKind) -> Result<Vec<RemoteIssue>, SyncError> {
            let g = self.inner.lock().unwrap();
            let want = kind.type_label();
            Ok(g.issues
                .iter()
                .filter(|i| i.labels.iter().any(|l| l == want))
                .cloned()
                .collect())
        }

        fn search_fingerprint(
            &self,
            _repo: &str,
            fingerprint: &str,
        ) -> Result<Option<RemoteIssue>, SyncError> {
            let g = self.inner.lock().unwrap();
            Ok(g.issues
                .iter()
                .find(|i| i.body.contains(fingerprint))
                .cloned())
        }

        fn ensure_label(&self, _repo: &str, _name: &str) -> Result<(), SyncError> {
            Ok(())
        }

        fn create_issue(&self, repo: &str, draft: &IssueDraft) -> Result<RemoteIssue, SyncError> {
            let mut g = self.inner.lock().unwrap();
            let number = g.next_number;
            g.next_number += 1;
            let issue = RemoteIssue {
                number,
                title: draft.title.clone(),
                body: draft.body.clone(),
                state: draft.state,
                labels: draft.labels.clone(),
                url: format!("https://github.com/{repo}/issues/{number}"),
            };
            g.issues.push(issue.clone());
            Ok(issue)
        }

        fn update_issue(
            &self,
            _repo: &str,
            number: u64,
            draft: &IssueDraft,
            add_labels: &[String],
            remove_labels: &[String],
        ) -> Result<RemoteIssue, SyncError> {
            let mut g = self.inner.lock().unwrap();
            let issue = g
                .issues
                .iter_mut()
                .find(|i| i.number == number)
                .ok_or_else(|| SyncError::Gh("not found".into()))?;
            issue.title = draft.title.clone();
            issue.body = draft.body.clone();
            issue.state = draft.state;
            for r in remove_labels {
                issue.labels.retain(|l| l != r);
            }
            for a in add_labels {
                if !issue.labels.iter().any(|l| l == a) {
                    issue.labels.push(a.clone());
                }
            }
            Ok(issue.clone())
        }

        fn comment(&self, _repo: &str, number: u64, body: &str) -> Result<(), SyncError> {
            self.inner
                .lock()
                .unwrap()
                .comments
                .push((number, body.to_string()));
            Ok(())
        }

        fn set_state(&self, _repo: &str, number: u64, state: RemoteState) -> Result<(), SyncError> {
            let mut g = self.inner.lock().unwrap();
            if let Some(i) = g.issues.iter_mut().find(|i| i.number == number) {
                i.state = state;
            }
            Ok(())
        }
    }

    fn tmp_incident_store() -> (tempfile::TempDir, DeveloperLogStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DeveloperLogStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn same_fingerprint_upserts_one_issue() {
        let (_dir, store) = tmp_incident_store();
        let req = ReportRequest {
            title: "Worktree path unusable after complete".into(),
            summary: "meta still points at deleted worktree".into(),
            error_class: ErrorClass::WorktreeTombstone,
            component: vec!["worktree".into()],
            ..Default::default()
        };
        store.report(req.clone()).unwrap();
        let gh = MemoryGithub::new("o/logs", true);
        let opts = SyncOptions {
            repo: "o/logs".into(),
            direction: SyncDirection::Push,
        };
        let r1 = sync_incidents(&store, &gh, &opts).unwrap();
        assert_eq!(r1.created, 1);
        assert_eq!(r1.updated, 0);
        assert_eq!(gh.issue_count(), 1);
        assert!(r1.private_warning);
        store.report(req).unwrap();
        let r2 = sync_incidents(&store, &gh, &opts).unwrap();
        assert_eq!(r2.created, 0);
        assert_eq!(r2.updated, 1);
        assert_eq!(gh.issue_count(), 1);
        assert!(!r2.private_warning, "private warning is first-sync only");
        let comments = gh.comments();
        assert!(
            comments.iter().any(|(_, b)| b.contains("seen 2x")),
            "occurrence bump should comment: {comments:?}"
        );
    }

    #[test]
    fn redact_strips_secrets_from_uploaded_body() {
        let (_dir, store) = tmp_incident_store();
        let ghp = ["ghp_f", "akefakefakefakefakefakefake"].concat();
        store
            .report(ReportRequest {
                title: "token leak check".into(),
                summary: format!("detail {ghp}"),
                error_class: ErrorClass::ToolSchema,
                ..Default::default()
            })
            .unwrap();
        let gh = MemoryGithub::new("o/logs", false);
        sync_incidents(
            &store,
            &gh,
            &SyncOptions {
                repo: "o/logs".into(),
                direction: SyncDirection::Push,
            },
        )
        .unwrap();
        let bodies = gh.issue_bodies();
        assert_eq!(bodies.len(), 1);
        assert!(
            !bodies[0].contains("ghp_f"),
            "secret survived into GH body: {}",
            bodies[0]
        );
    }

    #[test]
    fn resolve_closes_remote_with_label_and_sha_comment() {
        let (_dir, store) = tmp_incident_store();
        let r = store
            .report(ReportRequest {
                title: "keep-N deleted retain tree".into(),
                summary: "retain_worktree was pruned".into(),
                error_class: ErrorClass::WorkLostRisk,
                ..Default::default()
            })
            .unwrap();
        store
            .set_status_with(
                &r.incident_id,
                IncidentStatus::Resolved,
                Some("abc1234"),
                Some("fix"),
            )
            .unwrap();
        let gh = MemoryGithub::new("o/logs", false);
        sync_incidents(
            &store,
            &gh,
            &SyncOptions {
                repo: "o/logs".into(),
                direction: SyncDirection::Push,
            },
        )
        .unwrap();
        let issues = gh.inner.lock().unwrap().issues.clone();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].state, RemoteState::Closed);
        assert!(issues[0].labels.iter().any(|l| l == "resolved"));
        assert!(
            gh.comments().iter().any(|(_, b)| b.contains("`abc1234`")),
            "proving sha comment missing"
        );
    }

    #[test]
    fn pull_close_updates_local_status() {
        let (_dir, store) = tmp_incident_store();
        let r = store
            .report(ReportRequest {
                title: "A".into(),
                summary: "s".into(),
                error_class: ErrorClass::DocsGap,
                ..Default::default()
            })
            .unwrap();
        let gh = MemoryGithub::new("o/logs", false);
        sync_incidents(
            &store,
            &gh,
            &SyncOptions {
                repo: "o/logs".into(),
                direction: SyncDirection::Push,
            },
        )
        .unwrap();
        {
            let mut g = gh.inner.lock().unwrap();
            g.issues[0].state = RemoteState::Closed;
            g.issues[0].labels.push("resolved".into());
        }
        sync_incidents(
            &store,
            &gh,
            &SyncOptions {
                repo: "o/logs".into(),
                direction: SyncDirection::Pull,
            },
        )
        .unwrap();
        let got = store.get(&r.incident_id).unwrap();
        assert_eq!(got.status, IncidentStatus::Resolved);
    }

    #[test]
    fn default_sync_without_repo_is_error() {
        let cfg = crate::log_config::LogFileConfig::default();
        assert!(matches!(
            crate::github_sync::resolve_repo(None, &cfg),
            Err(SyncError::RepoUnset)
        ));
    }

    #[test]
    fn missing_gh_fails_cli_sync() {
        let (_dir, store) = tmp_incident_store();
        store
            .report(ReportRequest {
                title: "A".into(),
                summary: "s".into(),
                error_class: ErrorClass::Unknown,
                ..Default::default()
            })
            .unwrap();
        let gh = MemoryGithub::new("o/logs", false);
        gh.inner.lock().unwrap().missing = true;
        let err = sync_incidents(
            &store,
            &gh,
            &SyncOptions {
                repo: "o/logs".into(),
                direction: SyncDirection::Push,
            },
        )
        .unwrap_err();
        assert!(matches!(err, SyncError::GhMissing));
    }

    #[test]
    fn feature_fingerprint_upserts_one_issue() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeatureRequestStore::new(dir.path().to_path_buf());
        let req = FeatureRequestReport {
            title: "Hull merge tool".into(),
            summary: "Need automatic hull.json merge".into(),
            request_class: RequestClass::Subagent,
            component: vec!["land".into()],
            ..Default::default()
        };
        store.report(req.clone()).unwrap();
        store.report(req).unwrap();
        let gh = MemoryGithub::new("o/logs", false);
        let r = sync_features(
            &store,
            &gh,
            &SyncOptions {
                repo: "o/logs".into(),
                direction: SyncDirection::Both,
            },
        )
        .unwrap();
        assert_eq!(r.created, 1);
        assert_eq!(gh.issue_count(), 1);
        assert!(
            gh.issue_bodies()[0].contains("kind=feature"),
            "feature marker missing"
        );
    }
}
