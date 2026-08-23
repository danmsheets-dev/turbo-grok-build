//! Local filesystem store for Feature Request Log entries.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::redact::{redact_text, sanitize_evidence, truncate_field};
use crate::schema::Environment;

use super::fingerprint::compute_fr_fingerprint;
use super::schema::{
    FR_SCHEMA_VERSION, FeatureRequest, FeatureRequestEvent, FeatureRequestReport,
    FeatureRequestResult, RequestPriority, RequestStatus,
};

const ROOT_DIR: &str = "feature-request-log";
const REQUESTS_DIR: &str = "requests";
const INDEX_FILE: &str = "index.json";
const EVENTS_FILE: &str = "events.jsonl";
const BUNDLES_DIR: &str = "bundles";
const CONFIG_FILE: &str = "feature-request-log.toml";

/// Env var to disable all feature-request-log writes (`0` / `false` / `off`).
pub const FR_ENABLED_ENV: &str = "GROK_FEATURE_REQUEST_LOG";
/// Env var overriding the feature-request-log root directory.
pub const FR_DIR_ENV: &str = "GROK_FEATURE_REQUEST_LOG_DIR";

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Errors from the feature-request store.
#[derive(Debug, thiserror::Error)]
pub enum FrStoreError {
    #[error("feature request log disabled via {FR_ENABLED_ENV}")]
    Disabled,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("feature request not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    Invalid(String),
}

/// Lightweight index entry for fast listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrIndexEntry {
    pub request_id: String,
    pub fingerprint: String,
    pub title: String,
    pub priority: RequestPriority,
    pub status: RequestStatus,
    pub request_class: String,
    pub occurrence_count: u32,
    pub first_seen: String,
    pub last_seen: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    entries: Vec<FrIndexEntry>,
}

/// Path to `$GROK_HOME/feature-request-log.toml`.
pub fn fr_config_file_path() -> PathBuf {
    xai_grok_config::grok_home().join(CONFIG_FILE)
}

/// Load `$GROK_HOME/feature-request-log.toml` (`dir`, `github_repo`, `github_sync`).
///
/// Missing file → default (local-only, `github_sync = "off"`).
pub fn load_feature_log_file_config() -> crate::log_config::LogFileConfig {
    crate::log_config::load_log_toml_file(&fr_config_file_path())
}

fn persist_feature_log_file_config(
    cfg: &crate::log_config::LogFileConfig,
) -> Result<(), FrStoreError> {
    let body = crate::log_config::render_log_toml(crate::log_config::LogConfigKind::Feature, cfg);
    crate::log_config::write_log_toml_file(&fr_config_file_path(), &body)?;
    Ok(())
}

/// Default on-disk location: `$GROK_HOME/feature-request-log`.
pub fn fr_builtin_default_root() -> PathBuf {
    xai_grok_config::grok_home().join(ROOT_DIR)
}

/// Resolve the feature-request-log root directory.
///
/// Precedence: process override → env `GROK_FEATURE_REQUEST_LOG_DIR` →
/// `$GROK_HOME/feature-request-log.toml` → builtin default.
pub fn fr_default_root() -> PathBuf {
    if let Ok(guard) = ROOT_OVERRIDE.lock() {
        if let Some(ref p) = *guard {
            return expand_dir(p);
        }
    }
    if let Ok(v) = std::env::var(FR_DIR_ENV) {
        let v = v.trim();
        if !v.is_empty() {
            return expand_dir(Path::new(v));
        }
    }
    if let Some(p) = read_config_dir() {
        return expand_dir(&p);
    }
    fr_builtin_default_root()
}

/// Set a process-local root override (tests / one-shot CLI).
pub fn fr_set_root_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = ROOT_OVERRIDE.lock() {
        *guard = path.map(|p| expand_dir(&p));
    }
}

