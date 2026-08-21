//! Local filesystem store for Auto Developer Log incidents.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::fingerprint::compute_fingerprint;
use crate::redact::{sanitize_incident, sanitize_request};
use crate::schema::{
    Environment, Incident, IncidentStatus, LogEvent, ReportRequest, ReportResult, SCHEMA_VERSION,
    Severity,
};

const ROOT_DIR: &str = "developer-log";
const INCIDENTS_DIR: &str = "incidents";
const INDEX_FILE: &str = "index.json";
const EVENTS_FILE: &str = "events.jsonl";
const BUNDLES_DIR: &str = "bundles";
/// Sidecar under `$GROK_HOME` that stores the user-chosen log root.
const CONFIG_FILE: &str = "developer-log.toml";

/// Env var to disable all developer-log writes (`0` / `false` / `off`).
pub const ENABLED_ENV: &str = "GROK_DEVELOPER_LOG";
/// Env var overriding the developer-log root directory (absolute path preferred).
pub const DIR_ENV: &str = "GROK_DEVELOPER_LOG_DIR";

static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// Process-local override set by `set_root_override` (CLI / tests).
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Errors from the developer-log store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("developer log disabled via {ENABLED_ENV}")]
    Disabled,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("incident not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    Invalid(String),
}

/// Lightweight index entry for fast listing without reading every incident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub incident_id: String,
    pub fingerprint: String,
    pub title: String,
    pub severity: Severity,
    pub status: IncidentStatus,
    pub error_class: String,
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
    entries: Vec<IndexEntry>,
}

/// Path to `$GROK_HOME/developer-log.toml` (user-configured log root).
pub fn config_file_path() -> PathBuf {
    xai_grok_config::grok_home().join(CONFIG_FILE)
}

/// Default on-disk location when nothing is configured: `$GROK_HOME/developer-log`.
pub fn builtin_default_root() -> PathBuf {
    xai_grok_config::grok_home().join(ROOT_DIR)
}

/// Resolve the developer-log root directory.
///
/// Precedence (highest first):
/// 1. Process override via [`set_root_override`]
/// 2. Env [`DIR_ENV`] (`GROK_DEVELOPER_LOG_DIR`)
/// 3. `$GROK_HOME/developer-log.toml` → `dir = "..."`
/// 4. Builtin [`builtin_default_root`] (`$GROK_HOME/developer-log`)
pub fn default_root() -> PathBuf {
    if let Ok(guard) = ROOT_OVERRIDE.lock() {
        if let Some(ref p) = *guard {
            return expand_dir(p);
        }
    }
    if let Ok(v) = std::env::var(DIR_ENV) {
        let v = v.trim();
        if !v.is_empty() {
            return expand_dir(Path::new(v));
        }
    }
    if let Some(p) = read_config_dir() {
        return expand_dir(&p);
    }
    builtin_default_root()
}

/// Set a process-local root override (tests / one-shot CLI). Does not persist.
pub fn set_root_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = ROOT_OVERRIDE.lock() {
        *guard = path.map(|p| expand_dir(&p));
    }
}

