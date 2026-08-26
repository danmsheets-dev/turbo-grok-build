//! Opt-in GitHub Issues sync for Auto Developer Log / Feature Request Log.
//!
//! Local JSON is the write-ahead log. `developer_log` / `feature_request_log`
//! never wait on the network: `on-file` sync is a fail-open background thread
//! after the local write. Default is local-only (`github_sync = "off"`).

mod gh;
pub mod mapping;
mod sync;

pub use gh::GhCli;
pub use mapping::{LogKind, RemoteState};
pub use sync::{
    SyncOptions, SyncReport, sync_features, sync_incidents, sync_one_feature, sync_one_incident,
};

use crate::feature_request::store::fr_default_root;
use crate::log_config::{LogFileConfig, should_spawn_on_file};
use crate::store::default_root;

/// Errors from GitHub Issues sync. None of these are raised from the
/// agent tools — those stay local-only.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(
        "GitHub CLI (`gh`) is not installed or not on PATH. Install it from https://cli.github.com/ then run `gh auth login`."
    )]
    GhMissing,
    #[error("GitHub CLI is not authenticated. Run `gh auth login`. {0}")]
    GhUnauthenticated(String),
    #[error(
        "github_repo is not set. Pass `--repo owner/name` or set github_repo in ~/.grok/developer-log.toml (or feature-request-log.toml). Sync is opt-in and off by default."
    )]
    RepoUnset,
    #[error("invalid github_repo `{0}` (expected owner/name)")]
    InvalidRepo(String),
    #[error("refusing to upload `{0}`: unresolved secret shape after redaction")]
    RedactUnresolved(String),
    #[error("github issue body too large for `{0}`")]
    BodyTooLarge(String),
    #[error("`gh` timed out after {0}s")]
    Timeout(u64),
    #[error("GitHub API rate limited: {0}")]
    RateLimited(String),
    #[error(
        "GitHub Issues are disabled on `{repo}`.{fork_note} Enable them at \
         https://github.com/{repo}/settings -> General -> Features -> tick \"Issues\", then \
         re-run. Or target a repo you own with `--repo owner/name`. Nothing was lost: your \
         incidents are still in the local log."
    )]
    IssuesDisabled {
        /// `owner/name`.
        repo: String,
        /// Extra sentence when the repo is a fork, where this is the default.
        fork_note: String,
    },
    #[error(
        "Your GitHub login has {permission} access to `{repo}`; filing Issues needs WRITE or \
         higher. Ask a repo admin for write access, run \
         `gh auth refresh -h github.com -s repo` if your token is missing the `repo` scope, or \
         push to a repo you own with `--repo owner/name`. Nothing was lost: your incidents are \
         still in the local log."
    )]
    NoPushAccess {
        /// `owner/name`.
        repo: String,
        /// Permission GitHub reported, e.g. `READ`.
        permission: String,
    },
    #[error(
        "`{repo}` is archived, and GitHub rejects new Issues on archived repositories. \
         Unarchive it at https://github.com/{repo}/settings, or target another repo with \
         `--repo owner/name`. Nothing was lost: your incidents are still in the local log."
    )]
    RepoArchived {
        /// `owner/name`.
        repo: String,
    },
    #[error(
        "`{repo}` is a PUBLIC GitHub repository. Incident/feature JSON would be \
         world-readable. Point github_repo at a private repo, or set \
         GROK_GITHUB_SYNC_PUBLIC=1 to opt in. Nothing was lost: items stay in \
         the local log."
    )]
    PublicRepo {
        /// `owner/name`.
        repo: String,
    },
    #[error("`gh` failed: {0}")]
    Gh(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Store(String),
}

/// Push, pull, or both (CLI default is both).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    Push,
    Pull,
    Both,
}

/// Remote repository metadata from `gh repo view`.
#[derive(Debug, Clone)]
pub struct RepoMeta {
    pub name_with_owner: String,
    pub is_private: bool,
    pub url: String,
    /// GitHub turns Issues **off** on new forks, which is how a configured
    /// sync can look healthy and still land nothing.
    pub has_issues_enabled: bool,
    /// `ADMIN` / `MAINTAIN` / `WRITE` / `TRIAGE` / `READ`, or `None` when
    /// GitHub does not report one (some fine-grained tokens and app auth).
    pub viewer_permission: Option<String>,
    pub is_fork: bool,
    pub is_archived: bool,
}

impl RepoMeta {
    /// Whether this login can file issues here.
    ///
    /// An unknown permission counts as permissive: GitHub returns null for
    /// several legitimate auth shapes, and refusing a push that would have
    /// worked is worse than letting the API refuse it with its own message.
    pub fn can_push(&self) -> bool {
        match self.viewer_permission.as_deref() {
            None => true,
            Some(p) => matches!(
                p.to_ascii_uppercase().as_str(),
                "ADMIN" | "MAINTAIN" | "WRITE"
            ),
        }
    }