/// Persist `dir` to config and set process override.
pub fn fr_set_configured_dir(path: &Path) -> Result<PathBuf, FrStoreError> {
    let expanded = expand_dir(path);
    if expanded.as_os_str().is_empty() {
        return Err(FrStoreError::Invalid("directory path is empty".into()));
    }
    let force = std::env::var_os("GROK_FEATURE_REQUEST_LOG_FORCE_DIR").is_some_and(|v| {
        matches!(
            v.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    });
    let target = if !force && path_looks_like_app_source_tree(&expanded) {
        let nested = expanded.join(ROOT_DIR);
        tracing::warn!(
            requested = %expanded.display(),
            using = %nested.display(),
            "feature-request-log set-dir: path looks like a source tree; using nested feature-request-log/"
        );
        nested
    } else {
        expanded
    };
    fs::create_dir_all(&target)?;
    let _ = FeatureRequestStore::new(target.clone()).ensure_layout();
    let mut cfg = load_feature_log_file_config();
    cfg.dir = Some(target.clone());
    persist_feature_log_file_config(&cfg)?;
    fr_set_root_override(Some(target.clone()));
    Ok(target)
}

/// Clear the persisted dir. Preserves `github_repo` / `github_sync`.
pub fn fr_clear_configured_dir() -> Result<(), FrStoreError> {
    let mut cfg = load_feature_log_file_config();
    cfg.dir = None;
    if cfg.github_repo.is_none() && cfg.github_sync.is_off() {
        let path = fr_config_file_path();
        if path.is_file() {
            fs::remove_file(&path)?;
        }
    } else {
        persist_feature_log_file_config(&cfg)?;
    }
    fr_set_root_override(None);
    Ok(())
}

fn path_looks_like_app_source_tree(path: &Path) -> bool {
    let has_git = path.join(".git").exists();
    let has_cargo = path.join("Cargo.toml").is_file();
    let has_crates = path.join("crates").is_dir();
    let has_package = path.join("package.json").is_file();
    (has_git || has_cargo || has_package) && (has_crates || has_cargo || has_package)
}

fn read_config_dir() -> Option<PathBuf> {
    load_feature_log_file_config().dir
}

fn expand_dir(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        #[allow(deprecated)]
        let home = std::env::home_dir()
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        if s == "~" {
            return home;
        }
        return home.join(&s[2..]);
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Whether the feature request log is enabled (default true).
pub fn fr_is_enabled() -> bool {
    match std::env::var(FR_ENABLED_ENV) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !matches!(v.as_str(), "0" | "false" | "off" | "no" | "disabled")
        }
        Err(_) => true,
    }
}

/// Human-readable summary of how the root was resolved.
pub fn fr_root_resolution_note() -> String {
    if let Ok(guard) = ROOT_OVERRIDE.lock() {
        if guard.is_some() {
            return "process override / set-dir this session".into();
        }
    }
    if std::env::var(FR_DIR_ENV)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return format!("env {FR_DIR_ENV}");
    }
    if fr_config_file_path().is_file() && read_config_dir().is_some() {
        return format!("config {}", fr_config_file_path().display());
    }
    "default ($GROK_HOME/feature-request-log)".into()
}

/// Handle for a feature-request store rooted at `root`.
#[derive(Debug, Clone)]
pub struct FeatureRequestStore {
    root: PathBuf,
}

impl Default for FeatureRequestStore {
    fn default() -> Self {
        Self::new(fr_default_root())
    }
}

impl FeatureRequestStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn requests_dir(&self) -> PathBuf {
        self.root.join(REQUESTS_DIR)
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    pub fn events_path(&self) -> PathBuf {
        self.root.join(EVENTS_FILE)
    }

    pub fn bundles_dir(&self) -> PathBuf {
        self.root.join(BUNDLES_DIR)
    }

    pub fn ensure_layout(&self) -> Result<(), FrStoreError> {
        fs::create_dir_all(self.requests_dir())?;
        fs::create_dir_all(self.bundles_dir())?;
        let index = self.index_path();
        if !index.is_file() {
            let empty = IndexFile::default();
            let pretty = serde_json::to_string_pretty(&empty)
                .map_err(|e| FrStoreError::Invalid(e.to_string()))?;
            fs::write(index, pretty)?;
        }
        Ok(())
    }