/// Persist `dir` to `$GROK_HOME/developer-log.toml` and set the process override.
///
/// If `path` looks like an application source tree (git root + Cargo/crates),
/// incidents are stored under `{path}/developer-log` instead so logs are not
/// co-mingled with code. Set env `GROK_DEVELOPER_LOG_FORCE_DIR=1` to force the
/// exact path.
pub fn set_configured_dir(path: &Path) -> Result<PathBuf, StoreError> {
    let expanded = expand_dir(path);
    if expanded.as_os_str().is_empty() {
        return Err(StoreError::Invalid("directory path is empty".into()));
    }
    let force = std::env::var_os("GROK_DEVELOPER_LOG_FORCE_DIR").is_some_and(|v| {
        matches!(
            v.to_string_lossy().trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    });
    let target = if !force && path_looks_like_app_source_tree(&expanded) {
        let nested = expanded.join("developer-log");
        tracing::warn!(
            requested = %expanded.display(),
            using = %nested.display(),
            "developer-log set-dir: path looks like a source tree; using nested developer-log/"
        );
        nested
    } else {
        expanded
    };
    // Reject relative paths that would escape in surprising ways after expand.
    fs::create_dir_all(&target).map_err(StoreError::Io)?;
    // Ensure real store layout exists so empty stores are not "healthy" ghosts.
    let _ = DeveloperLogStore::new(target.clone()).ensure_layout();
    let cfg_path = config_file_path();
    if let Some(parent) = cfg_path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Minimal TOML — no extra dep. Quote path for spaces/backslashes.
    let escaped = target.display().to_string().replace('\\', "\\\\");
    let body = format!(
        "# Turbo Auto Developer Log — root directory for product incidents\n\
         # Override with env {DIR_ENV}=...\n\
         # Managed by `turbo issues set-dir`\n\
         dir = \"{escaped}\"\n"
    );
    let tmp = cfg_path.with_extension("toml.tmp");
    fs::write(&tmp, body)?;
    // Windows cannot rename over an existing file — remove dest first.
    if cfg_path.exists() {
        let _ = fs::remove_file(&cfg_path);
    }
    fs::rename(&tmp, &cfg_path)?;
    set_root_override(Some(target.clone()));
    Ok(target)
}

/// True when `path` looks like an app repo root (would be a bad log root).
fn path_looks_like_app_source_tree(path: &Path) -> bool {
    let has_git = path.join(".git").exists();
    let has_cargo = path.join("Cargo.toml").is_file();
    let has_crates = path.join("crates").is_dir();
    let has_package = path.join("package.json").is_file();
    (has_git || has_cargo || has_package) && (has_crates || has_cargo || has_package)
}

/// Clear the persisted dir config (revert to builtin default). Also clears override.
pub fn clear_configured_dir() -> Result<(), StoreError> {
    let cfg = config_file_path();
    if cfg.is_file() {
        fs::remove_file(&cfg)?;
    }
    set_root_override(None);
    Ok(())
}

fn read_config_dir() -> Option<PathBuf> {
    let cfg = config_file_path();
    let raw = fs::read_to_string(cfg).ok()?;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // dir = "..." or dir = '...'
        let rest = line.strip_prefix("dir")?;
        let rest = rest.trim().strip_prefix('=')?.trim();
        let unquoted = rest
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(rest);
        let unescaped = unquoted.replace("\\\\", "\\");
        if !unescaped.is_empty() {
            return Some(PathBuf::from(unescaped));
        }
    }
    None
}

fn expand_dir(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    // Expand leading ~ to home.
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
    // Relative: resolve against current_dir for stability in CLI.
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Whether the developer log is enabled (default true).
pub fn is_enabled() -> bool {
    match std::env::var(ENABLED_ENV) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !matches!(v.as_str(), "0" | "false" | "off" | "no" | "disabled")
        }
        Err(_) => true,
    }
}

/// Human-readable summary of how the root was resolved (for CLI / boot card).
pub fn root_resolution_note() -> String {
    if let Ok(guard) = ROOT_OVERRIDE.lock() {
        if guard.is_some() {
            return "process override / set-dir this session".into();
        }
    }
    if std::env::var(DIR_ENV)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return format!("env {DIR_ENV}");
    }
    if config_file_path().is_file() && read_config_dir().is_some() {
        return format!("config {}", config_file_path().display());
    }
    "default ($GROK_HOME/developer-log)".into()
}

/// Handle for a developer-log store rooted at `root`.
#[derive(Debug, Clone)]
pub struct DeveloperLogStore {
    root: PathBuf,
}

impl Default for DeveloperLogStore {
    fn default() -> Self {
        Self::new(default_root())
    }
}

impl DeveloperLogStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn incidents_dir(&self) -> PathBuf {
        self.root.join(INCIDENTS_DIR)
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

    pub fn ensure_layout(&self) -> Result<(), StoreError> {
        fs::create_dir_all(self.incidents_dir())?;
        fs::create_dir_all(self.bundles_dir())?;
        // Touch an empty index if missing so operators see a real store.
        let index = self.index_path();
        if !index.is_file() {
            let empty = IndexFile::default();
            let pretty = serde_json::to_string_pretty(&empty)
                .map_err(|e| StoreError::Invalid(e.to_string()))?;
            fs::write(index, pretty)?;
        }
        Ok(())
    }

