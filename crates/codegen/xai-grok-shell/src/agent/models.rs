//! Model fetching, resolution, and management.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::RwLock;

use agent_client_protocol as acp;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use indexmap::IndexMap;

use crate::agent::config::{self, ModelEntry, resolve_credentials, sampling_config_for_model};
use crate::auth::{AuthManager, GrokAuth, GrokComConfig};
use crate::remote::{FetchModelsResult, fetch_models_blocking};
use crate::sampling::SamplerConfig as SamplingConfig;
use globset::{Glob, GlobSet, GlobSetBuilder};
use xai_grok_sampling_types::{ReasoningEffort, ReasoningEffortOption};

// ── Auth method for model fetching ──────────────────────────────────────────

/// Credential for `/v1/models` fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelFetchAuth {
    Session,
    ApiKey,
    Deployment,
    CustomEndpoint,
}

impl ModelFetchAuth {
    /// custom_endpoint > session > deployment > API key.
    pub(crate) fn resolve(endpoints: &config::EndpointsConfig, has_cached_session: bool) -> Self {
        if endpoints.has_custom_endpoint() {
            Self::CustomEndpoint
        } else if has_cached_session {
            Self::Session
        } else if endpoints.deployment_key.is_some() {
            Self::Deployment
        } else if crate::agent::auth_method::has_xai_api_key_env() {
            Self::ApiKey
        } else {
            Self::Session
        }
    }