    /// Report a feature request; merges by fingerprint when one already exists.
    pub fn report(
        &self,
        request: FeatureRequestReport,
    ) -> Result<FeatureRequestResult, FrStoreError> {
        if !fr_is_enabled() {
            return Err(FrStoreError::Disabled);
        }
        if request.title.trim().is_empty() {
            return Err(FrStoreError::Invalid("title is required".into()));
        }
        if request.summary.trim().is_empty() {
            return Err(FrStoreError::Invalid("summary is required".into()));
        }
        let result = self.report_locked(request)?;
        crate::github_sync::spawn_on_file_if_enabled(
            crate::github_sync::LogKind::Feature,
            self.root(),
            &result.fingerprint,
        );
        Ok(result)
    }

    fn report_locked(
        &self,
        request: FeatureRequestReport,
    ) -> Result<FeatureRequestResult, FrStoreError> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        self.ensure_layout()?;
        let mut req = sanitize_report(request);
        let fingerprint = compute_fr_fingerprint(&req);
        req.fingerprint = Some(fingerprint.clone());

        let priority = req
            .priority
            .unwrap_or_else(|| req.request_class.default_priority());

        fill_environment_defaults(&mut req.environment);

        let mut index = self.load_index()?;
        let now = Utc::now();

        if let Some(existing) = index
            .entries
            .iter()
            .find(|e| e.fingerprint == fingerprint)
            .cloned()
        {
            let mut fr = self.read_at(&self.root.join(&existing.path))?;
            fr.occurrence_count = fr.occurrence_count.saturating_add(1);
            fr.last_seen = now;
            if priority.rank() < fr.priority.rank() {
                fr.priority = priority;
            }
            merge_strings(&mut fr.component, &req.component);
            merge_strings(&mut fr.tags, &req.tags);
            merge_strings(
                &mut fr.evidence.related_events,
                &req.evidence.related_events,
            );
            if fr.use_case.as_ref().map(|s| s.len()).unwrap_or(0)
                < req.use_case.as_ref().map(|s| s.len()).unwrap_or(0)
            {
                fr.use_case = req.use_case.clone();
            }
            if fr.current_workaround.is_none() {
                fr.current_workaround = req.current_workaround.clone();
            }
            if fr.proposed_behavior.is_none() {
                fr.proposed_behavior = req.proposed_behavior.clone();
            }
            if fr.acceptance_criteria.is_empty() && !req.acceptance_criteria.is_empty() {
                fr.acceptance_criteria = req.acceptance_criteria.clone();
            }
            if fr.summary.len() < req.summary.len() {
                fr.summary = req.summary.clone();
            }
            if req.environment.session_id.is_some() {
                fr.environment.session_id = req.environment.session_id.clone();
            }
            if req.environment.subagent_id.is_some() {
                fr.environment.subagent_id = req.environment.subagent_id.clone();
            }
            if req.environment.model.is_some() {
                fr.environment.model = req.environment.model.clone();
            }
            if req.environment.provider.is_some() {
                fr.environment.provider = req.environment.provider.clone();
            }

            let fr = sanitize_request_doc(fr);
            let rel_path = existing.path.clone();
            self.write_at(&self.root.join(&rel_path), &fr)?;
            self.upsert_index(&mut index, &fr, &rel_path)?;
            self.append_event(FeatureRequestEvent {
                ts: now,
                request_id: fr.request_id.clone(),
                fingerprint: fingerprint.clone(),
                action: "increment".into(),
                detail: Some(format!("occurrence_count={}", fr.occurrence_count)),
            })?;

            return Ok(FeatureRequestResult {
                request_id: fr.request_id,
                fingerprint,
                is_new: false,
                occurrence_count: fr.occurrence_count,
                path: self.root.join(&rel_path).display().to_string(),
                priority: fr.priority,
                request_class: fr.request_class,
                title: fr.title,
            });
        }