    /// Report a product issue; merges by fingerprint when one already exists.
    pub fn report(&self, request: ReportRequest) -> Result<ReportResult, StoreError> {
        if !is_enabled() {
            return Err(StoreError::Disabled);
        }
        if request.title.trim().is_empty() {
            return Err(StoreError::Invalid("title is required".into()));
        }
        if request.summary.trim().is_empty() {
            return Err(StoreError::Invalid("summary is required".into()));
        }

        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        self.ensure_layout()?;
        let mut req = sanitize_request(request);
        let fingerprint = compute_fingerprint(&req);
        req.fingerprint = Some(fingerprint.clone());

        let severity = req
            .severity
            .unwrap_or_else(|| req.error_class.default_severity());
        let kind = req.kind.unwrap_or_else(|| req.error_class.default_kind());

        // Ensure environment has version/os defaults.
        fill_environment_defaults(&mut req.environment);

        let mut index = self.load_index()?;
        let now = Utc::now();

        if let Some(existing) = index
            .entries
            .iter()
            .find(|e| e.fingerprint == fingerprint)
            .cloned()
        {
            let mut incident = self.read_incident_at(&self.root.join(&existing.path))?;
            incident.occurrence_count = incident.occurrence_count.saturating_add(1);
            incident.last_seen = now;
            // Keep highest severity (lowest rank).
            if severity.rank() < incident.severity.rank() {
                incident.severity = severity;
            }
            // Merge evidence / tags / components (dedup).
            merge_strings(&mut incident.component, &req.component);
            merge_strings(&mut incident.tags, &req.tags);
            merge_strings(
                &mut incident.evidence.related_events,
                &req.evidence.related_events,
            );
            merge_strings(
                &mut incident.evidence.attachments,
                &req.evidence.attachments,
            );
            if incident.evidence.meta_path.is_none() {
                incident.evidence.meta_path = req.evidence.meta_path.clone();
            }
            if incident.evidence.snapshot_ref.is_none() {
                incident.evidence.snapshot_ref = req.evidence.snapshot_ref.clone();
            }
            if incident.evidence.patch_path.is_none() {
                incident.evidence.patch_path = req.evidence.patch_path.clone();
            }
            if incident.evidence.session_ref.is_none() {
                incident.evidence.session_ref = req.evidence.session_ref.clone();
            }
            // Prefer fresher session/subagent ids.
            if req.environment.session_id.is_some() {
                incident.environment.session_id = req.environment.session_id.clone();
            }
            if req.environment.subagent_id.is_some() {
                incident.environment.subagent_id = req.environment.subagent_id.clone();
            }
            if req.environment.model.is_some() {
                incident.environment.model = req.environment.model.clone();
            }
            if req.environment.provider.is_some() {
                incident.environment.provider = req.environment.provider.clone();
            }
            // Keep newest summary if previous was empty-ish.
            if incident.summary.len() < req.summary.len() {
                incident.summary = req.summary.clone();
            }
            if incident.suggested_fix.is_none() {
                incident.suggested_fix = req.suggested_fix.clone();
            }
            if !req.repro.steps.is_empty() && incident.repro.steps.is_empty() {
                incident.repro = req.repro.clone();
            }

            let incident = sanitize_incident(incident);
            let rel_path = existing.path.clone();
            self.write_incident_at(&self.root.join(&rel_path), &incident)?;
            self.upsert_index_entry(&mut index, &incident, &rel_path)?;
            self.append_event(LogEvent {
                ts: now,
                incident_id: incident.incident_id.clone(),
                fingerprint: fingerprint.clone(),
                action: "increment".into(),
                detail: Some(format!("occurrence_count={}", incident.occurrence_count)),
            })?;

            return Ok(ReportResult {
                incident_id: incident.incident_id,
                fingerprint,
                is_new: false,
                occurrence_count: incident.occurrence_count,
                path: self.root.join(&rel_path).display().to_string(),
                severity: incident.severity,
                error_class: incident.error_class,
                title: incident.title,
            });
        }

        // New incident.
        let incident_id = format!("inc_{}", uuid::Uuid::now_v7().simple());
        let day = now.format("%Y-%m-%d").to_string();
        let day_dir = self.incidents_dir().join(&day);
        fs::create_dir_all(&day_dir)?;
        let file_name = format!("{incident_id}.json");
        let abs_path = day_dir.join(&file_name);
        let rel_path = format!("{INCIDENTS_DIR}/{day}/{file_name}");

        let incident = Incident {
            schema_version: SCHEMA_VERSION,
            incident_id: incident_id.clone(),
            fingerprint: fingerprint.clone(),
            kind,
            title: req.title.clone(),
            summary: req.summary.clone(),
            severity,
            status: IncidentStatus::Open,
            component: req.component.clone(),
            error_class: req.error_class,
            occurrence_count: 1,
            first_seen: now,
            last_seen: now,
            environment: req.environment.clone(),
            repro: req.repro.clone(),
            evidence: req.evidence.clone(),
            suggested_fix: req.suggested_fix.clone(),
            source: req.source.clone(),
            tags: req.tags.clone(),
        };
        let incident = sanitize_incident(incident);
        self.write_incident_at(&abs_path, &incident)?;
        self.upsert_index_entry(&mut index, &incident, &rel_path)?;
        self.append_event(LogEvent {
            ts: now,
            incident_id: incident_id.clone(),
            fingerprint: fingerprint.clone(),
            action: "create".into(),
            detail: None,
        })?;

        Ok(ReportResult {
            incident_id,
            fingerprint,
            is_new: true,
            occurrence_count: 1,
            path: abs_path.display().to_string(),
            severity: incident.severity,
            error_class: incident.error_class,
            title: incident.title,
        })
    }