    fn cache_auth_method(&self) -> CacheAuthMethod {
        match self {
            Self::CustomEndpoint | Self::ApiKey => CacheAuthMethod::ApiKey,
            Self::Session => CacheAuthMethod::Session,
            Self::Deployment => CacheAuthMethod::Deployment,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Eq, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CacheAuthMethod {
    Session,
    ApiKey,
    Deployment,
}

pub(crate) fn spawn_requires_agent_ready(
    subagent_type: &str,
    capability_mode: Option<xai_tool_types::SubagentCapabilityMode>,
) -> bool {
    xai_tool_types::spawn_requires_agent_ready(subagent_type, capability_mode)
}

pub(crate) fn model_entry_agent_ready(entry: &ModelEntry) -> bool {
    match entry.info.request_compat.as_ref() {
        Some(xai_grok_models::RequestCompat::ChatCompletions(compat)) => compat.agent_ready,
        Some(_) => true,
        None => {
            let id = entry
                .info
                .id
                .as_deref()
                .unwrap_or(entry.info.model.as_str());
            xai_grok_models::parse_managed_model_key(id)
                .map(|(provider, _)| provider.as_str() != "nvidia")
                .unwrap_or(true)
        }
    }
}

fn catalog_entry_is_eol(key: &str) -> bool {
    xai_grok_models::catalog_key_is_eol(key)
        || xai_grok_models::is_nvidia_glm_52_eol_slug(key)
        || xai_grok_models::is_poolside_laguna_m1_eol_slug(key)
}

fn eol_model_error(requested: &str) -> String {
    format!(
        "Model '{requested}' is end-of-life (HTTP 410 Gone) and cannot be spawned. \
         NVIDIA Integrate withdrew z-ai/glm-5.2; Poolside retired laguna-m.1; pick an \
         agent-ready slug or omit `model` to inherit the parent model."
    )
}

fn chat_only_model_error(
    requested: &str,
    available: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
    config_keys: Option<&std::collections::HashSet<String>>,
) -> String {
    let catalog = spawnable_task_model_catalog(available, is_session_auth, config_keys);
    let agent_ready = if catalog.agent_ready.is_empty() {
        "none currently listed".to_string()
    } else {
        catalog.agent_ready.join(", ")
    };
    let chat_only = if catalog.chat_only.is_empty() {
        "none currently listed".to_string()
    } else {
        catalog.chat_only.join(", ")
    };
    format!(
        "Model '{requested}' is chat-only (catalog agent_ready=false) and cannot be used for \
         general-purpose / write-capable subagents. Chat-only models (including NVIDIA \
         llama-3.3-70b, gpt-oss-120b, and Nemotron Ultra 550B) may be used with explore/plan/oracle \
         or capability_mode=read-only. Agent-ready slugs: {agent_ready}. Chat-only slugs: {chat_only}."
    )
}

pub(crate) fn task_model_error_for_catalog(
    requested: &str,
    available: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
) -> Option<String> {
    // Slug + EOL only. Write-capable `agent_ready` is enforced at TaskTool
    // (which sees subagent_type). This entry is used by handle_request, which
    // must still allow explore/plan/oracle on chat-only NVIDIA rows.
    task_model_error_for_catalog_spawn(
        requested,
        available,
        is_session_auth,
        "explore",
        Some(xai_tool_types::SubagentCapabilityMode::ReadOnly),
        None,
    )
}

pub(crate) fn task_model_error_for_catalog_spawn(
    requested: &str,
    available: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
    subagent_type: &str,
    capability_mode: Option<xai_tool_types::SubagentCapabilityMode>,
    config_keys: Option<&std::collections::HashSet<String>>,
) -> Option<String> {
    if xai_grok_models::is_nvidia_glm_52_eol_slug(requested) || catalog_entry_is_eol(requested) {
        return Some(eol_model_error(requested));
    }

    let is_available = |key: &str, entry: &ModelEntry| {
        spawn_catalog_candidate(key, entry, is_session_auth, config_keys)
    };

    if let Some((key, entry)) = find_task_model_entry(available, requested) {
        if catalog_entry_is_eol(key) {
            return Some(eol_model_error(requested));
        }
        if spawn_requires_agent_ready(subagent_type, capability_mode)
            && !model_entry_agent_ready(entry)
            && is_available(key, entry)
        {
            return Some(chat_only_model_error(
                requested,
                available,
                is_session_auth,
                config_keys,
            ));
        }
        if is_available(key, entry) {
            return None;
        }
    }

    let catalog = spawnable_task_model_catalog(available, is_session_auth, config_keys);
    let mut slugs = catalog.agent_ready;
    slugs.extend(catalog.chat_only);
    slugs.sort_unstable();
    slugs.dedup();
    let guidance = if slugs.is_empty() {
        "No valid model slugs are currently available. Omit `model` to inherit the parent model."
            .to_string()
    } else {
        format!(
            "Valid model slugs: {}. Omit `model` to inherit the parent model.",
            slugs.join(", ")
        )
    };
    Some(format!("Unknown Task.model slug '{requested}'. {guidance}"))
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SpawnableTaskModelCatalog {
    pub advertised: Vec<String>,
    pub agent_ready: Vec<String>,
    pub chat_only: Vec<String>,
}

fn spawn_catalog_candidate(
    key: &str,
    entry: &ModelEntry,
    is_session_auth: bool,
    config_keys: Option<&std::collections::HashSet<String>>,
) -> bool {
    if catalog_entry_is_eol(key) {
        return false;
    }
    if !entry.info.user_selectable {
        return false;
    }
    if entry.visible_for_auth(is_session_auth) {
        return true;
    }
    config_keys.is_some_and(|keys| keys.contains(key)) && entry.has_own_credentials()
}

fn openai_codex_spawn_aliases(slugs: &[String]) -> Vec<String> {
    slugs
        .iter()
        .filter_map(|key| {
            key.strip_prefix("openai-codex/")
                .map(|rest| format!("openai/{rest}"))
        })
        .collect()
}

pub(crate) fn spawnable_task_model_catalog(
    available: &IndexMap<String, ModelEntry>,
    is_session_auth: bool,
    config_keys: Option<&std::collections::HashSet<String>>,
) -> SpawnableTaskModelCatalog {
    let mut agent_ready = Vec::new();
    let mut chat_only = Vec::new();
    let mut configured_chat_only = Vec::new();
    for (key, entry) in available {
        if !spawn_catalog_candidate(key, entry, is_session_auth, config_keys) {
            continue;
        }
        if model_entry_agent_ready(entry) {
            agent_ready.push(key.clone());
        } else {
            chat_only.push(key.clone());
            if config_keys.is_some_and(|keys| keys.contains(key)) {
                configured_chat_only.push(key.clone());
            }
        }
    }
    agent_ready.extend(openai_codex_spawn_aliases(&agent_ready));
    chat_only.extend(openai_codex_spawn_aliases(&chat_only));
    configured_chat_only.extend(openai_codex_spawn_aliases(&configured_chat_only));
    agent_ready.sort_unstable();
    agent_ready.dedup();
    chat_only.sort_unstable();
    chat_only.dedup();
    let mut advertised = agent_ready.clone();
    advertised.extend(configured_chat_only);
    advertised.sort_unstable();
    advertised.dedup();
    SpawnableTaskModelCatalog {
        advertised,
        agent_ready,
        chat_only,
    }
}

/// Common slugs agents copy from session briefs / NVIDIA docs that are not
/// the catalog key Turbo spawn accepts.
///
/// - Luna: `openai/gpt-5.6-luna` → `openai-codex/gpt-5.6-luna`
/// - Lightning: `nvidia/nemotron-3.5-lightning-30b-a3b` →
///   `nvidia/nvidia/nemotron-3.5-lightning-30b-a3b`
/// - Muse Glimmer / Laguna XS / Mistral-Nemotron NVIDIA Integrate ids
fn task_model_aliases(requested: &str) -> Vec<String> {
    let requested = requested.trim();
    let lower = requested.to_ascii_lowercase();
    let mut out = Vec::new();

    const CODEX_FAMILY: &[(&str, &str)] = &[
        ("openai/gpt-5.6-luna", "openai-codex/gpt-5.6-luna"),
        ("openai/gpt-5.6-luna-pro", "openai-codex/gpt-5.6-luna-pro"),
        ("gpt-5.6-luna", "openai-codex/gpt-5.6-luna"),
        ("gpt-5.6-luna-pro", "openai-codex/gpt-5.6-luna-pro"),
        ("openai/gpt-5.6-sol", "openai-codex/gpt-5.6-sol"),
        ("openai/gpt-5.6-sol-pro", "openai-codex/gpt-5.6-sol-pro"),
        ("gpt-5.6-sol", "openai-codex/gpt-5.6-sol"),
        ("gpt-5.6-sol-pro", "openai-codex/gpt-5.6-sol-pro"),
        ("openai/gpt-5.6-terra", "openai-codex/gpt-5.6-terra"),
        ("openai/gpt-5.6-terra-pro", "openai-codex/gpt-5.6-terra-pro"),
        ("gpt-5.6-terra", "openai-codex/gpt-5.6-terra"),
        ("gpt-5.6-terra-pro", "openai-codex/gpt-5.6-terra-pro"),
        ("openai/gpt-5.5", "openai-codex/gpt-5.5"),
        ("openai/gpt-5.5-pro", "openai-codex/gpt-5.5-pro"),
        ("gpt-5.5", "openai-codex/gpt-5.5"),
        ("gpt-5.5-pro", "openai-codex/gpt-5.5-pro"),
        ("openai/gpt-5.4", "openai-codex/gpt-5.4"),
        ("openai/gpt-5.4-mini", "openai-codex/gpt-5.4-mini"),
        ("gpt-5.4", "openai-codex/gpt-5.4"),
        ("gpt-5.4-mini", "openai-codex/gpt-5.4-mini"),
        (
            "openai/gpt-5.3-codex-spark",
            "openai-codex/gpt-5.3-codex-spark",
        ),
        ("gpt-5.3-codex-spark", "openai-codex/gpt-5.3-codex-spark"),
    ];
    for (from, to) in CODEX_FAMILY {
        if lower == *from || lower.ends_with(&format!("/{from}")) {
            out.push((*to).to_string());
        }
    }
    // Generic openai/foo → openai-codex/foo so spawn descriptions and retries
    // match the catalog key the runtime actually accepts.
    if let Some(rest) = lower.strip_prefix("openai/")
        && !rest.is_empty()
        && !rest.starts_with("codex/")
        && !rest.starts_with("gpt-oss")
    {
        out.push(format!("openai-codex/{rest}"));
    }

    if lower.contains("nemotron-3.5-lightning")
        || lower.contains("nemotron-3-5-lightning")
        || lower.contains("nemotron-3.5-lightning-30b")
    {
        out.push("nvidia/nvidia/nemotron-3.5-lightning-30b-a3b".into());
        out.push("nvidia/nemotron-3.5-lightning-30b-a3b".into());
    }
    if lower.contains("muse-glimmer") {
        out.push("nvidia/meta/muse-glimmer-30b".into());
        out.push("nvidia/muse-glimmer-30b".into());
    }
    if lower.contains("laguna-xs-2.1") || lower.ends_with("laguna-xs") {
        out.push("nvidia/poolside/laguna-xs-2.1".into());
        out.push("nvidia/laguna-xs-2.1".into());
    }
    if lower.contains("mistral-nemotron") {
        out.push("nvidia/mistralai/mistral-nemotron".into());
        out.push("nvidia/mistral-nemotron".into());
    }
    out
}

/// Resolve a Task.model slug against the catalog, including common provider
/// prefix aliases agents copy from platform docs (`amazon-bedrock/…`, …).
///
/// Exact key / routing-slug match first ([`config::find_model_by_id`]); then:
/// - strip a known `provider/` prefix and retry
/// - if bare, try `amazon-bedrock/{requested}` and other common prefixes
/// - suffix match: catalog key ends with `/{requested}` or equals the bare id
///
/// Returns `(catalog_key, entry)` so callers can pin the canonical id. The key
/// is always borrowed from `available` (never from the request string).
pub(crate) fn find_task_model_entry<'a>(
    available: &'a IndexMap<String, ModelEntry>,
    requested: &str,
) -> Option<(&'a str, &'a ModelEntry)> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }

    /// Prefer the map key for `entry`, else `info.model` if it is a key.
    fn key_for<'a>(
        available: &'a IndexMap<String, ModelEntry>,
        entry: &'a ModelEntry,
        preferred: Option<&str>,
    ) -> Option<&'a str> {
        if let Some(p) = preferred
            && available.contains_key(p)
        {
            return available.get_key_value(p).map(|(k, _)| k.as_str());
        }
        if let Some((key, _)) = available.iter().find(|(_, e)| std::ptr::eq(*e, entry)) {
            return Some(key.as_str());
        }
        if available.contains_key(entry.info.model.as_str()) {
            return available
                .get_key_value(entry.info.model.as_str())
                .map(|(k, _)| k.as_str());
        }
        None
    }

    // Known remaps first so a non-selectable OpenRouter `openai/gpt-5.6-luna`
    // row cannot shadow the spawnable ChatGPT Codex slug.
    for alias in task_model_aliases(requested) {
        if let Some(entry) = config::find_model_by_id(available, &alias) {
            if let Some(key) = key_for(available, entry, Some(alias.as_str())) {
                return Some((key, entry));
            }
        }
    }

    if let Some(entry) = config::find_model_by_id(available, requested) {
        let key = key_for(available, entry, Some(requested))?;
        return Some((key, entry));
    }

    const PROVIDER_PREFIXES: &[&str] = &[
        "amazon-bedrock/",
        "bedrock/",
        "azure-openai/",
        "azure-openai-responses/",
        "openrouter/",
        "vercel-ai-gateway/",
        "github-copilot/",
        "nvidia/",
        "openai/",
        "anthropic/",
        "google/",
    ];

    // Strip one known provider prefix and retry exact lookup.
    for prefix in PROVIDER_PREFIXES {
        if let Some(rest) = requested.strip_prefix(prefix)
            && !rest.is_empty()
        {
            if let Some(entry) = config::find_model_by_id(available, rest) {
                if let Some(key) = key_for(available, entry, Some(rest)) {
                    return Some((key, entry));
                }
            }
            // Full bedrock-style keys often keep the vendor id after the slash:
            // requested `amazon-bedrock/openai.gpt-5.6-luna` → try suffix match.
            if let Some((key, entry)) = available.iter().find(|(key, entry)| {
                let routing = entry.info.model.as_str();
                *key == requested
                    || key.ends_with(&format!("/{rest}"))
                    || key.ends_with(rest)
                    || routing == rest
                    || routing == requested
            }) {
                return Some((key.as_str(), entry));
            }
        }
    }

    // Bare id → try common provider-prefixed catalog keys.
    if !requested.contains('/') {
        for prefix in PROVIDER_PREFIXES {
            let candidate = format!("{prefix}{requested}");
            if let Some(entry) = config::find_model_by_id(available, &candidate) {
                if let Some(key) = key_for(available, entry, Some(candidate.as_str())) {
                    return Some((key, entry));
                }
            }
        }
        // Suffix: catalog key ends with /{requested}
        if let Some((key, entry)) = available.iter().find(|(key, entry)| {
            key.ends_with(&format!("/{requested}")) || entry.info.model == requested
        }) {
            return Some((key.as_str(), entry));
        }
    }

    None
}