        let request_id = format!("fr_{}", uuid::Uuid::now_v7().simple());
        let day = now.format("%Y-%m-%d").to_string();
        let day_dir = self.requests_dir().join(&day);
        fs::create_dir_all(&day_dir)?;
        let file_name = format!("{request_id}.json");
        let abs_path = day_dir.join(&file_name);
        let rel_path = format!("{REQUESTS_DIR}/{day}/{file_name}");

        let fr = FeatureRequest {
            schema_version: FR_SCHEMA_VERSION,
            request_id: request_id.clone(),
            fingerprint: fingerprint.clone(),
            title: req.title.clone(),
            summary: req.summary.clone(),
            request_class: req.request_class,
            priority,
            status: RequestStatus::Open,
            component: req.component.clone(),
            occurrence_count: 1,
            first_seen: now,
            last_seen: now,
            use_case: req.use_case.clone(),
            current_workaround: req.current_workaround.clone(),
            proposed_behavior: req.proposed_behavior.clone(),
            acceptance_criteria: req.acceptance_criteria.clone(),
            environment: req.environment.clone(),
            evidence: req.evidence.clone(),
            source: req.source.clone(),
            tags: req.tags.clone(),
            ship_sha: None,
            ship_note: None,
        };
        let fr = sanitize_request_doc(fr);
        self.write_at(&abs_path, &fr)?;
        self.upsert_index(&mut index, &fr, &rel_path)?;
        self.append_event(FeatureRequestEvent {
            ts: now,
            request_id: request_id.clone(),
            fingerprint: fingerprint.clone(),
            action: "create".into(),
            detail: None,
        })?;