    /// List incidents, optionally filtered.
    pub fn list(&self, filter: &ListFilter) -> Result<Vec<IndexEntry>, StoreError> {
        let index = self.load_index()?;
        let mut entries: Vec<IndexEntry> = index
            .entries
            .into_iter()
            .filter(|e| filter.matches(e))
            .collect();
        entries.sort_by(|a, b| {
            // severity rank, then last_seen desc
            a.severity
                .rank()
                .cmp(&b.severity.rank())
                .then_with(|| b.last_seen.cmp(&a.last_seen))
        });
        if let Some(limit) = filter.limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    /// Load a full incident by id or fingerprint.
    pub fn get(&self, id_or_fingerprint: &str) -> Result<Incident, StoreError> {
        let key = id_or_fingerprint.trim();
        if key.is_empty() {
            return Err(StoreError::Invalid("empty id".into()));
        }
        let index = self.load_index()?;
        let entry = index
            .entries
            .iter()
            .find(|e| e.incident_id == key || e.fingerprint == key)
            .ok_or_else(|| StoreError::NotFound(key.to_string()))?;
        self.read_incident_at(&self.root.join(&entry.path))
    }

    /// Update status of an incident.
    pub fn set_status(
        &self,
        id_or_fingerprint: &str,
        status: IncidentStatus,
    ) -> Result<Incident, StoreError> {
        if !is_enabled() {
            return Err(StoreError::Disabled);
        }
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut index = self.load_index()?;
        let entry = index
            .entries
            .iter()
            .find(|e| e.incident_id == id_or_fingerprint || e.fingerprint == id_or_fingerprint)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id_or_fingerprint.to_string()))?;
        let mut incident = self.read_incident_at(&self.root.join(&entry.path))?;
        incident.status = status;
        incident.last_seen = Utc::now();
        self.write_incident_at(&self.root.join(&entry.path), &incident)?;
        self.upsert_index_entry(&mut index, &incident, &entry.path)?;
        self.append_event(LogEvent {
            ts: Utc::now(),
            incident_id: incident.incident_id.clone(),
            fingerprint: incident.fingerprint.clone(),
            action: "status".into(),
            detail: Some(status.as_str().to_string()),
        })?;
        Ok(incident)
    }

    fn load_index(&self) -> Result<IndexFile, StoreError> {
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

    fn write_index(&self, index: &IndexFile) -> Result<(), StoreError> {
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

    fn upsert_index_entry(
        &self,
        index: &mut IndexFile,
        incident: &Incident,
        rel_path: &str,
    ) -> Result<(), StoreError> {
        let entry = IndexEntry {
            incident_id: incident.incident_id.clone(),
            fingerprint: incident.fingerprint.clone(),
            title: incident.title.clone(),
            severity: incident.severity,
            status: incident.status,
            error_class: incident.error_class.as_str().to_string(),
            occurrence_count: incident.occurrence_count,
            first_seen: incident.first_seen.to_rfc3339(),
            last_seen: incident.last_seen.to_rfc3339(),
            path: rel_path.to_string(),
            component: incident.component.clone(),
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

    fn write_incident_at(&self, path: &Path, incident: &Incident) -> Result<(), StoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let pretty = serde_json::to_string_pretty(incident)?;
        fs::write(&tmp, pretty)?;
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }

    fn read_incident_at(&self, path: &Path) -> Result<Incident, StoreError> {
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn append_event(&self, event: LogEvent) -> Result<(), StoreError> {
        self.ensure_layout()?;
        let path = self.events_path();
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let line = serde_json::to_string(&event)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Read recent events (newest last).
    pub fn recent_events(&self, limit: usize) -> Result<Vec<LogEvent>, StoreError> {
        let path = self.events_path();
        if !path.is_file() {
            return Ok(vec![]);
        }
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut events: Vec<LogEvent> = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<LogEvent>(&line) {
                events.push(ev);
            }
        }
        if events.len() > limit {
            events = events.split_off(events.len() - limit);
        }
        Ok(events)
    }
}

/// Filters for [`DeveloperLogStore::list`].
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub severity: Option<Vec<Severity>>,
    pub status: Option<Vec<IncidentStatus>>,
    pub error_class: Option<String>,
    pub component: Option<String>,
    pub since: Option<chrono::DateTime<Utc>>,
    pub limit: Option<usize>,
    /// When true, include resolved/wontdo (default: open + acknowledged only).
    pub include_closed: bool,
}

impl ListFilter {
    fn matches(&self, e: &IndexEntry) -> bool {
        if let Some(ref sevs) = self.severity
            && !sevs.contains(&e.severity)
        {
            return false;
        }
        if let Some(ref statuses) = self.status {
            if !statuses.contains(&e.status) {
                return false;
            }
        } else if !self.include_closed
            && !matches!(
                e.status,
                IncidentStatus::Open | IncidentStatus::Acknowledged
            )
        {
            return false;
        }
        if let Some(ref class) = self.error_class
            && e.error_class != *class
        {
            return false;
        }
        if let Some(ref comp) = self.component
            && !e.component.iter().any(|c| c == comp)
        {
            return false;
        }
        if let Some(since) = self.since {
            if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&e.last_seen) {
                if last.with_timezone(&Utc) < since {
                    return false;
                }
            }
        }
        true
    }
}

fn merge_strings(dst: &mut Vec<String>, src: &[String]) {
    for s in src {
        if !s.is_empty() && !dst.iter().any(|d| d == s) {
            dst.push(s.clone());
        }
    }
    if dst.len() > 32 {
        dst.truncate(32);
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

/// Best-effort report that never panics; logs and returns `None` on failure.
pub fn report_best_effort(request: ReportRequest) -> Option<ReportResult> {
    if !is_enabled() {
        return None;
    }
    match DeveloperLogStore::default().report(request) {
        Ok(r) => {
            tracing::info!(
                incident_id = %r.incident_id,
                fingerprint = %r.fingerprint,
                is_new = r.is_new,
                occurrence_count = r.occurrence_count,
                error_class = %r.error_class,
                "developer_log reported incident"
            );
            Some(r)
        }
        Err(StoreError::Disabled) => None,
        Err(e) => {
            tracing::warn!(error = %e, "developer_log report failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ErrorClass, ReporterKind, Source};

    fn tmp_store() -> (tempfile::TempDir, DeveloperLogStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DeveloperLogStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn report_creates_and_dedups() {
        let (_dir, store) = tmp_store();
        let req = ReportRequest {
            title: "Worktree path unusable after complete".into(),
            summary: "meta still points at deleted worktree".into(),
            error_class: ErrorClass::WorktreeTombstone,
            component: vec!["worktree".into(), "subagent".into()],
            source: Source {
                reporter: ReporterKind::Runtime,
                auto: true,
                detector: Some("worktree_dispose".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let r1 = store.report(req.clone()).unwrap();
        assert!(r1.is_new);
        assert_eq!(r1.occurrence_count, 1);
        let r2 = store.report(req).unwrap();
        assert!(!r2.is_new);
        assert_eq!(r2.occurrence_count, 2);
        assert_eq!(r1.fingerprint, r2.fingerprint);
        assert_eq!(r1.incident_id, r2.incident_id);

        let listed = store.list(&ListFilter::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].occurrence_count, 2);

        let got = store.get(&r1.incident_id).unwrap();
        assert_eq!(got.occurrence_count, 2);
        assert_eq!(got.error_class, ErrorClass::WorktreeTombstone);
    }

    #[test]
    fn rejects_empty_title() {
        let (_dir, store) = tmp_store();
        let err = store
            .report(ReportRequest {
                title: "  ".into(),
                summary: "x".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)));
    }

    #[test]
    fn root_override_takes_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("my-adl");
        set_root_override(Some(custom.clone()));
        assert_eq!(default_root(), custom);
        set_root_override(None);
    }
}