/// Thread-safe model manager.
#[derive(Clone)]
pub struct ModelsManager {
    inner: Arc<Inner>,
}

/// Progress of the first real-catalog load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogProgress {
    Pending,
    Failed,
    Ready,
}

/// Catalog fields written together under one lock, so readers never see a torn mix.
#[derive(Default)]
struct CatalogState {
    prefetched: Option<IndexMap<String, ModelEntry>>,
    models: IndexMap<String, ModelEntry>,
    etag: Option<String>,
    /// Gates whether the apply path reselects the default (first real catalog)
    has_fetched_real_catalog: bool,
    /// `allowed_models` matched nothing; the prompt path blocks instead.
    allowlist_excludes_all: bool,
    /// Bumped on identity change; a fetch captured before it must not apply.
    generation: u64,
}

struct Inner {
    catalog: RwLock<CatalogState>,
    current_model_id: RwLock<acp::ModelId>,
    current_reasoning_effort: RwLock<Option<ReasoningEffort>>,
    // ── Owned context for self-contained refresh ────────────────
    auth_manager: Arc<AuthManager>,
    cfg: RwLock<config::Config>,
    fetch_auth: RwLock<ModelFetchAuth>,
    gateway: RwLock<Option<xai_acp_lib::AcpAgentGatewaySender>>,
    cache: ModelsCacheManager,
    endpoint: Arc<dyn ModelsEndpoint>,
    /// Guard to prevent overlapping retry loops.
    retry_in_flight: AtomicBool,
    /// Single-flight for the etag-triggered background refresh (`spawn_fetch`).
    refresh_in_flight: AtomicBool,
    fetches_in_flight: AtomicUsize,
    /// Model-switch signal: a generation counter bumped when the current model id changes.
    model_switch_watch: tokio::sync::watch::Sender<u64>,
    /// Progress of the first real-catalog load, watched by bounded waits.
    catalog_progress: tokio::sync::watch::Sender<CatalogProgress>,
    /// Set once the user explicitly picks a model (`/model`); guards the
    /// first-catalog reselect from clobbering that choice.
    user_selected_model: AtomicBool,
}

/// Clears an in-flight flag on drop so a panicking task can't wedge future refreshes.
struct RetryInFlightGuard(Arc<Inner>);
impl Drop for RetryInFlightGuard {
    fn drop(&mut self) {
        self.0.retry_in_flight.store(false, Ordering::Release);
    }
}
struct RefreshInFlightGuard(Arc<Inner>);
impl Drop for RefreshInFlightGuard {
    fn drop(&mut self) {
        self.0.refresh_in_flight.store(false, Ordering::Release);
    }
}

/// One fetch attempt (or retry sequence), counted for bounded waiters.
/// Begin before spawning the task; beginning supersedes an earlier `Failed`.
struct FetchAttemptGuard {
    inner: Arc<Inner>,
    generation: u64,
}
impl FetchAttemptGuard {
    fn begin(inner: &Arc<Inner>) -> Self {
        // Count first: a waiter that sees `Pending` must also see the attempt.
        inner.fetches_in_flight.fetch_add(1, Ordering::AcqRel);
        inner.catalog_progress.send_if_modified(|p| {
            let supersede = *p == CatalogProgress::Failed;
            if supersede {
                *p = CatalogProgress::Pending;
            }
            supersede
        });
        let generation = inner.catalog.read().generation;
        Self {
            inner: inner.clone(),
            generation,
        }
    }
}
impl Drop for FetchAttemptGuard {
    fn drop(&mut self) {
        if self.inner.fetches_in_flight.fetch_sub(1, Ordering::AcqRel) > 1 {
            return;
        }
        // Last attempt out with no outcome: latch so waiters return. The
        // lock makes the generation check atomic against `clear()`.
        let cat = self.inner.catalog.read();
        if cat.generation != self.generation
            || self.inner.fetches_in_flight.load(Ordering::Acquire) > 0
        {
            return;
        }
        self.inner.catalog_progress.send_if_modified(|p| {
            let unresolved = *p == CatalogProgress::Pending;
            if unresolved {
                *p = CatalogProgress::Failed;
            }
            unresolved
        });
    }
}

impl Default for ModelsManager {
    fn default() -> Self {
        let grok_home = crate::util::grok_home::grok_home();
        let auth_manager = Arc::new(AuthManager::new(&grok_home, GrokComConfig::default()));
        Self::new(
            None,
            IndexMap::new(),
            acp::ModelId::new("default"),
            auth_manager,
            config::Config::default(),
        )
    }
}

/// Builder for [`ModelsManager`]; transport and disk cache default to production (tests override them).
pub(crate) struct ModelsManagerBuilder {
    prefetched: Option<IndexMap<String, ModelEntry>>,
    models: IndexMap<String, ModelEntry>,
    current_model_id: acp::ModelId,
    auth_manager: Arc<AuthManager>,
    cfg: config::Config,
    endpoint: Arc<dyn ModelsEndpoint>,
    cache: ModelsCacheManager,
}