        Ok(FeatureRequestResult {
            request_id,
            fingerprint,
            is_new: true,
            occurrence_count: 1,
            path: abs_path.display().to_string(),
            priority: fr.priority,
            request_class: fr.request_class,
            title: fr.title,
        })
    }

    pub fn list(&self, filter: &FrListFilter) -> Result<Vec<FrIndexEntry>, FrStoreError> {
        let index = self.load_index()?;
        let mut entries: Vec<FrIndexEntry> = index
            .entries
            .into_iter()
            .filter(|e| filter.matches(e))
            .collect();
        entries.sort_by(|a, b| {
            a.priority
                .rank()
                .cmp(&b.priority.rank())
                .then_with(|| b.last_seen.cmp(&a.last_seen))
        });
        if let Some(limit) = filter.limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    pub fn get(&self, id_or_fingerprint: &str) -> Result<FeatureRequest, FrStoreError> {
        let key = id_or_fingerprint.trim();
        if key.is_empty() {
            return Err(FrStoreError::Invalid("empty id".into()));
        }
        let index = self.load_index()?;
        let entry = index
            .entries
            .iter()
            .find(|e| e.request_id == key || e.fingerprint == key)
            .ok_or_else(|| FrStoreError::NotFound(key.to_string()))?;
        self.read_at(&self.root.join(&entry.path))
    }

    pub fn set_status(
        &self,
        id_or_fingerprint: &str,
        status: RequestStatus,
    ) -> Result<FeatureRequest, FrStoreError> {
        self.set_status_with(id_or_fingerprint, status, None, None)
    }

    pub fn set_status_with(
        &self,
        id_or_fingerprint: &str,
        status: RequestStatus,
        sha: Option<&str>,
        note: Option<&str>,
    ) -> Result<FeatureRequest, FrStoreError> {
        if !fr_is_enabled() {
            return Err(FrStoreError::Disabled);
        }
        let fr = self.set_status_locked(id_or_fingerprint, status, sha, note)?;
        crate::github_sync::spawn_on_file_if_enabled(
            crate::github_sync::LogKind::Feature,
            self.root(),
            &fr.fingerprint,
        );
        Ok(fr)
    }

    fn set_status_locked(
        &self,
        id_or_fingerprint: &str,
        status: RequestStatus,
        sha: Option<&str>,
        note: Option<&str>,
    ) -> Result<FeatureRequest, FrStoreError> {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut index = self.load_index()?;
        let entry = index
            .entries
            .iter()
            .find(|e| e.request_id == id_or_fingerprint || e.fingerprint == id_or_fingerprint)
            .cloned()
            .ok_or_else(|| FrStoreError::NotFound(id_or_fingerprint.to_string()))?;
        let mut fr = self.read_at(&self.root.join(&entry.path))?;
        fr.status = status;
        fr.last_seen = Utc::now();
        if let Some(sha) = sha.map(str::trim).filter(|s| !s.is_empty()) {
            fr.ship_sha = Some(sha.to_owned());
        }
        if let Some(note) = note.map(str::trim).filter(|s| !s.is_empty()) {
            fr.ship_note = Some(note.to_owned());
        }
        fr = sanitize_request_doc(fr);
        self.write_at(&self.root.join(&entry.path), &fr)?;
        self.upsert_index(&mut index, &fr, &entry.path)?;
        let detail = match (
            sha.map(str::trim).filter(|s| !s.is_empty()),
            status.as_str(),
        ) {
            (Some(sha), st) => Some(format!("{st} sha={sha}")),
            (None, st) => Some(st.to_string()),
        };
        self.append_event(FeatureRequestEvent {
            ts: Utc::now(),
            request_id: fr.request_id.clone(),
            fingerprint: fr.fingerprint.clone(),
            action: "status".into(),
            detail,
        })?;
        Ok(fr)
    }

    fn load_index(&self) -> Result<IndexFile, FrStoreError> {
        let path = self.index_path();
        if !path.is_file() {
            return Ok(IndexFile::default());
        }
        let raw = fs::read_to_string(&path)?;
        if raw.trim().is_empty() {
            return Ok(IndexFile::default());
        }
        Ok(serde_json::from_str(&raw)?)
    }

    fn write_index(&self, index: &IndexFile) -> Result<(), FrStoreError> {
        self.ensure_layout()?;
        let path = self.index_path();
        let tmp = path.with_extension("json.tmp");
        let pretty = serde_json::to_string_pretty(index)?;
        fs::write(&tmp, pretty)?;
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn upsert_index(
        &self,
        index: &mut IndexFile,
        fr: &FeatureRequest,
        rel_path: &str,
    ) -> Result<(), FrStoreError> {
        let entry = FrIndexEntry {
            request_id: fr.request_id.clone(),
            fingerprint: fr.fingerprint.clone(),
            title: fr.title.clone(),
            priority: fr.priority,
            status: fr.status,
            request_class: fr.request_class.as_str().to_string(),
            occurrence_count: fr.occurrence_count,
            first_seen: fr.first_seen.to_rfc3339(),
            last_seen: fr.last_seen.to_rfc3339(),
            path: rel_path.to_string(),
            component: fr.component.clone(),
        };
        if let Some(slot) = index
            .entries
            .iter_mut()
            .find(|e| e.fingerprint == entry.fingerprint)
        {
            *slot = entry;
        } else {
            index.entries.push(entry);
        }
        self.write_index(index)
    }

    fn write_at(&self, path: &Path, fr: &FeatureRequest) -> Result<(), FrStoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        // Defense-in-depth: field-level sanitization already ran via `sanitize_request_doc`,
        // but walk the serialized JSON once more so any missed free-form value is
        // scrubbed before it lands on disk.
        let mut value = serde_json::to_value(fr)?;
        xai_grok_secrets::redact_json_string_values(&mut value);
        let pretty = serde_json::to_string_pretty(&value)?;
        fs::write(&tmp, pretty)?;
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_at(&self, path: &Path) -> Result<FeatureRequest, FrStoreError> {
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn append_event(&self, event: FeatureRequestEvent) -> Result<(), FrStoreError> {
        self.ensure_layout()?;
        let path = self.events_path();
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let line = serde_json::to_string(&event)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Best-effort report (never panics; logs and returns None on failure).
    pub fn report_best_effort(request: FeatureRequestReport) -> Option<FeatureRequestResult> {
        if !fr_is_enabled() {
            return None;
        }
        match FeatureRequestStore::default().report(request) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(error = %e, "feature_request_log report failed");
                None
            }
        }
    }
}

/// Filters for [`FeatureRequestStore::list`].
#[derive(Debug, Clone, Default)]
pub struct FrListFilter {
    pub priority: Option<Vec<RequestPriority>>,
    pub status: Option<Vec<RequestStatus>>,
    pub request_class: Option<String>,
    pub component: Option<String>,
    pub limit: Option<usize>,
    /// When true, include shipped / declined (default: openish only).
    pub include_closed: bool,
}

impl FrListFilter {
    fn matches(&self, e: &FrIndexEntry) -> bool {
        if let Some(ref pris) = self.priority
            && !pris.contains(&e.priority)
        {
            return false;
        }
        if let Some(ref statuses) = self.status {
            if !statuses.contains(&e.status) {
                return false;
            }
        } else if !self.include_closed && !e.status.is_openish() {
            return false;
        }
        if let Some(ref class) = self.request_class
            && !e.request_class.eq_ignore_ascii_case(class)
        {
            return false;
        }
        if let Some(ref comp) = self.component
            && !e.component.iter().any(|c| c.eq_ignore_ascii_case(comp))
        {
            return false;
        }
        true
    }
}

fn merge_strings(dst: &mut Vec<String>, src: &[String]) {
    for s in src {
        if !dst.iter().any(|d| d == s) {
            dst.push(s.clone());
        }
    }
}

fn fill_environment_defaults(env: &mut Environment) {
    if env.product_version.is_none() {
        env.product_version = Some(xai_grok_version::installed());
    }
    if env.os.is_none() {
        env.os = Some(std::env::consts::OS.to_string());
    }
    if env.arch.is_none() {
        env.arch = Some(std::env::consts::ARCH.to_string());
    }
}

fn sanitize_report(mut req: FeatureRequestReport) -> FeatureRequestReport {
    req.title = truncate_field(&redact_text(&req.title), 200);
    req.summary = truncate_field(&redact_text(&req.summary), 4_000);
    if let Some(ref mut u) = req.use_case {
        *u = truncate_field(&redact_text(u), 4_000);
    }
    if let Some(ref mut w) = req.current_workaround {
        *w = truncate_field(&redact_text(w), 4_000);
    }
    if let Some(ref mut p) = req.proposed_behavior {
        *p = truncate_field(&redact_text(p), 4_000);
    }
    req.acceptance_criteria = req
        .acceptance_criteria
        .into_iter()
        .map(|s| truncate_field(&redact_text(&s), 500))
        .filter(|s| !s.is_empty())
        .take(20)
        .collect();
    req.component = req
        .component
        .into_iter()
        .map(|s| truncate_field(&redact_text(&s), 64))
        .filter(|s| !s.is_empty())
        .take(16)
        .collect();
    req.tags = req
        .tags
        .into_iter()
        .map(|s| truncate_field(&redact_text(&s), 64))
        .filter(|s| !s.is_empty())
        .take(16)
        .collect();
    sanitize_env_fields(&mut req.environment);
    if let Some(m) = req.source.reporter_model.as_mut() {
        *m = truncate_field(&redact_text(m), 128);
    }
    req.evidence = sanitize_evidence(req.evidence);
    req
}

/// Re-sanitize a stored feature request (export / GitHub body).
pub fn sanitize_feature_request(fr: FeatureRequest) -> FeatureRequest {
    sanitize_request_doc(fr)
}

pub(super) fn sanitize_request_doc(mut fr: FeatureRequest) -> FeatureRequest {
    fr.title = truncate_field(&redact_text(&fr.title), 200);
    fr.summary = truncate_field(&redact_text(&fr.summary), 4_000);
    if let Some(ref mut u) = fr.use_case {
        *u = truncate_field(&redact_text(u), 4_000);
    }
    if let Some(ref mut w) = fr.current_workaround {
        *w = truncate_field(&redact_text(w), 4_000);
    }
    if let Some(ref mut p) = fr.proposed_behavior {
        *p = truncate_field(&redact_text(p), 4_000);
    }
    sanitize_env_fields(&mut fr.environment);
    fr.evidence = sanitize_evidence(fr.evidence);
    if let Some(ref mut note) = fr.ship_note {
        *note = truncate_field(&redact_text(note), 4_000);
    }
    fr
}

fn sanitize_env_fields(env: &mut Environment) {
    if let Some(m) = env.model.as_mut() {
        *m = truncate_field(&redact_text(m), 128);
    }
    if let Some(p) = env.provider.as_mut() {
        *p = truncate_field(&redact_text(p), 128);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature_request::schema::{RequestClass, agent_source};

    #[test]
    fn report_dedups_by_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeatureRequestStore::new(dir.path().to_path_buf());
        let req = FeatureRequestReport {
            title: "Hull merge tool".into(),
            summary: "Need automatic hull.json merge on multi-agent land".into(),
            request_class: RequestClass::Subagent,
            component: vec!["land".into()],
            use_case: Some("Pirates multi-blender land".into()),
            source: agent_source("feature_request_log", None),
            ..Default::default()
        };
        let a = store.report(req.clone()).unwrap();
        assert!(a.is_new);
        assert_eq!(a.occurrence_count, 1);
        let b = store.report(req).unwrap();
        assert!(!b.is_new);
        assert_eq!(b.occurrence_count, 2);
        assert_eq!(a.request_id, b.request_id);
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn list_filters_open_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeatureRequestStore::new(dir.path().to_path_buf());
        let r = store
            .report(FeatureRequestReport {
                title: "A".into(),
                summary: "s".into(),
                request_class: RequestClass::ToolSurface,
                ..Default::default()
            })
            .unwrap();
        store
            .set_status_with(
                &r.request_id,
                RequestStatus::Shipped,
                Some("abc1234"),
                Some("shipped in rc6"),
            )
            .unwrap();
        let shipped = store.get(&r.request_id).unwrap();
        assert_eq!(shipped.ship_sha.as_deref(), Some("abc1234"));
        assert_eq!(shipped.ship_note.as_deref(), Some("shipped in rc6"));
        let open = store.list(&FrListFilter::default()).unwrap();
        assert!(open.is_empty());
        let all = store
            .list(&FrListFilter {
                include_closed: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn rejects_empty_title() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeatureRequestStore::new(dir.path().to_path_buf());
        let err = store
            .report(FeatureRequestReport {
                title: "  ".into(),
                summary: "s".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(err, FrStoreError::Invalid(_)));
    }

    /// Fixtures join fragments at runtime so secret scanners don't flag the source.
    fn fixture(parts: &[&str]) -> String {
        parts.concat()
    }

    /// Regression: embedded fake tokens in FR fields must be
    /// redacted to `[REDACTED_SECRET]` in the JSON persisted to disk.
    #[test]
    fn stored_feature_request_redacts_embedded_fake_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeatureRequestStore::new(dir.path().to_path_buf());
        let ghp = fixture(&["ghp_f", "akefakefakefakefakefakefake"]);
        let aws = fixture(&["AKIA", "ABCDEFGHIJKLMNOP"]);
        let bearer = fixture(&["Authorization: Bearer sk-CANARY", "abcdefghij1234567890"]);
        let req = FeatureRequestReport {
            title: fixture(&["FR with ", &ghp, " token"]),
            summary: fixture(&["detail ", &aws, " and ", &bearer]),
            request_class: RequestClass::ToolSurface,
            use_case: Some(fixture(&["needs ", &ghp])),
            ..Default::default()
        };
        let r = store.report(req).unwrap();
        let raw = std::fs::read_to_string(&r.path).unwrap_or_default();
        assert!(
            !raw.contains("ghp_f"),
            "GitHub PAT prefix leaked to disk: {raw}"
        );
        assert!(
            !raw.contains("AKIA"),
            "AWS access key leaked to disk: {raw}"
        );
        assert!(
            !raw.contains("CANARY"),
            "bearer token leaked to disk: {raw}"
        );
        assert!(
            raw.contains("[REDACTED_SECRET]"),
            "expected [REDACTED_SECRET] in stored JSON: {raw}"
        );
    }
}