    /// Refuse early, with a remediation, when the remote cannot accept issues.
    ///
    /// Called before listing rather than after: `gh issue list` fails outright
    /// on a repo with Issues disabled, so without this the push loop never runs
    /// and the operator sees a raw API string instead of what to do about it.
    pub fn check_can_receive(&self, pushing: bool) -> Result<(), SyncError> {
        let repo = self.name_with_owner.clone();
        if self.is_archived {
            return Err(SyncError::RepoArchived { repo });
        }
        if !self.has_issues_enabled {
            return Err(SyncError::IssuesDisabled {
                fork_note: if self.is_fork {
                    " GitHub disables Issues on new forks by default.".to_string()
                } else {
                    String::new()
                },
                repo,
            });
        }
        if pushing && !self.can_push() {
            return Err(SyncError::NoPushAccess {
                permission: self
                    .viewer_permission
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                repo,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod repo_meta_tests {
    use super::*;

    fn meta() -> RepoMeta {
        RepoMeta {
            name_with_owner: "danmsheets-dev/turbo-grok-build".into(),
            is_private: false,
            url: "https://github.com/danmsheets-dev/turbo-grok-build".into(),
            has_issues_enabled: true,
            viewer_permission: Some("WRITE".into()),
            is_fork: true,
            is_archived: false,
        }
    }

    /// The field failure: a fork with Issues off. The old code found out from
    /// `gh issue list` and surfaced the raw API string.
    #[test]
    fn a_fork_with_issues_disabled_is_refused_with_a_remedy() {
        let mut m = meta();
        m.has_issues_enabled = false;
        let err = m.check_can_receive(true).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("/settings"),
            "must link the exact page: {text}"
        );
        assert!(text.contains("Features"), "{text}");
        assert!(
            text.contains("forks"),
            "a fork must be told why it is off by default: {text}"
        );
        assert!(
            text.contains("still in the local log"),
            "the operator must know nothing was lost: {text}"
        );
        // Pull is refused too: `gh issue list` fails either way.
        assert!(m.check_can_receive(false).is_err());
    }

    #[test]
    fn read_only_permission_blocks_push_but_not_pull() {
        let mut m = meta();
        m.viewer_permission = Some("READ".into());
        assert!(!m.can_push());
        let err = m.check_can_receive(true).unwrap_err();
        assert!(err.to_string().contains("READ"), "{err}");
        assert!(m.check_can_receive(false).is_ok(), "reading is still fine");
    }

    /// GitHub reports null for several legitimate auth shapes. Refusing a push
    /// that would have worked is worse than letting the API refuse it.
    #[test]
    fn unknown_viewer_permission_is_permissive() {
        let mut m = meta();
        m.viewer_permission = None;
        assert!(m.can_push());
        assert!(m.check_can_receive(true).is_ok());
    }

    #[test]
    fn write_maintain_and_admin_can_push_case_insensitively() {
        for p in ["WRITE", "maintain", "Admin"] {
            let mut m = meta();
            m.viewer_permission = Some(p.into());
            assert!(m.can_push(), "{p} must be allowed to file issues");
        }
        for p in ["READ", "TRIAGE", "NONE"] {
            let mut m = meta();
            m.viewer_permission = Some(p.into());
            assert!(!m.can_push(), "{p} must not be treated as write access");
        }
    }

    #[test]
    fn an_archived_repo_is_refused_first() {
        let mut m = meta();
        m.is_archived = true;
        m.has_issues_enabled = false;
        let err = m.check_can_receive(true).unwrap_err();
        assert!(
            matches!(err, SyncError::RepoArchived { .. }),
            "archived is the more actionable diagnosis: {err}"
        );
    }
}

/// Draft used for create/update.
#[derive(Debug, Clone)]
pub struct IssueDraft {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub state: RemoteState,
}

/// Snapshot of a GitHub issue.
#[derive(Debug, Clone)]
pub struct RemoteIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: RemoteState,
    pub labels: Vec<String>,
    pub url: String,
}

/// Transport used by sync so tests can mock `gh`.
pub trait GithubTransport {
    fn ensure_gh_and_auth(&self) -> Result<(), SyncError>;
    fn repo_meta(&self, repo: &str) -> Result<RepoMeta, SyncError>;
    fn list_issues(&self, repo: &str, kind: LogKind) -> Result<Vec<RemoteIssue>, SyncError>;
    fn search_fingerprint(
        &self,
        repo: &str,
        fingerprint: &str,
    ) -> Result<Option<RemoteIssue>, SyncError>;
    fn ensure_label(&self, repo: &str, name: &str) -> Result<(), SyncError>;
    fn create_issue(&self, repo: &str, draft: &IssueDraft) -> Result<RemoteIssue, SyncError>;
    fn update_issue(
        &self,
        repo: &str,
        number: u64,
        draft: &IssueDraft,
        add_labels: &[String],
        remove_labels: &[String],
    ) -> Result<RemoteIssue, SyncError>;
    fn comment(&self, repo: &str, number: u64, body: &str) -> Result<(), SyncError>;
    fn set_state(&self, repo: &str, number: u64, state: RemoteState) -> Result<(), SyncError>;
}

/// After a local WAL write: maybe fire a fail-open background push.
///
/// Never waits on the network. No-op when `github_sync` is not `on-file`,
/// when `github_repo` is unset, or when `store_root` is not the configured
/// operator store (so tests and one-shot temp stores stay local).
pub fn spawn_on_file_if_enabled(kind: LogKind, store_root: &std::path::Path, fingerprint: &str) {
    let cfg = match kind {
        LogKind::Incident => crate::load_developer_log_file_config(),
        LogKind::Feature => crate::load_feature_log_file_config(),
    };
    spawn_on_file_with_config(kind, store_root, fingerprint, &cfg);
}

pub(crate) fn spawn_on_file_with_config(
    kind: LogKind,
    store_root: &std::path::Path,
    fingerprint: &str,
    cfg: &LogFileConfig,
) {
    if !should_spawn_on_file(cfg) {
        return;
    }
    let configured = match kind {
        LogKind::Incident => default_root(),
        LogKind::Feature => fr_default_root(),
    };
    if store_root != configured.as_path() {
        return;
    }
    let Some(repo) = cfg
        .github_repo
        .as_deref()
        .and_then(|r| crate::log_config::validate_github_repo(r).ok())
        .map(str::to_string)
    else {
        return;
    };
    let fp = fingerprint.to_string();
    let root = store_root.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("turbo-log-gh-sync".into())
        .spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let gh = GhCli::new();
                let result = match kind {
                    LogKind::Incident => {
                        let store = crate::store::DeveloperLogStore::new(root);
                        sync_one_incident(&store, &gh, &repo, &fp)
                    }
                    LogKind::Feature => {
                        let store = crate::feature_request::FeatureRequestStore::new(root);
                        sync_one_feature(&store, &gh, &repo, &fp)
                    }
                };
                if let Err(e) = result {
                    tracing::warn!(
                        error = %e,
                        fingerprint = %fp,
                        "on-file github sync failed (fail-open; local JSON is source of truth)"
                    );
                }
            }));
        });
}