impl ModelsManagerBuilder {
    pub(crate) fn new(
        prefetched: Option<IndexMap<String, ModelEntry>>,
        models: IndexMap<String, ModelEntry>,
        current_model_id: acp::ModelId,
        auth_manager: Arc<AuthManager>,
        cfg: config::Config,
    ) -> Self {
        Self {
            prefetched,
            models,
            current_model_id,
            auth_manager,
            cfg,
            endpoint: Arc::new(HttpModelsEndpoint),
            cache: ModelsCacheManager::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn endpoint(mut self, endpoint: Arc<dyn ModelsEndpoint>) -> Self {
        self.endpoint = endpoint;
        self
    }

    #[cfg(test)]
    pub(crate) fn cache(mut self, cache: ModelsCacheManager) -> Self {
        self.cache = cache;
        self
    }

    pub(crate) fn build(self) -> ModelsManager {
        let has_session = self.auth_manager.current_or_expired().is_some();
        let fetch_auth = ModelFetchAuth::resolve(&self.cfg.endpoints, has_session);
        let current_reasoning_effort = self.cfg.models.default_reasoning_effort;
        ModelsManager {
            inner: Arc::new(Inner {
                catalog: RwLock::new(CatalogState {
                    prefetched: self.prefetched,
                    models: self.models,
                    ..Default::default()
                }),
                current_model_id: RwLock::new(self.current_model_id),
                current_reasoning_effort: RwLock::new(current_reasoning_effort),
                auth_manager: self.auth_manager,
                cfg: RwLock::new(self.cfg),
                fetch_auth: RwLock::new(fetch_auth),
                gateway: RwLock::new(None),
                cache: self.cache,
                endpoint: self.endpoint,
                retry_in_flight: AtomicBool::new(false),
                refresh_in_flight: AtomicBool::new(false),
                fetches_in_flight: AtomicUsize::new(0),
                model_switch_watch: tokio::sync::watch::channel(0u64).0,
                catalog_progress: tokio::sync::watch::channel(CatalogProgress::Pending).0,
                user_selected_model: AtomicBool::new(false),
            }),
        }
    }
}

impl ModelsManager {
    pub(crate) fn new(
        prefetched: Option<IndexMap<String, ModelEntry>>,
        models: IndexMap<String, ModelEntry>,
        current_model_id: acp::ModelId,
        auth_manager: Arc<AuthManager>,
        cfg: config::Config,
    ) -> Self {
        ModelsManagerBuilder::new(prefetched, models, current_model_id, auth_manager, cfg).build()
    }

    /// Subscribe to model-switch events. Returns a `watch::Receiver`
    pub(crate) fn subscribe_model_switch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.model_switch_watch.subscribe()
    }

    /// Cheap snapshot of the current model-switch generation, for the laziness-check poll loop.
    pub(crate) fn model_switch_generation(&self) -> u64 {
        *self.inner.model_switch_watch.borrow()
    }

    /// Build from a resolved config. Falls back to bundled default if no models available.
    pub fn from_config(
        cfg: &config::Config,
        prefetched_models: Option<IndexMap<String, ModelEntry>>,
        auth_manager: Arc<AuthManager>,
    ) -> Result<Self, String> {
        let has_session = auth_manager.current_or_expired().is_some();
        let is_session_auth = auth_manager
            .current_or_expired()
            .is_some_and(|a| a.is_session_auth());
        let fetch_auth = ModelFetchAuth::resolve(&cfg.endpoints, has_session);
        let mut cached_etag = None;
        let prefetched_models = prefetched_models.or_else(|| {
            let cache = ModelsCacheManager::new();
            cache
                .load_fresh(
                    &fetch_auth.cache_auth_method(),
                    &crate::remote::models_list_url(&cfg.endpoints, fetch_auth),
                )
                .map(|c| {
                    cached_etag = c.etag;
                    c.models
                })
        });
        let has_prefetched = prefetched_models.is_some();
        let catalog = resolve_model_catalog(cfg, prefetched_models.clone());

        if has_prefetched {
            validate_selectable(cfg, &catalog)?;
        }

        let (current_model_key, current_model, model_source) =
            resolve_default_model(cfg, &catalog, is_session_auth);

        tracing::info!(
            model_id = %current_model.model,
            source = %model_source,
            "default model resolved"
        );

        let current_model_id = acp::ModelId::new(Arc::from(current_model_key));

        let mgr = Self::new(
            prefetched_models,
            catalog,
            current_model_id,
            auth_manager,
            cfg.clone(),
        );
        if has_prefetched {
            let mut cat = mgr.inner.catalog.write();
            cat.has_fetched_real_catalog = true;
            // With the etag, the first check renews instead of refetching.
            cat.etag = cached_etag;
            mgr.inner
                .catalog_progress
                .send_replace(CatalogProgress::Ready);
        }
        Ok(mgr)
    }

    pub(crate) fn set_gateway(&self, gateway: xai_acp_lib::AcpAgentGatewaySender) {
        *self.inner.gateway.write() = Some(gateway);
    }

    /// Swap config, rebuild catalog, and reselect the model.
    pub(crate) fn apply_config(&self, new_config: config::Config) {
        if let Err(e) = new_config.validate_model_filters() {
            tracing::error!(error = %e, "ignoring config reload: invalid model filters");
            return;
        }
        let prefetched = self.inner.catalog.read().prefetched.clone();
        let new_catalog = resolve_model_catalog(&new_config, prefetched);
        let has_real_catalog = self.inner.catalog.read().has_fetched_real_catalog;
        if has_real_catalog && let Err(e) = validate_selectable(&new_config, &new_catalog) {
            tracing::error!(error = %e, "ignoring config reload: allowed_models excludes all models");
            return;
        }

        let (old_preferred, old_default_is_campaign) = {
            let cfg = self.inner.cfg.read();
            (
                cfg.models.default.clone(),
                cfg.models.default_is_campaign_driven,
            )
        };
        let new_preferred = new_config.models.default.clone();
        let has_session = self.inner.auth_manager.current_or_expired().is_some();
        *self.inner.fetch_auth.write() =
            ModelFetchAuth::resolve(&new_config.endpoints, has_session);
        *self.inner.cfg.write() = new_config.clone();
        {
            let mut cat = self.inner.catalog.write();
            if has_real_catalog {
                cat.allowlist_excludes_all = allowlist_matches_nothing(&new_config, &new_catalog);
            }
            cat.models = new_catalog;
        }

        let preferred_changed = new_preferred != old_preferred && new_preferred.is_some();
        let mut campaign_defaults = std::collections::HashSet::new();
        if new_config.models.default_is_campaign_driven
            && let Some(d) = &new_preferred
        {
            campaign_defaults.insert(d.clone());
        }
        if old_default_is_campaign && let Some(d) = &old_preferred {
            campaign_defaults.insert(d.clone());
        }
        let campaign_only_flip =
            is_campaign_only_flip(&old_preferred, &new_preferred, &campaign_defaults);
        let current_still_ok = {
            let cat = self.inner.catalog.read();
            let models = &cat.models;
            let cur = self.inner.current_model_id.read();
            models
                .get(cur.0.as_ref())
                .is_some_and(|e| e.info.user_selectable)
        };
        if preferred_changed && !(campaign_only_flip && current_still_ok) {
            self.reselect_default_model(&new_config);
        } else {
            self.reselect_current_model_if_missing(&new_config);
        }

        self.notify_models_updated();
    }

    /// [`Self::apply_config`] plus an unconditional default re-resolve, for remote-settings arrival while no session exists.
    pub(crate) fn apply_config_reselecting_default(&self, new_config: config::Config) {
        self.apply_config(new_config.clone());
        self.reselect_default_model(&new_config);
        self.notify_models_updated();
    }

    // ── Accessors ───────────────────────────────────────────────────

    pub fn models(&self) -> IndexMap<String, ModelEntry> {
        self.inner.catalog.read().models.clone()
    }

    pub fn endpoints(&self) -> config::EndpointsConfig {
        self.inner.cfg.read().endpoints.clone()
    }

    /// Does the current credential grant access to OAuth-only models?
    fn is_session_auth(&self) -> bool {
        self.inner
            .auth_manager
            .current_or_expired()
            .is_some_and(|a| a.is_session_auth())
    }

    /// ACP-visible (non-hidden) projection of the catalog.
    pub fn available(&self) -> IndexMap<acp::ModelId, acp::ModelInfo> {
        let snapshot = {
            let cat = self.inner.catalog.read();
            let models = &cat.models;
            models.clone()
        };

        let selectable: IndexMap<_, _> = snapshot
            .into_iter()
            .filter(|(_, e)| e.info.user_selectable)
            .collect();

        let mut available = available_models(&selectable, self.is_session_auth());
        let config_model_ids = self
            .inner
            .cfg
            .read()
            .config_models
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in config_model_ids {
            if let Some(info) = available.get_mut(&acp::ModelId::new(id)) {
                info.meta
                    .get_or_insert_with(serde_json::Map::new)
                    .insert(config::CONFIG_MODEL_META_KEY.to_string(), true.into());
            }
        }
        available
    }

    pub(crate) fn task_model_error(&self, requested: &str) -> Option<String> {
        self.task_model_error_for_spawn(requested, "general-purpose", None)
    }

    pub(crate) fn task_model_error_for_spawn(
        &self,
        requested: &str,
        subagent_type: &str,
        capability_mode: Option<xai_tool_types::SubagentCapabilityMode>,
    ) -> Option<String> {
        let is_session_auth = self.is_session_auth();
        let models = self.inner.catalog.read().models.clone();
        let config_keys: std::collections::HashSet<String> = self
            .inner
            .cfg
            .read()
            .config_models
            .keys()
            .cloned()
            .collect();
        task_model_error_for_catalog_spawn(
            requested,
            &models,
            is_session_auth,
            subagent_type,
            capability_mode,
            Some(&config_keys),
        )
    }

    fn spawn_catalog(&self) -> SpawnableTaskModelCatalog {
        let is_session_auth = self.is_session_auth();
        let models = self.inner.catalog.read().models.clone();
        let config_keys: std::collections::HashSet<String> = self
            .inner
            .cfg
            .read()
            .config_models
            .keys()
            .cloned()
            .collect();
        spawnable_task_model_catalog(&models, is_session_auth, Some(&config_keys))
    }

    /// Agent-ready catalog keys plus credentialed config entries that must be
    /// discoverable even when they fail closed as chat-only.
    pub(crate) fn spawnable_task_model_slugs(&self) -> Vec<String> {
        self.spawn_catalog().advertised
    }

    /// Chat-only catalog keys (agent_ready=false) for explore/plan/oracle or
    /// `capability_mode=read-only`. Hidden from the default general-purpose list.
    pub(crate) fn spawnable_task_model_chat_only_slugs(&self) -> Vec<String> {
        self.spawn_catalog().chat_only
    }

    pub fn current_model_id(&self) -> acp::ModelId {
        self.inner.current_model_id.read().clone()
    }

    pub(crate) fn set_current_model_id(&self, id: acp::ModelId) {
        self.inner
            .user_selected_model
            .store(true, Ordering::Relaxed);
        self.set_current_model_id_internal(id);
    }

    fn set_current_model_id_internal(&self, id: acp::ModelId) {
        let changed = {
            let mut cur = self.inner.current_model_id.write();
            let changed = *cur != id;
            *cur = id;
            changed
        };
        if changed {
            self.inner
                .model_switch_watch
                .send_modify(|generation| *generation += 1);
        }
    }

    /// Per-model Layer-3 LazinessDetector config for `model_id` (disabled default when absent).
    pub(crate) fn laziness_detector_for(
        &self,
        model_id: &str,
    ) -> config::LazinessDetectorPerModelConfig {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .map(|e| e.info().laziness_detector.clone())
            .unwrap_or_default()
    }

    /// Test-only catalog poke: inserts a `ModelEntry` keyed by `id`,
    #[cfg(test)]
    pub(crate) fn insert_test_entry(&self, id: impl Into<String>, entry: ModelEntry) {
        self.inner.catalog.write().models.insert(id.into(), entry);
    }

    pub(crate) fn current_reasoning_effort(&self) -> Option<ReasoningEffort> {
        *self.inner.current_reasoning_effort.read()
    }

    pub(crate) fn set_current_reasoning_effort(&self, effort: Option<ReasoningEffort>) {
        *self.inner.current_reasoning_effort.write() = effort;
    }

    /// Whether the given model supports reasoning effort according to the catalog.
    pub(crate) fn model_supports_reasoning_effort(&self, model_id: &str) -> bool {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .map(|e| e.info().supports_reasoning_effort)
            .unwrap_or(false)
    }

    pub(crate) fn model_default_reasoning_effort(&self, model_id: &str) -> Option<ReasoningEffort> {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .and_then(|e| e.info().reasoning_effort)
    }

    /// The raw catalog `reasoning_efforts` list for `model_id` with no fallback,
    pub(crate) fn model_reasoning_efforts(&self, model_id: &str) -> Vec<ReasoningEffortOption> {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .map(|e| e.info().reasoning_efforts.clone())
            .unwrap_or_default()
    }

    pub(crate) fn model_supports_backend_search(&self, model_id: &str) -> bool {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .map(|e| e.info().supports_backend_search)
            .unwrap_or(false)
    }

    pub(crate) fn model_compactions_remaining(
        &self,
        model_id: &str,
    ) -> Option<xai_grok_sampling_types::CompactionsRemaining> {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .and_then(|e| e.info().compactions_remaining)
    }

    pub(crate) fn model_compaction_at_tokens(
        &self,
        model_id: &str,
    ) -> Option<xai_grok_sampling_types::CompactionAtTokens> {
        self.inner
            .catalog
            .read()
            .models
            .get(model_id)
            .and_then(|e| e.info().compaction_at_tokens)
    }

    /// Catalog opt-in to display the served-checkpoint fingerprint for this model.
    pub(crate) fn model_show_model_fingerprint(&self, model_id: &str) -> bool {
        let cat = self.inner.catalog.read();
        let models = &cat.models;
        resolve_catalog_key(models, &acp::ModelId::new(model_id))
            .and_then(|key| models.get(key.0.as_ref()))
            .map(|e| e.info().show_model_fingerprint)
            .unwrap_or(false)
    }

    /// Resolved next-prompt-suggestion model pin from the live config
    pub(crate) fn prompt_suggest_model_pin(&self) -> crate::config::PromptSuggestModelPin {
        self.inner.cfg.read().prompt_suggest_model_pin.clone()
    }

    /// Whether `model_id` resolves in the current catalog — as a config key
    pub(crate) fn model_in_catalog(&self, model_id: &str) -> bool {
        let cat = self.inner.catalog.read();
        let models = &cat.models;
        resolve_catalog_key(models, &acp::ModelId::new(model_id)).is_some()
    }

    #[cfg(test)]
    fn prefetched(&self) -> Option<IndexMap<String, ModelEntry>> {
        self.inner.catalog.read().prefetched.clone()
    }

    #[cfg(test)]
    fn has_fetched_real_catalog(&self) -> bool {
        self.inner.catalog.read().has_fetched_real_catalog
    }

    /// Wait, bounded by one auth refresh plus one fetch, for the first
    /// fetch outcome; never triggers a fetch.
    pub(crate) async fn wait_for_first_catalog(&self) {
        self.wait_for_first_catalog_inner(crate::util::config::resolve_remote_fetch_enabled())
            .await;
    }

    async fn wait_for_first_catalog_inner(&self, remote_fetch_enabled: bool) -> bool {
        const BUDGET: std::time::Duration = crate::http::STARTUP_AUTH_REFRESH_TIMEOUT
            .saturating_add(crate::http::STARTUP_FETCH_TIMEOUT);
        let mut progress = self.inner.catalog_progress.subscribe();
        match *progress.borrow() {
            CatalogProgress::Ready => return true,
            CatalogProgress::Failed => return false,
            CatalogProgress::Pending => {}
        }
        if !remote_fetch_enabled {
            return false;
        }
        // Signed out with a session-only endpoint: no fetch is coming.
        if *self.inner.fetch_auth.read() == ModelFetchAuth::Session
            && self.inner.auth_manager.current_or_expired().is_none()
        {
            return false;
        }
        // Attempts latch `Failed` on exit, so pending plus idle means none started.
        if self.inner.fetches_in_flight.load(Ordering::Acquire) == 0 {
            return *progress.borrow() == CatalogProgress::Ready;
        }
        matches!(
            tokio::time::timeout(BUDGET, progress.wait_for(|p| *p != CatalogProgress::Pending))
                .await,
            Ok(Ok(p)) if *p == CatalogProgress::Ready
        )
    }

    // ── Mutations ───────────────────────────────────────────────────

    fn rebuild(&self, cfg: &config::Config, prefetched: Option<IndexMap<String, ModelEntry>>) {
        self.inner.catalog.write().models = resolve_model_catalog(cfg, prefetched);
    }

    /// Re-run catalog resolution so platform credentials (e.g. a freshly
    /// minted Kimi Code OAuth bearer) are stamped onto platform entries.
    /// Also attempts a live `GET {base}/models` per enabled platform with
    /// credentials so subscription listings replace offline fallbacks.
    pub fn restamp_platform_credentials(&self) {
        let cfg = self.inner.cfg.read().clone();
        let prefetched = self.inner.catalog.read().prefetched.clone();
        // First stamp credentials from disk (no network) so the picker unlocks
        // immediately even if the live listing fails.
        self.rebuild(&cfg, prefetched.clone());
        let merged = crate::agent::platform_models_fetch::fetch_and_merge_platform_models(
            prefetched,
            &cfg.platforms,
        );
        {
            let mut cat = self.inner.catalog.write();
            cat.prefetched = merged.clone();
            cat.models = resolve_model_catalog(&cfg, merged);
        }
        // Clearing or expiring a provider credential leaves its models in the
        // picker as locked discovery rows. Such a row must not remain the
        // active sampling model; fall back before publishing the update.
        self.reselect_current_model_if_missing(&cfg);
        self.notify_models_updated();
    }

    /// Reset to this identity's bundled catalog and reselect a valid default.
    fn rebuild_bundled(&self, cfg: &config::Config) {
        self.rebuild(cfg, None);
        self.reselect_current_model_if_missing(cfg);
    }

    /// Refresh models when the etag changes.
    pub(crate) async fn refresh_if_new_etag(&self, etag: String) {
        let same_etag = {
            let cat = self.inner.catalog.read();
            cat.etag.as_deref() == Some(etag.as_str())
        };
        if same_etag {
            let fetch_auth = *self.inner.fetch_auth.read();
            self.inner
                .cache
                .renew_ttl(&fetch_auth.cache_auth_method(), &self.cache_origin())
                .await;
            return;
        }
        tracing::info!(etag = %etag, "models etag changed, refreshing");
        self.spawn_fetch(Some(etag));
    }

    /// Auth identity changed: invalidate the disk cache and refresh the catalog.
    pub(crate) async fn on_auth_changed(&self) {
        let config = self.inner.cfg.read().clone();
        crate::agent::init::update_telemetry_config(&config, &self.inner.auth_manager);
        self.inner.cache.invalidate();
        // Fetches and the etag from the previous identity are stale now.
        {
            let mut cat = self.inner.catalog.write();
            cat.generation += 1;
            cat.etag = None;
        }
        let has_session = self.inner.auth_manager.current_or_expired().is_some();
        let fetch_auth = ModelFetchAuth::resolve(&config.endpoints, has_session);
        *self.inner.fetch_auth.write() = fetch_auth;
        // No session but the endpoint needs one: a fetch would 401, so skip it
        // and reset to this identity's bundled catalog.
        if !has_session && fetch_auth == ModelFetchAuth::Session {
            self.clear();
            self.rebuild_bundled(&config);
            // No fetch is coming; wake parked waiters. Lock and gate like
            // every other outcome publish.
            {
                let _cat = self.inner.catalog.read();
                self.inner.catalog_progress.send_if_modified(|p| {
                    let pending = *p == CatalogProgress::Pending;
                    if pending {
                        *p = CatalogProgress::Failed;
                    }
                    pending
                });
            }
            self.notify_models_updated();
            return;
        }

        let remote_fetch_enabled = crate::util::config::resolve_remote_fetch_enabled();
        self.fetch_and_apply_inner(remote_fetch_enabled).await;

        let needs_bundled_fallback = {
            let cat = self.inner.catalog.read();
            !cat.has_fetched_real_catalog && cat.prefetched.is_none()
        };
        if needs_bundled_fallback {
            if remote_fetch_enabled {
                xai_grok_telemetry::unified_log::warn(
                    "model catalog: falling back to bundled defaults only",
                    None,
                    Some(serde_json::json!({
                        "trigger": "on_auth_changed",
                        "had_real_catalog": false,
                    })),
                );
            } else {
                tracing::debug!("model catalog: bundled defaults in use (remote_fetch disabled)");
            }
            self.rebuild_bundled(&config);

            if remote_fetch_enabled {
                self.spawn_catalog_retry(remote_fetch_enabled);
            }
        }

        self.notify_models_updated();
    }

    fn notify_models_updated(&self) {
        let available = self.available();
        let current = self.current_model_id();
        let count = available.len();
        xai_grok_telemetry::unified_log::info(
            "model catalog: notifying clients",
            None,
            Some(serde_json::json!({
                "model_count": count,
                "current_model_id": current.0.as_ref(),
            })),
        );
        if let Some(ref gw) = *self.inner.gateway.read() {
            let model_state =
                acp::SessionModelState::new(current, available.values().cloned().collect());
            if let Ok(params) = serde_json::value::to_raw_value(&model_state) {
                gw.forward_fire_and_forget(acp::ExtNotification::new(
                    "x.ai/models/update",
                    params.into(),
                ));
            }
        }
    }

    /// Hot-reload the catalog from `~/.grok/models_cache.json` after an external write (config-watcher detected).
    pub(crate) fn reload_from_disk_cache(&self) {
        self.reload_from_cache_manager(&self.inner.cache);
    }

    /// Core of [`Self::reload_from_disk_cache`], parameterized over the cache
    fn reload_from_cache_manager(&self, cache: &ModelsCacheManager) {
        let fetch_auth = *self.inner.fetch_auth.read();
        let Some(cached) = cache.load_fresh(&fetch_auth.cache_auth_method(), &self.cache_origin())
        else {
            tracing::debug!("models cache changed on disk but is not loadable; ignoring");
            return;
        };

        let same_content = {
            let cat = self.inner.catalog.read();
            cat.prefetched.as_ref().is_some_and(|current| {
                serde_json::to_string(current).ok() == serde_json::to_string(&cached.models).ok()
            })
        };
        if same_content {
            if cached.etag.is_some() {
                self.inner.catalog.write().etag = cached.etag;
            }
            tracing::debug!("models cache changed on disk but catalog is identical; skipping");
            return;
        }

        let cfg = self.inner.cfg.read().clone();
        let count = cached.models.len();
        self.apply_catalog(&cfg, cached.models, cached.etag);
        tracing::info!(count, "model catalog hot-reloaded from disk cache");
        xai_grok_telemetry::unified_log::info(
            "model catalog: reloaded from external disk-cache write",
            None,
            Some(serde_json::json!({ "model_count": count })),
        );
        self.notify_models_updated();
    }

    /// Retry model catalog fetch in the background with exponential backoff.
    fn spawn_catalog_retry(&self, remote_fetch_enabled: bool) {
        self.spawn_catalog_retry_with_backoff(
            remote_fetch_enabled,
            crate::tools::retry::BackoffConfig::new(5, 5_000, 60_000),
        );
    }

    /// [`Self::spawn_catalog_retry`] with an injectable backoff (fast in tests).
    fn spawn_catalog_retry_with_backoff(
        &self,
        remote_fetch_enabled: bool,
        backoff: crate::tools::retry::BackoffConfig,
    ) {
        if !remote_fetch_enabled {
            return;
        }
        if self
            .inner
            .retry_in_flight
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            tracing::debug!("model catalog retry already in flight, skipping");
            return;
        }

        // The whole retry sequence is one attempt to waiters.
        let attempt = FetchAttemptGuard::begin(&self.inner);
        let mgr = self.clone();
        tokio::task::spawn(async move {
            let _attempt = attempt;
            let _retry_guard = RetryInFlightGuard(mgr.inner.clone());
            let result = crate::tools::retry::execute_with_backoff(
                &backoff,
                || {
                    let mgr = mgr.clone();
                    async move {
                        if mgr.inner.catalog.read().has_fetched_real_catalog {
                            return Ok(());
                        }

                        mgr.fetch_and_apply().await;

                        if mgr.inner.catalog.read().has_fetched_real_catalog {
                            Ok(())
                        } else {
                            Err("model catalog fetch returned no models")
                        }
                    }
                },
                |attempt, max_retries, delay| async move {
                    xai_grok_telemetry::unified_log::warn(
                        "model catalog: retry scheduled",
                        None,
                        Some(serde_json::json!({
                            "attempt": attempt,
                            "max_retries": max_retries,
                            "delay_ms": delay.as_millis() as u64,
                        })),
                    );
                },
            )
            .await;

            match result {
                Ok(()) => {
                    let count = mgr.available().len();
                    xai_grok_telemetry::unified_log::info(
                        "model catalog: retry succeeded",
                        None,
                        Some(serde_json::json!({ "model_count": count })),
                    );
                    mgr.notify_models_updated();
                }
                Err(e) => {
                    xai_grok_telemetry::unified_log::warn(
                        "model catalog: all retries exhausted",
                        None,
                        Some(serde_json::json!({ "error": e })),
                    );
                }
            }
        });
    }

    /// One-shot background catalog refresh after readiness; no-op when a fresh disk cache already loaded a real catalog.
    pub fn spawn_background_refresh(&self) {
        self.spawn_background_refresh_inner(crate::util::config::resolve_remote_fetch_enabled());
    }

    fn spawn_background_refresh_inner(&self, remote_fetch_enabled: bool) {
        if self.inner.catalog.read().has_fetched_real_catalog {
            tracing::debug!(
                "skipping startup background model refresh: fresh cache already loaded"
            );
            return;
        }
        self.spawn_catalog_retry(remote_fetch_enabled);
    }

    /// Refresh the model catalog on every auth token refresh.
    pub fn start_auth_refresh_watcher(&self, notify: Arc<tokio::sync::Notify>) {
        let mgr = self.clone();
        let had_catalog_at_start = self.inner.catalog.read().has_fetched_real_catalog;
        xai_grok_telemetry::unified_log::info(
            "model catalog: auth refresh watcher started",
            None,
            Some(serde_json::json!({
                "had_real_catalog": had_catalog_at_start,
                "model_count": self.available().len(),
            })),
        );
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                if !crate::util::config::resolve_remote_fetch_enabled() {
                    tracing::debug!(
                        "model catalog: auth refresh watcher skipped (remote_fetch disabled)"
                    );
                    continue;
                }
                let had_catalog = mgr.inner.catalog.read().has_fetched_real_catalog;
                let old_count = mgr.available().len();
                xai_grok_telemetry::unified_log::info(
                    "model catalog: auth refresh watcher triggered",
                    None,
                    Some(serde_json::json!({
                        "had_real_catalog": had_catalog,
                        "model_count_before": old_count,
                    })),
                );
                mgr.fetch_and_apply().await;
                let has_catalog = mgr.inner.catalog.read().has_fetched_real_catalog;
                let new_count = mgr.available().len();
                if has_catalog {
                    if !had_catalog || new_count != old_count {
                        xai_grok_telemetry::unified_log::info(
                            "model catalog: auth refresh watcher updated catalog",
                            None,
                            Some(serde_json::json!({
                                "model_count_before": old_count,
                                "model_count_after": new_count,
                                "was_recovery": !had_catalog,
                            })),
                        );
                    }
                    mgr.notify_models_updated();
                } else {
                    xai_grok_telemetry::unified_log::warn(
                        "model catalog: auth refresh watcher fetch failed",
                        None,
                        Some(serde_json::json!({
                            "model_count": old_count,
                        })),
                    );
                }
            }
        });
    }

    /// Wipe in-memory state so a previous identity's catalog doesn't leak.
    fn clear(&self) {
        {
            let mut cat = self.inner.catalog.write();
            let generation = cat.generation + 1;
            *cat = CatalogState::default();
            cat.generation = generation;
            self.inner
                .catalog_progress
                .send_replace(CatalogProgress::Pending);
        }
        // A new identity starts fresh: drop the prior user's pick so its
        // first catalog reselects that identity's default.
        self.inner
            .user_selected_model
            .store(false, Ordering::Relaxed);
    }

    /// Build a `SamplingConfig` from the current model + auth state.
    pub fn sampling_config(&self) -> SamplingConfig {
        let config = self.inner.cfg.read().clone();
        let auth_manager = self.inner.auth_manager.as_ref();
        let current_model_id = self.current_model_id();
        let all_models = self.models();
        let fallback;
        let current_model = match all_models
            .get(current_model_id.0.as_ref())
            .or_else(|| all_models.values().next())
        {
            Some(m) => m,
            None => {
                tracing::warn!("no models available in catalog; defaulting to bundled model");
                let default_id = crate::models::default_model().to_string();
                fallback = ModelEntry::fallback(&default_id, &config.endpoints);
                &fallback
            }
        };

        let session_auth = auth_manager.current_or_expired();
        let credentials =
            resolve_credentials(current_model, session_auth.as_ref().map(|a| a.key.as_str()));

        sampling_config_for_model(
            current_model,
            credentials,
            config.endpoints.alpha_test_key.clone(),
            config.client_version.clone(),
            crate::managed_config::resolve_deployment_id(
                config.endpoints.deployment_key.as_deref(),
            ),
            None,
        )
    }

    /// Disk-cache origin key for this manager's current endpoints/auth shape
    fn cache_origin(&self) -> String {
        let endpoints = self.inner.cfg.read().endpoints.clone();
        let fetch_auth = *self.inner.fetch_auth.read();
        crate::remote::models_list_url(&endpoints, fetch_auth)
    }

    /// A catalog-fetch session refresh bounded by `STARTUP_AUTH_REFRESH_TIMEOUT`.
    /// A hung IdP on a cold cache degrades to a session-less fetch (the
    /// bundled/cache catalog stays and the next refresh retries) instead of
    /// stalling boot, mirroring the readiness path's no-mint auth bound.
    async fn bounded_startup_auth(auth_manager: &Arc<AuthManager>) -> Option<GrokAuth> {
        Self::bounded_auth_refresh(async { auth_manager.auth().await.ok() }).await
    }

    /// Bounds an auth-refresh future to `STARTUP_AUTH_REFRESH_TIMEOUT`, yielding
    /// `None` on timeout. Split out so the timeout contract is unit-testable
    /// without a live IdP.
    async fn bounded_auth_refresh<F>(fut: F) -> Option<GrokAuth>
    where
        F: std::future::Future<Output = Option<GrokAuth>>,
    {
        match tokio::time::timeout(crate::http::STARTUP_AUTH_REFRESH_TIMEOUT, fut).await {
            Ok(auth) => auth,
            Err(_) => {
                tracing::warn!(
                    timeout_secs = crate::http::STARTUP_AUTH_REFRESH_TIMEOUT.as_secs(),
                    "model catalog: auth refresh timed out; fetching without a fresh session"
                );
                None
            }
        }
    }

    fn spawn_fetch(&self, new_etag: Option<String>) {
        self.spawn_fetch_inner(
            new_etag,
            crate::util::config::resolve_remote_fetch_enabled(),
        );
    }

    fn spawn_fetch_inner(&self, new_etag: Option<String>, remote_fetch_enabled: bool) {
        if !remote_fetch_enabled {
            tracing::info!("model catalog refresh skipped: remote_fetch disabled");
            return;
        }
        if self
            .inner
            .refresh_in_flight
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            tracing::debug!("model catalog refresh already in flight, skipping");
            return;
        }
        // Generation first: an identity change after this point fails the
        // apply fence instead of publishing an old-credential fetch.
        let attempt = FetchAttemptGuard::begin(&self.inner);
        let generation = attempt.generation;
        let cfg = self.inner.cfg.read().clone();
        let endpoints = cfg.endpoints.clone();
        let fetch_auth = *self.inner.fetch_auth.read();
        let auth_manager = self.inner.auth_manager.clone();
        let endpoint = self.inner.endpoint.clone();
        let mgr = self.clone();

        tokio::task::spawn(async move {
            let _attempt = attempt;
            let _refresh_guard = RefreshInFlightGuard(mgr.inner.clone());
            let auth = Self::bounded_startup_auth(&auth_manager).await;
            let new_prefetched = match tokio::time::timeout(
                crate::http::STARTUP_FETCH_TIMEOUT,
                endpoint.fetch_models(endpoints, auth, fetch_auth),
            )
            .await
            {
                Ok(models) => models,
                Err(_) => {
                    tracing::warn!("etag-triggered model refresh timed out");
                    None
                }
            };
            if !mgr.apply_refresh_result_fenced(&cfg, new_prefetched, new_etag, generation) {
                return;
            }
            tracing::info!("models manager refreshed");
            mgr.notify_models_updated();
        });
    }

    async fn fetch_and_apply(&self) {
        self.fetch_and_apply_inner(crate::util::config::resolve_remote_fetch_enabled())
            .await
    }

    async fn fetch_and_apply_inner(&self, remote_fetch_enabled: bool) {
        if !remote_fetch_enabled {
            tracing::info!("model catalog refresh skipped: remote_fetch disabled");
            return;
        }
        let attempt = FetchAttemptGuard::begin(&self.inner);
        let generation = attempt.generation;
        let auth = Self::bounded_startup_auth(&self.inner.auth_manager).await;
        let has_auth = auth.is_some();
        let fetch_auth = *self.inner.fetch_auth.read();
        let cfg = self.inner.cfg.read().clone();
        xai_grok_telemetry::unified_log::info(
            "model catalog: fetching",
            None,
            Some(serde_json::json!({
                "has_auth": has_auth,
                "fetch_auth": format!("{fetch_auth:?}"),
            })),
        );
        let endpoint = self.inner.endpoint.clone();
        let new_prefetched = match tokio::time::timeout(
            crate::http::STARTUP_FETCH_TIMEOUT,
            endpoint.fetch_models(cfg.endpoints.clone(), auth, fetch_auth),
        )
        .await
        {
            Ok(res) => res,
            Err(_elapsed) => {
                tracing::warn!(
                    timeout_secs = crate::http::STARTUP_FETCH_TIMEOUT.as_secs(),
                    "model catalog fetch timed out"
                );
                None
            }
        };
        let success = self.apply_refresh_result_fenced(&cfg, new_prefetched, None, generation);
        if success {
            xai_grok_telemetry::unified_log::info(
                "model catalog: fetch succeeded",
                None,
                Some(serde_json::json!({
                    "model_count": self.available().len(),
                })),
            );
        }
    }

    /// Publish a resolved catalog under one atomic write, then reselect the model (default on first real catalog, else keep current if present).
    fn apply_catalog(
        &self,
        cfg: &config::Config,
        models: IndexMap<String, ModelEntry>,
        new_etag: Option<String>,
    ) {
        let _ = self.apply_catalog_fenced(cfg, models, new_etag, None);
    }

    /// Discards a result captured before an identity change; returns
    /// whether the catalog applied.
    fn apply_catalog_fenced(
        &self,
        cfg: &config::Config,
        models: IndexMap<String, ModelEntry>,
        new_etag: Option<String>,
        generation: Option<u64>,
    ) -> bool {
        let (first_real_catalog, excludes_all) = {
            let mut cat = self.inner.catalog.write();
            if let Some(generation) = generation
                && cat.generation != generation
            {
                tracing::info!("model catalog result discarded: identity changed during fetch");
                return false;
            }
            let first_real_catalog = !cat.has_fetched_real_catalog;
            cat.has_fetched_real_catalog = true;
            cat.prefetched = Some(models);
            cat.models = resolve_model_catalog(cfg, cat.prefetched.clone());
            cat.etag = new_etag;
            cat.allowlist_excludes_all = allowlist_matches_nothing(cfg, &cat.models);
            // In the lock: the flag and its mirror can't desync vs `clear()`.
            self.inner
                .catalog_progress
                .send_replace(CatalogProgress::Ready);
            (first_real_catalog, cat.allowlist_excludes_all)
        };
        if excludes_all {
            tracing::error!("allowed_models excludes all fetched models; prompts will be blocked");
        }

        // Respect an explicit pre-catalog `/model` pick: auto-select the
        // default on the first catalog only when the user hasn't chosen.
        // Either way a now-invalid selection is replaced.
        if first_real_catalog && !self.inner.user_selected_model.load(Ordering::Relaxed) {
            self.reselect_default_model(cfg);
        } else {
            self.reselect_current_model_if_missing(cfg);
        }
        true
    }

    /// A same-identity refresh, as the fetch paths see it.
    #[cfg(test)]
    fn apply_refresh_result(
        &self,
        config: &config::Config,
        new_prefetched: Option<IndexMap<String, ModelEntry>>,
        new_etag: Option<String>,
    ) -> bool {
        let generation = self.inner.catalog.read().generation;
        self.apply_refresh_result_fenced(config, new_prefetched, new_etag, generation)
    }

    fn apply_refresh_result_fenced(
        &self,
        config: &config::Config,
        new_prefetched: Option<IndexMap<String, ModelEntry>>,
        new_etag: Option<String>,
        generation: u64,
    ) -> bool {
        let Some(new_prefetched) = new_prefetched else {
            tracing::warn!("model refresh failed, leaving existing models unchanged");
            // Lock held across the send: atomic against a racing `clear()`.
            {
                let cat = self.inner.catalog.read();
                if cat.generation == generation {
                    self.inner.catalog_progress.send_if_modified(|p| {
                        let first_failure = *p == CatalogProgress::Pending;
                        if first_failure {
                            *p = CatalogProgress::Failed;
                        }
                        first_failure
                    });
                }
            }
            xai_grok_telemetry::unified_log::warn(
                "model catalog refresh failed",
                None,
                Some(serde_json::json!({
                    "had_real_catalog": self.inner.catalog.read().has_fetched_real_catalog,
                })),
            );
            return false;
        };
        self.apply_catalog_fenced(config, new_prefetched, new_etag, Some(generation))
    }

    pub fn allowlist_excludes_all(&self) -> bool {
        self.inner.catalog.read().allowlist_excludes_all
    }

    /// Re-pick the default when the current model is gone or unselectable.
    ///
    /// First-party auth-visibility (OAuth-only vs API-key) never evicts an
    /// explicit user pick — the user may switch auth modes and keep the model.
    /// Managed platform entries without credentials are completely unusable,
    /// so those are always reselected even after an explicit pick.
    fn reselect_current_model_if_missing(&self, config: &config::Config) {
        let current = self.inner.current_model_id.read().clone();
        let user_selected = self.inner.user_selected_model.load(Ordering::Relaxed);
        let needs_reselection = {
            let cat = self.inner.catalog.read();
            let models = &cat.models;
            match models.get(current.0.as_ref()) {
                None => true,
                Some(entry) => {
                    if !entry.info.user_selectable {
                        true
                    } else if !entry.visible_for_auth(self.is_session_auth()) {
                        // Unusable under current auth/credentials.
                        // Keep first-party user picks; always evict locked platforms.
                        !user_selected || entry.is_managed_platform_model()
                    } else {
                        false
                    }
                }
            }
        };
        if !needs_reselection {
            return;
        }
        let (key, _, source) = {
            let cat = self.inner.catalog.read();
            let models = &cat.models;
            resolve_default_model(config, models, self.is_session_auth())
        };
        let new_id = acp::ModelId::new(Arc::from(key));
        tracing::info!(
            old = %current.0, new = %new_id.0, source = %source,
            "current model unavailable in new catalog, reselecting default"
        );
        self.set_current_model_id_internal(new_id);
    }

    /// Re-resolve the default model against the current catalog.
    fn reselect_default_model(&self, config: &config::Config) {
        let (key, _, source) = {
            let cat = self.inner.catalog.read();
            let models = &cat.models;
            resolve_default_model(config, models, self.is_session_auth())
        };
        let new_id = acp::ModelId::new(Arc::from(key));
        let current = self.inner.current_model_id.read().clone();
        if current.0.as_ref() != new_id.0.as_ref() {
            tracing::info!(
                old = %current.0, new = %new_id.0, source = %source,
                "re-resolved default model after catalog populated"
            );
            self.set_current_model_id_internal(new_id);
        }
    }
}

mod cache;
mod endpoint;
mod fetch;
mod resolution;

pub(crate) use cache::*;
pub(crate) use endpoint::*;
pub(crate) use fetch::*;
pub use fetch::{
    EarlyPrefetchHandle, EarlyPrefetchResult, start_early_prefetch,
    start_early_prefetch_settings_only, start_early_prefetch_with_auth,
};
pub(crate) use resolution::*;

#[cfg(test)]
mod tests;