/// CLI `--push` / `--pull` / `--both` (default both when none given).
pub fn sync_direction(push: bool, pull: bool, both: bool) -> SyncDirection {
    if both || (push && pull) || (!push && !pull) {
        SyncDirection::Both
    } else if push {
        SyncDirection::Push
    } else {
        SyncDirection::Pull
    }
}

/// `--repo` wins; else config `github_repo`.
pub fn resolve_repo(cli_repo: Option<&str>, cfg: &LogFileConfig) -> Result<String, SyncError> {
    let raw = cli_repo
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| cfg.github_repo.clone());
    let Some(raw) = raw else {
        return Err(SyncError::RepoUnset);
    };
    crate::log_config::validate_github_repo(&raw)
        .map(|s| s.to_string())
        .map_err(|_| SyncError::InvalidRepo(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_config::GithubSyncMode;

    #[test]
    fn default_direction_is_both() {
        assert_eq!(sync_direction(false, false, false), SyncDirection::Both);
        assert_eq!(sync_direction(false, false, true), SyncDirection::Both);
        assert_eq!(sync_direction(true, true, false), SyncDirection::Both);
        assert_eq!(sync_direction(true, false, false), SyncDirection::Push);
        assert_eq!(sync_direction(false, true, false), SyncDirection::Pull);
    }

    #[test]
    fn resolve_repo_defaults_unset() {
        let cfg = LogFileConfig::default();
        assert!(matches!(
            resolve_repo(None, &cfg),
            Err(SyncError::RepoUnset)
        ));
        let got = resolve_repo(Some("o/n"), &cfg).unwrap();
        assert_eq!(got, "o/n");
    }

    #[test]
    fn on_file_skipped_for_temp_store() {
        let cfg = LogFileConfig {
            github_repo: Some("o/n".into()),
            github_sync: GithubSyncMode::OnFile,
            dir: None,
        };
        // temp path ≠ configured root → no spawn (function returns without thread)
        let tmp = std::env::temp_dir().join("turbo-log-sync-test-not-root");
        spawn_on_file_with_config(LogKind::Incident, &tmp, "fp-1", &cfg);
    }
}
