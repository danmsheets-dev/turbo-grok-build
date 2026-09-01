//! Built-in third-party platform registry.
//!
//! Phase 1: Moonshot open platforms (API key).
//! Phase 2: Kimi Code subscription (device OAuth).
//! Phase 3: OpenAI + Anthropic (API key; catalog from Pi models.generated).
//! Phase 4: OpenCode Go subscription (Console API key; mixed Chat/Messages catalog).

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::LazyLock;

use crate::provider_compat::{
    AnthropicMessagesCompat, MaxTokensField, OpenAiCompletionsCompat, OpenAiResponsesCompat,
    ProviderRouteSpec, RequestCompat, RouteAuth, ThinkingFormat,
};

/// Env var for the moonshot.cn API key (wins over the generic name).
pub const MOONSHOT_CN_API_KEY_ENV: &str = "GROK_MOONSHOT_CN_API_KEY";
/// Env var for the moonshot.ai API key (wins over the generic name).
pub const MOONSHOT_AI_API_KEY_ENV: &str = "GROK_MOONSHOT_AI_API_KEY";
/// Generic Moonshot API key applied to both open platforms when the
/// platform-scoped name is unset. Also accepts the common `MOONSHOT_API_KEY`
/// alias used by Moonshot docs.
pub const MOONSHOT_API_KEY_ENV: &str = "GROK_MOONSHOT_API_KEY";
/// Common third-party alias (Moonshot open-platform docs).
pub const MOONSHOT_API_KEY_ALIAS_ENV: &str = "MOONSHOT_API_KEY";

/// Env overrides for Moonshot base URLs (dev/test only).
pub const MOONSHOT_CN_BASE_URL_ENV: &str = "GROK_MOONSHOT_CN_BASE_URL";
pub const MOONSHOT_AI_BASE_URL_ENV: &str = "GROK_MOONSHOT_AI_BASE_URL";

/// Env override for the Kimi Code subscription inference base.
pub const KIMI_CODE_BASE_URL_ENV: &str = "GROK_KIMI_CODE_BASE_URL";
/// Kimi Code API key (provider-scoped, highest static credential precedence).
pub const KIMI_CODE_API_KEY_ENV: &str = "GROK_KIMI_CODE_API_KEY";
/// Shared Grok-prefixed Kimi API-key alias.
pub const KIMI_API_KEY_ENV: &str = "GROK_KIMI_API_KEY";
/// Official Pi/Kimi API-key alias.
pub const KIMI_API_KEY_ALIAS_ENV: &str = "KIMI_API_KEY";
/// Env override for the Kimi Code wire backend (`messages` default;
/// `chat_completions` opts into the OpenAI-compatible endpoint while we
/// validate parity — gray-release switch).
pub const KIMI_CODE_API_BACKEND_ENV: &str = "GROK_KIMI_CODE_API_BACKEND";
/// Env override for the Kimi Code OAuth host.
pub const KIMI_CODE_OAUTH_HOST_ENV: &str = "GROK_KIMI_CODE_OAUTH_HOST";

/// Env override for the OpenAI Codex (ChatGPT subscription) inference base.
pub const OPENAI_CODEX_BASE_URL_ENV: &str = "GROK_OPENAI_CODEX_BASE_URL";
/// Env override for the OpenAI Codex OAuth host.
pub const OPENAI_CODEX_OAUTH_HOST_ENV: &str = "GROK_OPENAI_CODEX_OAUTH_HOST";

/// OpenCode Go subscription API key (platform-scoped, wins over the official alias).
pub const OPENCODE_GO_API_KEY_ENV: &str = "GROK_OPENCODE_GO_API_KEY";
/// Official OpenCode Go / Zen API key alias.
pub const OPENCODE_API_KEY_ENV: &str = "OPENCODE_API_KEY";
/// OpenCode Go OpenAI/Anthropic-compatible gateway base.
pub const OPENCODE_GO_BASE_URL_DEFAULT: &str = "https://opencode.ai/zen/go/v1";

/// OpenAI API key (platform-scoped, wins over `OPENAI_API_KEY`).
pub const OPENAI_API_KEY_ENV: &str = "GROK_OPENAI_API_KEY";
/// Common OpenAI SDK alias.
pub const OPENAI_API_KEY_ALIAS_ENV: &str = "OPENAI_API_KEY";
pub const OPENAI_BASE_URL_ENV: &str = "GROK_OPENAI_BASE_URL";

/// Anthropic API key (platform-scoped).
pub const ANTHROPIC_API_KEY_ENV: &str = "GROK_ANTHROPIC_API_KEY";
/// Common Anthropic aliases used by Claude Code / Pi.
pub const ANTHROPIC_API_KEY_ALIAS_ENV: &str = "ANTHROPIC_API_KEY";
pub const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
/// Grok-native base URL override. This already includes the API version path
/// because Grok appends only `/messages`.
pub const ANTHROPIC_BASE_URL_ENV: &str = "GROK_ANTHROPIC_BASE_URL";
/// Claude Code / Anthropic SDK base URL override. SDK-style values name the
/// gateway root and need `/v1` before Grok appends `/messages`.
pub const ANTHROPIC_BASE_URL_ALIAS_ENV: &str = "ANTHROPIC_BASE_URL";

const MOONSHOT_CN_BASE_URL_DEFAULT: &str = "https://api.moonshot.cn/v1";
const MOONSHOT_AI_BASE_URL_DEFAULT: &str = "https://api.moonshot.ai/v1";
/// Kimi Code subscription base for Grok's HTTP client.
///
/// Official Pi stores `https://api.kimi.com/coding` and lets the Anthropic SDK
/// append `/v1/messages`. Grok's sampler joins `{base}/messages`, so the base
/// must include `/v1` (same pattern as Anthropic's `…/v1`). Override with
/// `GROK_KIMI_CODE_BASE_URL`.
const KIMI_CODE_BASE_URL_DEFAULT: &str = "https://api.kimi.com/coding/v1";
const KIMI_CODE_OAUTH_HOST_DEFAULT: &str = "https://auth.kimi.com";
/// OpenAI Codex (ChatGPT subscription) inference base. Grok's sampler joins
/// `{base}/responses`, producing the Codex backend SSE endpoint
/// `https://chatgpt.com/backend-api/codex/responses` (same as official Pi).
const OPENAI_CODEX_BASE_URL_DEFAULT: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_CODEX_OAUTH_HOST_DEFAULT: &str = "https://auth.openai.com";
const OPENAI_BASE_URL_DEFAULT: &str = "https://api.openai.com/v1";
const ANTHROPIC_BASE_URL_DEFAULT: &str = "https://api.anthropic.com/v1";
/// Nexus gateway root (Claude-Code-style base). OpenAI clients use `{root}/openai`,
/// Anthropic/Responses clients use `{root}/v1`. Override with `GROK_NEXUS_BASE_URL`.
/// The `/providers nexus <key> [base_url]` login can also persist a per-account root.
pub const NEXUS_BASE_URL_DEFAULT: &str = "https://nexuscore.now";
/// Required Anthropic Messages API version header (also sent for Kimi Code).
pub const ANTHROPIC_VERSION_HEADER_VALUE: &str = "2023-06-01";

fn env_or(var: &str, compiled: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => compiled.to_string(),
    }
}

/// Grok's sampler joins `{base}/messages`. Official Pi stores
/// `https://api.kimi.com/coding` and lets the Anthropic SDK append `/v1/messages`.
/// Accept both shapes so `GROK_KIMI_CODE_BASE_URL=…/coding` does not 404 as
/// `…/coding/messages` (`resource_not_found_error`).
pub fn normalize_kimi_code_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return KIMI_CODE_BASE_URL_DEFAULT.to_string();
    }
    // Pi / Anthropic-SDK style base ends at `/coding` — add `/v1` for Grok.
    if trimmed.ends_with("/coding") {
        return format!("{trimmed}/v1");
    }
    trimmed.to_string()
}

/// Normalize a user-supplied Nexus base into the bare gateway root `R`.
///
/// Accepts any client-facing shape (`https://nexuscore.now`,
/// `…/openai`, `…/openai/v1`, `…/v1`, trailing slash) and strips the
/// client-view suffix so per-backend bases can be re-derived. Empty → default.
pub fn nexus_normalize_root(raw: &str) -> String {
    let mut t = raw.trim().trim_end_matches('/');
    // Strip the longest client-view suffix first (`/openai/v1` before `/openai`).
    for suffix in ["/openai/v1", "/openai", "/v1"] {
        if let Some(base) = t.strip_suffix(suffix) {
            t = base.trim_end_matches('/');
            break;
        }
    }
    if t.is_empty() {
        NEXUS_BASE_URL_DEFAULT.to_string()
    } else {
        t.to_string()
    }
}

/// Nexus OpenAI chat/completions base: `{R}/openai/v1` → `…/chat/completions`.
pub fn nexus_chat_base(root: &str) -> String {
    format!("{}/openai/v1", root.trim_end_matches('/'))
}

/// Nexus Claude Messages base: `{R}/v1` → sampler joins `{base}/messages`.
pub fn nexus_messages_base(root: &str) -> String {
    format!("{}/v1", root.trim_end_matches('/'))
}

/// Nexus Responses base: `{R}/v1` → `…/responses`.
pub fn nexus_responses_base(root: &str) -> String {
    format!("{}/v1", root.trim_end_matches('/'))
}

/// Convert Claude Code / Anthropic SDK base URLs into Grok's base shape.
///
/// Claude Code treats `ANTHROPIC_BASE_URL` as a gateway root and appends
/// `/v1/messages`. Grok appends only `/messages`, so the equivalent Grok base
/// must end in `/v1`. Already-versioned values are accepted defensively, as is
/// a mistakenly supplied full `/v1/messages` endpoint.
fn normalize_anthropic_sdk_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return ANTHROPIC_BASE_URL_DEFAULT.to_string();
    }
    normalize_messages_sdk_base_url(trimmed)
}

/// Normalize any Anthropic-compatible Messages gateway root so Grok can append
/// `/messages` (producing `…/v1/messages`).
///
/// Used for catalog `base_url_override` values such as MiniMax
/// (`…/anthropic`) and Fireworks (`…/inference`) as well as Anthropic SDK
/// roots. Already-versioned bases and full `/v1/messages` endpoints are left
/// in the Grok shape (`…/v1`).
pub fn normalize_messages_sdk_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    if let Some(base) = trimmed.strip_suffix("/messages")
        && base.ends_with("/v1")
    {
        return base.to_string();
    }
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

/// Normalize Pi-supported Azure endpoint shapes to the Responses SDK base.
pub fn normalize_azure_openai_base_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let mut parsed = url::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }

    let host = parsed.host_str()?.to_ascii_lowercase();
    let is_azure_host = host.ends_with(".openai.azure.com")
        || host.ends_with(".cognitiveservices.azure.com")
        || host.ends_with(".ai.azure.com");
    let normalized_path = parsed.path().trim_end_matches('/');
    if is_azure_host && matches!(normalized_path, "" | "/openai" | "/openai/v1/responses") {
        parsed.set_path("/openai/v1");
        parsed.set_query(None);
    }
    Some(parsed.to_string().trim_end_matches('/').to_string())
}

fn normalize_runtime_base_url(
    url: &str,
    normalization: ProviderBaseUrlNormalization,
) -> Option<String> {
    if matches!(normalization, ProviderBaseUrlNormalization::AzureOpenAi) {
        return normalize_azure_openai_base_url(url);
    }
    let trimmed = url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn expand_base_url_placeholders(
    value: &str,
    allowed_env_keys: &[String],
    getenv: &mut impl FnMut(&str) -> Option<String>,
) -> (String, bool) {
    let Ok(names) = base_url_template_env_names(value) else {
        return (value.to_string(), false);
    };
    let allowed: std::collections::BTreeSet<&str> =
        allowed_env_keys.iter().map(String::as_str).collect();
    let mut output = value.to_string();
    for name in names {
        if !allowed.contains(name.as_str()) {
            return (output, false);
        }
        let Some(raw) = getenv(&name) else {
            return (output, false);
        };
        let replacement = raw.trim();
        if replacement.is_empty()
            || !replacement
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return (output, false);
        }
        output = output.replace(&format!("{{{name}}}"), replacement);
    }
    let ready = !output.contains(['{', '}']);
    (output, ready)
}

fn base_url_template_env_names(value: &str) -> Result<Vec<String>, ()> {
    let bytes = value.as_bytes();
    let mut names = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'}' => return Err(()),
            b'{' => {
                let relative_end = bytes[cursor + 1..]
                    .iter()
                    .position(|byte| *byte == b'}')
                    .ok_or(())?;
                let end = cursor + 1 + relative_end;
                let name = &value[cursor + 1..end];
                if !valid_env_key(name) || name.contains('{') {
                    return Err(());
                }
                names.push(name.to_string());
                cursor = end + 1;
            }
            _ => cursor += 1,
        }
    }
    Ok(names)
}

fn valid_env_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn deployment_name_from_pi_map(value: &str, model_id: &str) -> Option<String> {
    let mut resolved = None;
    for entry in value.split(',') {
        let Some((candidate, deployment)) = entry.trim().split_once('=') else {
            continue;
        };
        let candidate = candidate.trim();
        let deployment = deployment.trim();
        if candidate == model_id && !deployment.is_empty() {
            resolved = Some(deployment.to_string());
        }
    }
    resolved
}

/// Embedded provider registry. The registry is parsed and cross-validated with
/// [`PLATFORM_CATALOG_JSON`] before either asset is exposed to callers.
pub const PLATFORM_REGISTRY_JSON: &str = include_str!("../platform_registry.json");

const PLATFORM_REGISTRY_VERSION: u32 = 2;
const PLATFORM_CATALOG_VERSION: u32 = 3;

/// Canonical, data-driven provider identifier.
///
/// [`PlatformId`] remains the compatibility enum for provider-specific runtime
/// behavior. New generated providers should enter through this string-backed
/// identifier and [`ProviderSpec`] rather than growing that enum indefinitely.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve a canonical id or registered alias to its canonical id.
    pub fn registered(value: &str) -> Option<Self> {
        provider_spec(value).map(|spec| spec.id.clone())
    }

    /// Return the legacy typed platform when this provider needs the existing
    /// compatibility path.
    pub fn platform_id(&self) -> Option<PlatformId> {
        PlatformId::parse(self.as_str()).filter(|platform| platform.as_str() == self.as_str())
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for ProviderId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Upstream source responsible for a provider's generated catalog metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCatalogSource {
    Pi,
    ModelsDev,
    Hyper,
}

/// Whether a provider is ready for users or reserved for a later native
/// adapter wave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Active,
    Planned,
}

/// Provider-level adapter selection. `Standard` dispatches from each catalog
/// row's wire protocol; the remaining variants preserve provider-specific
/// behavior behind a small typed boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    #[default]
    Standard,
    KimiCoding,
    #[serde(rename = "openai_codex")]
    OpenAiCodex,
    MistralConversations,
    Nexus,
    AnthropicClaude,
    #[serde(rename = "github_copilot")]
    GitHubCopilot,
    GoogleGenerateContent,
    BedrockConverseStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialKind {
    ApiKey,
    Oauth,
    Hybrid,
}

/// Default credential placement. `ProtocolDefault` means Bearer for OpenAI
/// Chat/Responses and `x-api-key` for Anthropic Messages; a route may override
/// this in later adapter metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthPlacement {
    ProtocolDefault,
    Bearer,
    XApiKey,
    /// Azure OpenAI's raw `api-key` header.
    ApiKey,
    /// Cloudflare AI Gateway's `cf-aig-authorization: Bearer …` header.
    CfAigAuthorization,
    XGoogApiKey,
}

/// Provider-specific runtime materialization layered on a standard wire API.
///
/// This metadata contains environment variable *names* only. Secret values are
/// resolved by the shell/sampler credential path and are never stored here.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderRuntimeSpec {
    /// Environment names allowed in `{ENV_NAME}` base-URL placeholders.
    pub base_url_template_env_keys: Vec<String>,
    /// Static route query key -> environment override name.
    pub query_params_from_env: BTreeMap<String, String>,
    /// Pi-compatible `model=deployment,...` mapping for the wire model id.
    pub model_id_map_env_key: Option<String>,
    /// Optional normalization applied after URL placeholders are expanded.
    pub base_url_normalization: ProviderBaseUrlNormalization,
    /// Environment names whose presence can make this provider usable without a static API key.
    pub external_readiness_env_keys: Vec<String>,
    /// Required project environment names for native Google Vertex routes.
    pub project_env_keys: Vec<String>,
    /// Required location environment names for native Google Vertex routes.
    pub location_env_keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderBaseUrlNormalization {
    #[default]
    None,
    AzureOpenAi,
}

/// Fully materialized non-secret runtime route for one catalog model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderRuntime {
    pub base_url: String,
    pub query_params: BTreeMap<String, String>,
    pub wire_model_id: String,
    /// False when a URL placeholder is unresolved/unsafe or the final URL is
    /// not an absolute HTTP(S) base. Callers must keep the model locked.
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredentialPolicy {
    pub kind: ProviderCredentialKind,
    pub env_keys: Vec<String>,
    pub auth: ProviderAuthPlacement,
    /// Canonical persisted/config credential family. Providers in the same
    /// family may expose different routes while accepting one official key.
    #[serde(default)]
    pub storage_group: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDiscoveryMode {
    Disabled,
    ModelsEndpoint,
    Adapter,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscovery {
    pub mode: ProviderDiscoveryMode,
    pub models_path: Option<String>,
    #[serde(default)]
    pub model_prefixes: Vec<String>,
}

/// One provider registry row. No secret values are stored in this structure;
/// `env_keys` contains names only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSpec {
    pub id: ProviderId,
    pub pi_id: Option<String>,
    pub catalog_source: ProviderCatalogSource,
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub status: ProviderStatus,
    pub adapter: AdapterKind,
    pub default_base_url: String,
    #[serde(default)]
    pub base_url_env_keys: Vec<String>,
    pub credentials: ProviderCredentialPolicy,
    #[serde(default)]
    pub runtime: ProviderRuntimeSpec,
    pub discovery: ProviderDiscovery,
}

impl ProviderSpec {
    pub fn matches(&self, value: &str) -> bool {
        self.id.as_str() == value || self.aliases.iter().any(|alias| alias == value)
    }

    pub fn as_str(&self) -> &str {
        self.id.as_str()
    }

    pub fn legacy_platform(&self) -> Option<PlatformId> {
        self.id.platform_id()
    }

    pub fn uses_oauth(&self) -> bool {
        matches!(
            self.credentials.kind,
            ProviderCredentialKind::Oauth | ProviderCredentialKind::Hybrid
        )
    }

    pub fn accepts_api_key(&self) -> bool {
        matches!(
            self.credentials.kind,
            ProviderCredentialKind::ApiKey | ProviderCredentialKind::Hybrid
        )
    }

    pub fn uses_x_api_key(&self) -> bool {
        self.credentials.auth == ProviderAuthPlacement::XApiKey
    }

    pub fn credential_storage_group(&self) -> &str {
        self.credentials
            .storage_group
            .as_deref()
            .unwrap_or_else(|| self.id.as_str())
    }

    /// Resolve this provider's base URL without ever reading credential values.
    pub fn base_url(&self) -> String {
        if let Some(platform) = self.legacy_platform() {
            return platform.base_url();
        }
        for name in &self.base_url_env_keys {
            if let Ok(value) = std::env::var(name)
                && !value.trim().is_empty()
            {
                return value;
            }
        }
        self.default_base_url.clone()
    }

    /// Resolve URL placeholders, environment-backed query overrides, Azure
    /// normalization, and an optional Pi deployment map for one model route.
    pub fn resolve_runtime(
        &self,
        base_url: &str,
        model_id: &str,
        query_params: &BTreeMap<String, String>,
    ) -> ResolvedProviderRuntime {
        self.resolve_runtime_with(base_url, model_id, query_params, |name| {
            std::env::var(name).ok()
        })
    }

    /// Testable core of [`Self::resolve_runtime`] with an injected environment.
    pub fn resolve_runtime_with(
        &self,
        base_url: &str,
        model_id: &str,
        query_params: &BTreeMap<String, String>,
        mut getenv: impl FnMut(&str) -> Option<String>,
    ) -> ResolvedProviderRuntime {
        let (expanded_base_url, placeholders_ready) = expand_base_url_placeholders(
            base_url,
            &self.runtime.base_url_template_env_keys,
            &mut getenv,
        );
        let normalized_base_url =
            normalize_runtime_base_url(&expanded_base_url, self.runtime.base_url_normalization);
        let mut ready = placeholders_ready && normalized_base_url.is_some();

        let mut resolved_query = query_params.clone();
        for (query_key, env_key) in &self.runtime.query_params_from_env {
            let Some(value) = getenv(env_key) else {
                continue;
            };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if value.contains(['\r', '\n']) {
                ready = false;
                continue;
            }
            resolved_query.insert(query_key.clone(), value.to_string());
        }

        let wire_model_id = self
            .runtime
            .model_id_map_env_key
            .as_deref()
            .and_then(&mut getenv)
            .and_then(|mapping| deployment_name_from_pi_map(&mapping, model_id))
            .unwrap_or_else(|| model_id.to_string());

        ResolvedProviderRuntime {
            base_url: normalized_base_url.unwrap_or(expanded_base_url),
            query_params: resolved_query,
            wire_model_id,
            ready,
        }
    }

    pub fn base_url_matches(&self, url: &str) -> bool {
        urls_same_origin(&self.base_url(), url)
    }

    pub fn managed_model_key(&self, model_id: &str) -> String {
        format!("{}/{model_id}", self.id)
    }

    /// Human setup instructions that contain names only, never secret values.
    pub fn setup_hint(&self) -> String {
        if let Some(platform) = self.legacy_platform() {
            return platform.setup_hint();
        }
        if self.uses_oauth() {
            return format!(
                "Sign in with your {} subscription: run /login",
                self.display_name
            );
        }
        let env_part = match self.credentials.env_keys.as_slice() {
            [] => String::new(),
            [one] => format!("export {one}=<key>"),
            [first, rest @ ..] => {
                format!("export {first}=<key> (or {})", rest.join(" / "))
            }
        };
        let ui_part = format!("run /providers {} <api_key>", self.id);
        let config_part = format!(
            "add `api_key = \"<key>\"` under `[platforms.{}]` in ~/.grok/config.toml",
            self.id
        );
        let mut hint = if env_part.is_empty() {
            format!("{ui_part}, or {config_part}")
        } else {
            format!("{ui_part}, or {env_part}, or {config_part}")
        };
        if !self.runtime.base_url_template_env_keys.is_empty() {
            hint.push_str(&format!(
                "; also set {} for the endpoint template, or set one of {} to a complete base URL",
                self.runtime.base_url_template_env_keys.join(" and "),
                self.base_url_env_keys.join(" / ")
            ));
        }
        hint
    }
}

/// Parsed provider registry. Access it through [`provider_registry`] so the
/// embedded catalog and compatibility enum are validated first.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRegistry {
    version: u32,
    source: String,
    providers: Vec<ProviderSpec>,
}

impl ProviderRegistry {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn providers(&self) -> &[ProviderSpec] {
        &self.providers
    }

    pub fn find(&self, id_or_alias: &str) -> Option<&ProviderSpec> {
        self.providers.iter().find(|spec| spec.matches(id_or_alias))
    }
}

/// All validation failures found in an embedded or generated provider asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAssetError {
    issues: Vec<String>,
}

impl ProviderAssetError {
    fn new(issues: Vec<String>) -> Self {
        debug_assert!(!issues.is_empty());
        Self { issues }
    }

    pub fn issues(&self) -> &[String] {
        &self.issues
    }
}

impl fmt::Display for ProviderAssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} provider asset validation error(s):",
            self.issues.len()
        )?;
        for issue in &self.issues {
            writeln!(f, "- {issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderAssetError {}

/// Built-in inference platforms (aligned with official Pi `@earendil-works/pi-ai`
/// provider ids where applicable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlatformId {
    KimiCode,
    /// OpenAI Codex subscription via ChatGPT OAuth (`chatgpt.com/backend-api`).
    OpenAiCodex,
    /// OpenCode Go subscription via a Console-issued API key. The gateway
    /// exposes both OpenAI Chat Completions and Anthropic Messages models.
    OpenCodeGo,
    MoonshotCn,
    MoonshotAi,
    OpenAi,
    Anthropic,
    DeepSeek,
    Groq,
    /// Mistral API key provider using Pi's Mistral Chat Completions dialect.
    Mistral,
    XaiDirect,
    Together,
    Fireworks,
    Cerebras,
    Nvidia,
    OpenRouter,
    /// Poolside-hosted OpenAI-compatible Chat Completions (`inference.poolside.ai`).
    Poolside,
    MiniMax,
    MiniMaxCn,
    Zai,
    /// International Z.AI Coding Plan (`api.z.ai` coding/paas endpoint).
    ZaiCoding,
    /// China Z.AI / BigModel Coding Plan (`open.bigmodel.cn` coding/paas).
    ZaiCodingCn,
    Ollama,
    /// Nexus gateway (OpenAI/Anthropic-compatible BYOK; Bearer API key).
    /// Root `https://nexuscore.now`; chat/completions, Claude Messages and
    /// Responses all speak Bearer. No X/xAI membership gate (ApiKey auth).
    Nexus,
    /// Anthropic Claude subscription (Pro/Max) via browser OAuth. Bearer token
    /// + `anthropic-beta: oauth-2025-04-20` against Anthropic Messages. Distinct
    /// from `Anthropic` (which is x-api-key BYOK).
    AnthropicClaude,
}

impl PlatformId {
    /// All platforms; subscription first.
    pub const ALL: [PlatformId; 25] = [
        Self::KimiCode,
        Self::OpenAiCodex,
        Self::OpenCodeGo,
        Self::MoonshotCn,
        Self::MoonshotAi,
        Self::OpenAi,
        Self::Anthropic,
        Self::DeepSeek,
        Self::Groq,
        Self::Mistral,
        Self::XaiDirect,
        Self::Together,
        Self::Fireworks,
        Self::Cerebras,
        Self::Nvidia,
        Self::OpenRouter,
        Self::Poolside,
        Self::MiniMax,
        Self::MiniMaxCn,
        Self::Zai,
        Self::ZaiCoding,
        Self::ZaiCodingCn,
        Self::Ollama,
        Self::Nexus,
        Self::AnthropicClaude,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::KimiCode => "kimi-code",
            Self::OpenAiCodex => "openai-codex",
            Self::OpenCodeGo => "opencode-go",
            Self::MoonshotCn => "moonshot-cn",
            Self::MoonshotAi => "moonshot-ai",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
            Self::Groq => "groq",
            Self::Mistral => "mistral",
            Self::XaiDirect => "xai-direct",
            Self::Together => "together",
            Self::Fireworks => "fireworks",
            Self::Cerebras => "cerebras",
            Self::Nvidia => "nvidia",
            Self::OpenRouter => "openrouter",
            Self::Poolside => "poolside",
            Self::MiniMax => "minimax",
            Self::MiniMaxCn => "minimax-cn",
            Self::Zai => "zai",
            Self::ZaiCoding => "zai-coding",
            Self::ZaiCodingCn => "zai-coding-cn",
            Self::Ollama => "ollama",
            Self::Nexus => "nexus",
            Self::AnthropicClaude => "anthropic-claude",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|platform| platform.as_str() == s || platform.aliases().contains(&s))
    }

    /// Accepted non-canonical spellings. Kept explicit while special-provider
    /// runtime branches still use this enum; generated providers use
    /// [`ProviderSpec::aliases`] instead.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::KimiCode => &["kimi-coding"],
            Self::OpenAiCodex => &["chatgpt-codex"],
            Self::OpenCodeGo => &["opencodego"],
            Self::MoonshotCn => &["moonshotai-cn"],
            Self::MoonshotAi => &["moonshotai"],
            Self::XaiDirect => &["xai"],
            Self::ZaiCoding => &["zai-code-plan"],
            Self::AnthropicClaude => &["claude", "claude-code"],
            _ => &[],
        }
    }

    pub fn provider_id(self) -> ProviderId {
        ProviderId(self.as_str().to_string())
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::KimiCode => "Kimi For Coding",
            Self::OpenAiCodex => "OpenAI Codex (ChatGPT)",
            Self::OpenCodeGo => "OpenCode Go",
            Self::MoonshotCn => "Moonshot AI (moonshot.cn)",
            Self::MoonshotAi => "Moonshot AI (moonshot.ai)",
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
            Self::DeepSeek => "DeepSeek",
            Self::Groq => "Groq",
            Self::Mistral => "Mistral",
            Self::XaiDirect => "xAI (direct API key)",
            Self::Together => "Together AI",
            Self::Fireworks => "Fireworks",
            Self::Cerebras => "Cerebras",
            Self::Nvidia => "NVIDIA NIM",
            Self::OpenRouter => "OpenRouter",
            Self::Poolside => "Poolside",
            Self::MiniMax => "MiniMax",
            Self::MiniMaxCn => "MiniMax (China)",
            Self::Zai => "Z.AI",
            Self::ZaiCoding => "Z.AI Coding Plan",
            Self::ZaiCodingCn => "Z.AI Coding Plan (CN)",
            Self::Ollama => "Ollama Cloud",
            Self::Nexus => "Nexus",
            Self::AnthropicClaude => "Anthropic Claude (Pro/Max)",
        }
    }

    /// Compiled-in default base (overridable via `GROK_*_BASE_URL` env).
    fn default_base_url(self) -> &'static str {
        match self {
            // Kimi Code subscription: https://api.kimi.com/coding/v1.
            Self::KimiCode => KIMI_CODE_BASE_URL_DEFAULT,
            Self::OpenAiCodex => OPENAI_CODEX_BASE_URL_DEFAULT,
            Self::OpenCodeGo => OPENCODE_GO_BASE_URL_DEFAULT,
            Self::MoonshotCn => MOONSHOT_CN_BASE_URL_DEFAULT,
            Self::MoonshotAi => MOONSHOT_AI_BASE_URL_DEFAULT,
            Self::OpenAi => OPENAI_BASE_URL_DEFAULT,
            Self::Anthropic => ANTHROPIC_BASE_URL_DEFAULT,
            Self::DeepSeek => "https://api.deepseek.com",
            Self::Groq => "https://api.groq.com/openai/v1",
            Self::Mistral => "https://api.mistral.ai/v1",
            Self::XaiDirect => "https://api.x.ai/v1",
            Self::Together => "https://api.together.xyz/v1",
            Self::Fireworks => "https://api.fireworks.ai/inference/v1",
            Self::Cerebras => "https://api.cerebras.ai/v1",
            Self::Nvidia => "https://integrate.api.nvidia.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Poolside => "https://inference.poolside.ai/v1",
            Self::MiniMax => "https://api.minimax.io/v1",
            Self::MiniMaxCn => "https://api.minimaxi.com/v1",
            // General Z.AI PaaS (pay-as-you-go). Coding Plan uses `ZaiCoding`.
            Self::Zai => "https://api.z.ai/api/paas/v4",
            // International Coding Plan OpenAI-compatible endpoint.
            Self::ZaiCoding => "https://api.z.ai/api/coding/paas/v4",
            Self::ZaiCodingCn => "https://open.bigmodel.cn/api/coding/paas/v4",
            Self::Ollama => "https://ollama.com/v1",
            // Nexus gateway root. Per-backend bases are derived at model-fetch
            // time (`nexus_chat_base` / `nexus_messages_base`); this raw root is
            // what `GROK_NEXUS_BASE_URL` overrides and login persists.
            Self::Nexus => NEXUS_BASE_URL_DEFAULT,
            // Claude subscription inference uses Anthropic Messages; the sampler
            // joins `{base}/messages`, so the base ends in `/v1`.
            Self::AnthropicClaude => ANTHROPIC_BASE_URL_DEFAULT,
        }
    }

    /// Inference / model-list base URL.
    pub fn base_url(self) -> String {
        self.base_url_with(|name| std::env::var(name).ok())
    }

    /// Testable base-URL resolver. Grok-native overrides use the sampler's
    /// `{base}/messages` convention; the Claude Code alias uses the Anthropic
    /// SDK's `{root}/v1/messages` convention and is normalized accordingly.
    fn base_url_with(self, mut getenv: impl FnMut(&str) -> Option<String>) -> String {
        let mut read = |name: &str| getenv(name).filter(|value| !value.trim().is_empty());

        let raw = if self == Self::Anthropic {
            if let Some(url) = read(ANTHROPIC_BASE_URL_ENV) {
                // GROK_ANTHROPIC_BASE_URL already uses Grok's versioned-base shape.
                url
            } else if let Some(url) = read(ANTHROPIC_BASE_URL_ALIAS_ENV) {
                normalize_anthropic_sdk_base_url(&url)
            } else {
                self.default_base_url().to_string()
            }
        } else {
            // Prefer well-known envs for core platforms; generic
            // GROK_{ID}_BASE_URL for the rest.
            let specific = match self {
                Self::KimiCode => Some(KIMI_CODE_BASE_URL_ENV),
                Self::OpenAiCodex => Some(OPENAI_CODEX_BASE_URL_ENV),
                Self::MoonshotCn => Some(MOONSHOT_CN_BASE_URL_ENV),
                Self::MoonshotAi => Some(MOONSHOT_AI_BASE_URL_ENV),
                Self::OpenAi => Some(OPENAI_BASE_URL_ENV),
                Self::Anthropic => unreachable!("handled above"),
                _ => None,
            };
            if let Some(var) = specific {
                read(var).unwrap_or_else(|| self.default_base_url().to_string())
            } else {
                let generic = format!(
                    "GROK_{}_BASE_URL",
                    self.as_str().replace('-', "_").to_ascii_uppercase()
                );
                read(&generic).unwrap_or_else(|| self.default_base_url().to_string())
            }
        };
        if self == Self::KimiCode {
            normalize_kimi_code_base_url(&raw)
        } else {
            raw
        }
    }

    /// OAuth host for the subscription channel only.
    pub fn oauth_host(self) -> Option<String> {
        match self {
            Self::KimiCode => Some(env_or(
                KIMI_CODE_OAUTH_HOST_ENV,
                KIMI_CODE_OAUTH_HOST_DEFAULT,
            )),
            Self::OpenAiCodex => Some(env_or(
                OPENAI_CODEX_OAUTH_HOST_ENV,
                OPENAI_CODEX_OAUTH_HOST_DEFAULT,
            )),
            _ => None,
        }
    }

    /// True for the OAuth-bearer subscription channel.
    pub fn uses_oauth(self) -> bool {
        matches!(
            self,
            Self::KimiCode | Self::OpenAiCodex | Self::AnthropicClaude
        )
    }

    /// Anthropic Messages uses `x-api-key` rather than Bearer.
    pub fn uses_x_api_key(self) -> bool {
        matches!(self, Self::Anthropic)
    }

    /// Model-id prefixes admitted from this platform's `/models` listing.
    /// `None` = no filtering.
    pub fn allowed_model_prefixes(self) -> Option<&'static [&'static str]> {
        match self {
            Self::KimiCode => None,
            Self::MoonshotCn | Self::MoonshotAi => Some(&["kimi-k", "kimi-k3", "k3", "k2p7"]),
            _ => None,
        }
    }

    /// Env var names holding this platform's API key (open platforms only).
    /// Empty for the OAuth channel.
    ///
    /// SECURITY: the *values* behind these names must never be logged.
    pub fn api_key_env_names(self) -> &'static [&'static str] {
        match self {
            Self::KimiCode => &[
                KIMI_CODE_API_KEY_ENV,
                KIMI_API_KEY_ENV,
                KIMI_API_KEY_ALIAS_ENV,
            ],
            Self::OpenAiCodex | Self::AnthropicClaude => &[],
            Self::OpenCodeGo => &[OPENCODE_GO_API_KEY_ENV, OPENCODE_API_KEY_ENV],
            Self::MoonshotCn => &[
                MOONSHOT_CN_API_KEY_ENV,
                MOONSHOT_API_KEY_ENV,
                MOONSHOT_API_KEY_ALIAS_ENV,
            ],
            Self::MoonshotAi => &[
                MOONSHOT_AI_API_KEY_ENV,
                MOONSHOT_API_KEY_ENV,
                MOONSHOT_API_KEY_ALIAS_ENV,
            ],
            Self::OpenAi => &[OPENAI_API_KEY_ENV, OPENAI_API_KEY_ALIAS_ENV],
            Self::Anthropic => &[
                ANTHROPIC_API_KEY_ENV,
                // Match Claude Code: an explicit Bearer token wins over the
                // standard x-api-key alias when both are present.
                ANTHROPIC_AUTH_TOKEN_ENV,
                ANTHROPIC_API_KEY_ALIAS_ENV,
            ],
            Self::DeepSeek => &["GROK_DEEPSEEK_API_KEY", "DEEPSEEK_API_KEY"],
            Self::Groq => &["GROK_GROQ_API_KEY", "GROQ_API_KEY"],
            Self::Mistral => &["GROK_MISTRAL_API_KEY", "MISTRAL_API_KEY"],
            Self::XaiDirect => &["GROK_XAI_DIRECT_API_KEY", "XAI_API_KEY"],
            Self::Together => &["GROK_TOGETHER_API_KEY", "TOGETHER_API_KEY"],
            Self::Fireworks => &["GROK_FIREWORKS_API_KEY", "FIREWORKS_API_KEY"],
            Self::Cerebras => &["GROK_CEREBRAS_API_KEY", "CEREBRAS_API_KEY"],
            Self::Nvidia => &["GROK_NVIDIA_API_KEY", "NVIDIA_API_KEY"],
            Self::OpenRouter => &["GROK_OPENROUTER_API_KEY", "OPENROUTER_API_KEY"],
            Self::Poolside => &["GROK_POOLSIDE_API_KEY", "POOLSIDE_API_KEY"],
            Self::MiniMax => &["GROK_MINIMAX_API_KEY", "MINIMAX_API_KEY"],
            Self::MiniMaxCn => &["GROK_MINIMAX_CN_API_KEY", "MINIMAX_API_KEY"],
            Self::Zai => &["GROK_ZAI_API_KEY", "ZAI_API_KEY"],
            Self::ZaiCoding => &["GROK_ZAI_CODING_API_KEY", "ZAI_API_KEY"],
            Self::ZaiCodingCn => &["GROK_ZAI_CODING_CN_API_KEY", "ZAI_API_KEY"],
            Self::Ollama => &["GROK_OLLAMA_API_KEY", "OLLAMA_API_KEY"],
            Self::Nexus => &["GROK_NEXUS_API_KEY", "NEXUS_API_KEY"],
        }
    }

    /// `{base}/models` URL for catalog sync.
    pub fn models_list_url(self) -> String {
        let base = self.base_url().trim_end_matches('/').to_string();
        format!("{base}/models")
    }

    /// Human setup instructions for enabling this platform (no secrets).
    ///
    /// Shown wherever a locked (credential-less) platform model surfaces:
    /// the model picker description, `set_session_model` rejections, and
    /// the pager's `/providers` overview.
    pub fn setup_hint(self) -> String {
        let envs = self.api_key_env_names();
        let env_part = match envs {
            [] => String::new(),
            [one] => format!("export {one}=<key>"),
            [first, rest @ ..] => format!("export {first}=<key> (or {})", rest.join(" / ")),
        };
        let ui_part = format!("run /providers {} <api_key>", self.as_str());
        let config_part = format!(
            "add `api_key = \"<key>\"` under `[platforms.{}]` in ~/.grok/config.toml",
            self.as_str()
        );
        let api_key_hint = if env_part.is_empty() {
            format!("{ui_part}, or {config_part}")
        } else {
            format!("{ui_part}, or {env_part}, or {config_part}")
        };

        if self.uses_oauth() {
            let login_target = match self {
                Self::OpenAiCodex => "/login openai",
                Self::AnthropicClaude => "/login claude",
                _ => "/login kimi",
            };
            let oauth_hint = format!(
                "Sign in with your {} subscription: run {login_target}",
                self.display_name()
            );
            return if self == Self::KimiCode {
                format!("{oauth_hint}, or use an API key: {api_key_hint}")
            } else {
                oauth_hint
            };
        }
        api_key_hint
    }

    /// Whether to auto-fetch live `GET /models` for this platform.
    ///
    /// Kimi / Moonshot / Ollama Cloud auto-sync; others use the Pi offline
    /// catalog (org listings are huge / noisy).
    pub fn live_models_list_enabled(self) -> bool {
        matches!(
            self,
            Self::KimiCode | Self::MoonshotCn | Self::MoonshotAi | Self::Ollama | Self::Nexus
        )
    }

    /// Managed catalog key: `{platform_id}/{model_id}`.
    pub fn managed_model_key(self, model_id: &str) -> String {
        format!("{}/{model_id}", self.as_str())
    }

    /// Whether `url` is this platform's inference base (scheme+host match).
    pub fn base_url_matches(self, url: &str) -> bool {
        let base = self.base_url();
        urls_same_origin(&base, url)
    }
}

impl From<PlatformId> for ProviderId {
    fn from(platform: PlatformId) -> Self {
        platform.provider_id()
    }
}

fn urls_same_origin(a: &str, b: &str) -> bool {
    fn host_key(u: &str) -> Option<String> {
        let u = u.trim().trim_end_matches('/');
        let rest = u
            .strip_prefix("https://")
            .or_else(|| u.strip_prefix("http://"))?;
        let host = rest.split('/').next()?.to_ascii_lowercase();
        Some(host)
    }
    match (host_key(a), host_key(b)) {
        (Some(ha), Some(hb)) => ha == hb,
        _ => {
            let na = a.trim().trim_end_matches('/').to_ascii_lowercase();
            let nb = b.trim().trim_end_matches('/').to_ascii_lowercase();
            !na.is_empty() && na == nb
        }
    }
}

/// Split `{provider_id}/{model_id}` back into a canonical provider + bare model id.
///
/// Unlike the legacy enum-only parser, this recognizes every validated registry
/// provider, including generated providers that do not need bespoke runtime code.
pub fn parse_managed_model_key(key: &str) -> Option<(ProviderId, &str)> {
    let (provider, model_id) = key.split_once('/')?;
    if model_id.is_empty() {
        return None;
    }
    let spec = provider_spec(provider)?;
    Some((spec.id.clone(), model_id))
}

/// Wire API backend for a built-in catalog entry (maps to shell `ApiBackend`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformApiBackend {
    ChatCompletions,
    Responses,
    Messages,
    GoogleGenerateContent,
    BedrockConverseStream,
    PiMessages,
}

impl PlatformApiBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
            Self::GoogleGenerateContent => "google_generate_content",
            Self::BedrockConverseStream => "bedrock_converse_stream",
            Self::PiMessages => "pi_messages",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chat_completions" | "chat-completions" => Some(Self::ChatCompletions),
            "responses" => Some(Self::Responses),
            "messages" => Some(Self::Messages),
            "google_generate_content" | "google-generate-content" => {
                Some(Self::GoogleGenerateContent)
            }
            "bedrock_converse_stream" | "bedrock-converse-stream" => {
                Some(Self::BedrockConverseStream)
            }
            "pi_messages" | "pi-messages" => Some(Self::PiMessages),
            _ => None,
        }
    }

    pub fn endpoint_path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat/completions",
            Self::Responses => "responses",
            Self::Messages => "messages",
            Self::GoogleGenerateContent => "models/{model}:generateContent",
            Self::BedrockConverseStream => "model/{model}/converse-stream",
            Self::PiMessages => "messages",
        }
    }
}

/// One built-in offline catalog entry for a platform.
///
/// Source of truth for open-platform ids: platform.kimi.ai `/docs/models`
/// (2026-07): `kimi-k3`, `kimi-k2.7-code`, `kimi-k2.7-code-highspeed`,
/// `kimi-k2.6`, `kimi-k2.5`. Deprecated `kimi-k2-*-preview` / thinking-turbo
/// are kept only as last-resort aliases until live `/models` replaces them.
///
/// OpenAI / Anthropic / OpenCode Go entries are loaded from
/// [`PLATFORM_CATALOG_JSON`] (curated from Pi and models.dev).
#[derive(Debug, Clone)]
pub struct BuiltinPlatformModel {
    pub provider: ProviderId,
    pub model: String,
    pub name: String,
    pub description: String,
    pub context_window: u64,
    pub supports_reasoning_effort: bool,
    /// When false, only OAuth session / credential-stamped users see this in
    /// the picker. Catalog rows always start `false` at parse time; the shell
    /// flips this on when keys resolve.
    pub supported_in_api: bool,
    /// Catalog-declared availability. `false` means permanently unavailable
    /// (EOL / 404 / withdrawn) and must stay hidden even after credentials
    /// stamp. Offline OAuth seeds set this `true` so login can unlock them.
    pub catalog_available: bool,
    /// Whether this row is a user-facing picker entry. Runtime aliases stay
    /// in the catalog for config and subagent model pins but are hidden from
    /// normal model selection.
    pub picker_visible: bool,
    /// Permanently unavailable (provider HTTP 410 / withdrawn). Historical
    /// snapshot rows stay in the catalog for archaeology but spawn, picker,
    /// and `/providers` must treat them as gone.
    pub eol: bool,
    /// Recommended `max_tokens` / max_completion_tokens (Kimi docs: 32k for
    /// coding thinking models).
    pub max_completion_tokens: Option<u32>,
    pub api_backend: PlatformApiBackend,
    /// Per-row base URL from the catalog JSON (e.g. MiniMax Messages uses
    /// `https://api.minimax.io/anthropic` rather than the platform default
    /// `/v1` OpenAI-compatible root). When `None`, callers use
    /// the provider registry's environment-aware base URL.
    pub base_url_override: Option<String>,
    /// Fully resolved, protocol-specific request behavior imported from Pi.
    pub request_compat: RequestCompat,
    /// Explicit endpoint, authentication placement, static headers, and query
    /// parameters for this model route.
    pub route: ProviderRouteSpec,
}

impl BuiltinPlatformModel {
    pub fn catalog_key(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    pub fn provider_spec(&self) -> &'static ProviderSpec {
        provider_spec(self.provider.as_str())
            .expect("validated built-in model references a registered provider")
    }

    pub fn legacy_platform(&self) -> Option<PlatformId> {
        self.provider.platform_id()
    }

    /// Base URL in Grok's `{base}/{backend-endpoint}` convention.
    ///
    /// The sampler joins `{base}/messages` (not `/v1/messages`). Anthropic-SDK
    /// style roots therefore need a trailing `/v1`:
    /// - Anthropic platform rows use the registry's environment-aware base URL,
    ///   preserving `GROK_ANTHROPIC_BASE_URL` / `ANTHROPIC_BASE_URL` handling.
    /// - Other **Messages** backends with a catalog `base_url_override`
    ///   (MiniMax `/anthropic`, Fireworks `/inference`, …) are normalized the
    ///   same way via [`normalize_messages_sdk_base_url`].
    /// - Chat Completions / Responses overrides are returned as-is.
    fn raw_resolved_base_url(&self) -> String {
        let spec = self.provider_spec();
        let has_env_override = spec.base_url_env_keys.iter().any(|name| {
            std::env::var(name)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
        });
        let raw = if has_env_override {
            // A user/provider override must win over Pi's per-row route base.
            spec.base_url()
        } else {
            self.base_url_override
                .clone()
                .unwrap_or_else(|| spec.base_url())
        };
        let grok_anthropic_override_is_already_versioned = self.legacy_platform()
            == Some(PlatformId::Anthropic)
            && std::env::var(ANTHROPIC_BASE_URL_ENV)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
        if self.api_backend == PlatformApiBackend::Messages
            && !grok_anthropic_override_is_already_versioned
        {
            normalize_messages_sdk_base_url(&raw)
        } else {
            raw
        }
    }

    /// Materialize all non-secret runtime route metadata for this model.
    pub fn resolved_runtime(&self) -> ResolvedProviderRuntime {
        let wire_model = match self.provider.as_str() {
            "nvidia" => nvidia_wire_model_id(&self.model),
            "poolside" => poolside_wire_model_id(&self.model),
            _ => self.model.clone(),
        };
        self.provider_spec().resolve_runtime(
            &self.raw_resolved_base_url(),
            &wire_model,
            &self.route.query_params,
        )
    }

    pub fn resolved_base_url(&self) -> String {
        self.resolved_runtime().base_url
    }

    pub fn context_window_nonzero(&self) -> NonZeroU64 {
        NonZeroU64::new(self.context_window).expect("builtin context_window is non-zero")
    }
}

const CTX_256K: u64 = 262_144;
const CTX_1M: u64 = 1_048_576;
const MAX_TOK_32K: Option<u32> = Some(32_768);

/// Embedded third-party platform catalog (offline). See
/// `platform_catalog.json` header for upstream sources.
pub const PLATFORM_CATALOG_JSON: &str = include_str!("../platform_catalog.json");

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    version: u32,
    source: String,
    models: Vec<CatalogModelRow>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogModelRow {
    platform: String,
    model: String,
    name: String,
    description: String,
    context_window: u64,
    max_completion_tokens: Option<u32>,
    api_backend: String,
    supports_reasoning_effort: bool,
    /// Optional per-model base URL (e.g. MiniMax Anthropic-compatible path).
    #[serde(default)]
    base_url_override: Option<String>,
    request_compat: RequestCompat,
    route: ProviderRouteSpec,
    /// Catalog row availability. `false` = permanently unavailable (EOL).
    /// `true` still starts runtime-hidden until the shell stamps credentials.
    supported_in_api: bool,
    /// Explicit EOL / HTTP 410 flag. Prefer this over deleting the snapshot
    /// row. Defaults to `false` so existing catalog JSON does not need a
    /// bulk rewrite.
    #[serde(default)]
    eol: bool,
    source: String,
}

struct EmbeddedProviderAssets {
    registry: ProviderRegistry,
    catalog_models: Vec<BuiltinPlatformModel>,
}

static EMBEDDED_PROVIDER_ASSETS: LazyLock<EmbeddedProviderAssets> = LazyLock::new(|| {
    load_provider_assets(PLATFORM_REGISTRY_JSON, PLATFORM_CATALOG_JSON)
        .unwrap_or_else(|error| panic!("embedded provider registry/catalog is invalid:\n{error}"))
});

/// Validated runtime provider metadata.
pub fn provider_registry() -> &'static ProviderRegistry {
    &EMBEDDED_PROVIDER_ASSETS.registry
}

/// Find a provider by canonical id or alias.
pub fn provider_spec(id_or_alias: &str) -> Option<&'static ProviderSpec> {
    provider_registry().find(id_or_alias)
}

/// Validate generated provider assets without installing them. Sync tooling and
/// tests use this entry point to reject partial, stale, or unknown rows.
pub fn validate_provider_assets(
    registry_json: &str,
    catalog_json: &str,
) -> Result<(), ProviderAssetError> {
    load_provider_assets(registry_json, catalog_json).map(|_| ())
}

fn load_provider_assets(
    registry_json: &str,
    catalog_json: &str,
) -> Result<EmbeddedProviderAssets, ProviderAssetError> {
    let registry = parse_provider_registry(registry_json)?;
    let catalog_models = parse_platform_catalog(catalog_json, &registry)?;
    Ok(EmbeddedProviderAssets {
        registry,
        catalog_models,
    })
}

fn parse_provider_registry(json: &str) -> Result<ProviderRegistry, ProviderAssetError> {
    let registry: ProviderRegistry = serde_json::from_str(json).map_err(|error| {
        ProviderAssetError::new(vec![format!(
            "platform_registry.json is not valid JSON: {error}"
        )])
    })?;
    let issues = validate_provider_registry(&registry);
    if issues.is_empty() {
        Ok(registry)
    } else {
        Err(ProviderAssetError::new(issues))
    }
}

fn validate_provider_registry(registry: &ProviderRegistry) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut issues = Vec::new();
    if registry.version != PLATFORM_REGISTRY_VERSION {
        issues.push(format!(
            "platform_registry.json version {} is unsupported; expected {PLATFORM_REGISTRY_VERSION}",
            registry.version
        ));
    }
    if registry.source.trim().is_empty() {
        issues.push("platform_registry.json source must not be blank".into());
    }
    if registry.providers.is_empty() {
        issues.push("platform_registry.json providers must not be empty".into());
    }

    let mut canonical_ids = BTreeMap::<String, usize>::new();
    let mut all_names = BTreeMap::<String, String>::new();
    for (index, spec) in registry.providers.iter().enumerate() {
        let id = spec.id.as_str();
        let label = format!("providers[{index}] ({id})");
        if !valid_provider_token(id) {
            issues.push(format!(
                "{label}: id must be a lowercase kebab-case provider token"
            ));
        }
        if let Some(previous) = canonical_ids.insert(id.to_string(), index) {
            issues.push(format!(
                "{label}: duplicate canonical id (already used by providers[{previous}])"
            ));
        }
        if let Some(owner) = all_names.insert(id.to_string(), id.to_string()) {
            issues.push(format!("{label}: id collides with name owned by {owner}"));
        }
        if spec.display_name.trim().is_empty() || spec.display_name.trim() != spec.display_name {
            issues.push(format!(
                "{label}: display_name must be non-blank without surrounding whitespace"
            ));
        }
        if let Some(pi_id) = &spec.pi_id
            && !valid_provider_token(pi_id)
        {
            issues.push(format!(
                "{label}: pi_id `{pi_id}` is not a valid provider token"
            ));
        }
        if !valid_http_base_url(&spec.default_base_url) {
            issues.push(format!(
                "{label}: default_base_url must be an absolute http(s) URL without whitespace or a trailing slash"
            ));
        }

        let mut aliases = BTreeSet::new();
        for alias in &spec.aliases {
            if !valid_provider_token(alias) {
                issues.push(format!(
                    "{label}: alias `{alias}` is not a valid provider token"
                ));
            }
            if !aliases.insert(alias.as_str()) {
                issues.push(format!("{label}: duplicate alias `{alias}`"));
            }
            if alias == id {
                issues.push(format!("{label}: alias `{alias}` repeats the canonical id"));
            }
            if let Some(owner) = all_names.insert(alias.clone(), id.to_string()) {
                issues.push(format!(
                    "{label}: alias `{alias}` collides with a canonical id or alias owned by {owner}"
                ));
            }
        }

        validate_env_keys(
            &label,
            "base_url_env_keys",
            &spec.base_url_env_keys,
            &mut issues,
        );
        if spec.base_url_env_keys.is_empty() {
            issues.push(format!("{label}: base_url_env_keys must not be empty"));
        }
        validate_env_keys(
            &label,
            "runtime.base_url_template_env_keys",
            &spec.runtime.base_url_template_env_keys,
            &mut issues,
        );
        validate_base_url_template(
            &label,
            "default_base_url",
            &spec.default_base_url,
            &spec.runtime.base_url_template_env_keys,
            &mut issues,
        );
        for (query_key, env_key) in &spec.runtime.query_params_from_env {
            if query_key.trim().is_empty() || query_key.trim() != query_key {
                issues.push(format!(
                    "{label}: runtime query parameter keys must not be blank or padded"
                ));
            }
            validate_env_keys(
                &label,
                &format!("runtime.query_params_from_env[{query_key}]"),
                std::slice::from_ref(env_key),
                &mut issues,
            );
        }
        if let Some(env_key) = &spec.runtime.model_id_map_env_key {
            validate_env_keys(
                &label,
                "runtime.model_id_map_env_key",
                std::slice::from_ref(env_key),
                &mut issues,
            );
        }
        validate_env_keys(
            &label,
            "credentials.env_keys",
            &spec.credentials.env_keys,
            &mut issues,
        );
        match spec.credentials.kind {
            ProviderCredentialKind::Oauth if !spec.credentials.env_keys.is_empty() => {
                issues.push(format!(
                    "{label}: OAuth providers must not declare API-key env names"
                ));
            }
            ProviderCredentialKind::ApiKey | ProviderCredentialKind::Hybrid
                if spec.credentials.env_keys.is_empty() =>
            {
                issues.push(format!(
                    "{label}: API-key and hybrid providers must declare at least one env name"
                ));
            }
            _ => {}
        }
        if let Some(group) = &spec.credentials.storage_group
            && !valid_provider_token(group)
        {
            issues.push(format!(
                "{label}: credentials.storage_group `{group}` is not a valid provider token"
            ));
        }

        match spec.discovery.mode {
            ProviderDiscoveryMode::ModelsEndpoint => {
                if !matches!(spec.discovery.models_path.as_deref(), Some(path) if path.starts_with('/') && path.len() > 1)
                {
                    issues.push(format!(
                        "{label}: models_endpoint discovery requires an absolute models_path"
                    ));
                }
            }
            ProviderDiscoveryMode::Disabled | ProviderDiscoveryMode::Adapter => {
                if spec.discovery.models_path.is_some() {
                    issues.push(format!(
                        "{label}: disabled/adapter discovery must not declare models_path"
                    ));
                }
            }
        }
        let mut prefixes = BTreeSet::new();
        for prefix in &spec.discovery.model_prefixes {
            if prefix.trim().is_empty() || prefix.trim() != prefix {
                issues.push(format!(
                    "{label}: model prefixes must be non-blank without surrounding whitespace"
                ));
            }
            if !prefixes.insert(prefix.as_str()) {
                issues.push(format!("{label}: duplicate model prefix `{prefix}`"));
            }
        }
    }

    for spec in &registry.providers {
        let Some(group) = spec.credentials.storage_group.as_deref() else {
            continue;
        };
        let Some(owner) = registry
            .providers
            .iter()
            .find(|candidate| candidate.id.as_str() == group)
        else {
            issues.push(format!(
                "provider `{}` credential storage group `{group}` is not a canonical provider id",
                spec.id
            ));
            continue;
        };
        if owner.credentials.kind != spec.credentials.kind
            || owner.credentials.auth != spec.credentials.auth
        {
            issues.push(format!(
                "provider `{}` credential storage group `{group}` must use the same credential kind and auth placement",
                spec.id
            ));
        }
    }

    // Compatibility gate: every bespoke typed platform must retain exactly one
    // registry row. Additional data-driven providers intentionally do not need
    // a `PlatformId` variant.
    for platform in PlatformId::ALL {
        let Some(spec) = registry
            .providers
            .iter()
            .find(|spec| spec.id.as_str() == platform.as_str())
        else {
            issues.push(format!(
                "registry is missing PlatformId `{}`",
                platform.as_str()
            ));
            continue;
        };
        validate_platform_compatibility(platform, spec, &mut issues);
    }

    issues
}

fn validate_platform_compatibility(
    platform: PlatformId,
    spec: &ProviderSpec,
    issues: &mut Vec<String>,
) {
    let id = platform.as_str();
    let expected_aliases: Vec<String> = platform
        .aliases()
        .iter()
        .map(|alias| (*alias).to_string())
        .collect();
    if spec.aliases != expected_aliases {
        issues.push(format!(
            "provider `{id}` aliases {:?} do not match PlatformId aliases {:?}",
            spec.aliases, expected_aliases
        ));
    }
    if spec.display_name != platform.display_name() {
        issues.push(format!(
            "provider `{id}` display_name {:?} does not match {:?}",
            spec.display_name,
            platform.display_name()
        ));
    }
    if spec.default_base_url != platform.default_base_url() {
        issues.push(format!(
            "provider `{id}` default_base_url {:?} does not match {:?}",
            spec.default_base_url,
            platform.default_base_url()
        ));
    }
    let expected_env_keys: Vec<String> = platform
        .api_key_env_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    if spec.credentials.env_keys != expected_env_keys {
        issues.push(format!(
            "provider `{id}` credential env-key order {:?} does not match PlatformId {:?}",
            spec.credentials.env_keys, expected_env_keys
        ));
    }
    let expected_base_env_keys = expected_base_url_env_keys(platform);
    if spec.base_url_env_keys != expected_base_env_keys {
        issues.push(format!(
            "provider `{id}` base URL env-key order {:?} does not match {:?}",
            spec.base_url_env_keys, expected_base_env_keys
        ));
    }

    let expected_kind = if platform == PlatformId::KimiCode {
        ProviderCredentialKind::Hybrid
    } else if platform.uses_oauth() {
        ProviderCredentialKind::Oauth
    } else {
        ProviderCredentialKind::ApiKey
    };
    if spec.credentials.kind != expected_kind {
        issues.push(format!(
            "provider `{id}` credential kind {:?} does not match {:?}",
            spec.credentials.kind, expected_kind
        ));
    }
    let expected_auth = match platform {
        PlatformId::Anthropic => ProviderAuthPlacement::XApiKey,
        PlatformId::Mistral | PlatformId::Nexus => ProviderAuthPlacement::Bearer,
        _ if platform.uses_oauth() => ProviderAuthPlacement::Bearer,
        _ => ProviderAuthPlacement::ProtocolDefault,
    };
    if spec.credentials.auth != expected_auth {
        issues.push(format!(
            "provider `{id}` auth placement {:?} does not match {:?}",
            spec.credentials.auth, expected_auth
        ));
    }
    let expected_storage_group = (platform == PlatformId::OpenCodeGo).then_some("opencode");
    if spec.credentials.storage_group.as_deref() != expected_storage_group {
        issues.push(format!(
            "provider `{id}` credential storage group {:?} does not match {:?}",
            spec.credentials.storage_group, expected_storage_group
        ));
    }

    let expected_adapter = match platform {
        PlatformId::KimiCode => AdapterKind::KimiCoding,
        PlatformId::OpenAiCodex => AdapterKind::OpenAiCodex,
        PlatformId::Mistral => AdapterKind::MistralConversations,
        PlatformId::Nexus => AdapterKind::Nexus,
        PlatformId::AnthropicClaude => AdapterKind::AnthropicClaude,
        _ => AdapterKind::Standard,
    };
    if spec.adapter != expected_adapter {
        issues.push(format!(
            "provider `{id}` adapter {:?} does not match {:?}",
            spec.adapter, expected_adapter
        ));
    }
    let expected_status = ProviderStatus::Active;
    if spec.status != expected_status {
        issues.push(format!(
            "provider `{id}` status {:?} does not match {:?}",
            spec.status, expected_status
        ));
    }

    let expected_discovery = if platform == PlatformId::Nexus {
        ProviderDiscoveryMode::Adapter
    } else if platform.live_models_list_enabled() {
        ProviderDiscoveryMode::ModelsEndpoint
    } else {
        ProviderDiscoveryMode::Disabled
    };
    if spec.discovery.mode != expected_discovery {
        issues.push(format!(
            "provider `{id}` discovery {:?} does not match {:?}",
            spec.discovery.mode, expected_discovery
        ));
    }
    let expected_path =
        (expected_discovery == ProviderDiscoveryMode::ModelsEndpoint).then_some("/models");
    if spec.discovery.models_path.as_deref() != expected_path {
        issues.push(format!(
            "provider `{id}` models_path {:?} does not match {:?}",
            spec.discovery.models_path, expected_path
        ));
    }
    let expected_prefixes: Vec<String> = platform
        .allowed_model_prefixes()
        .unwrap_or_default()
        .iter()
        .map(|prefix| (*prefix).to_string())
        .collect();
    if spec.discovery.model_prefixes != expected_prefixes {
        issues.push(format!(
            "provider `{id}` model prefixes {:?} do not match {:?}",
            spec.discovery.model_prefixes, expected_prefixes
        ));
    }
}

fn expected_base_url_env_keys(platform: PlatformId) -> Vec<String> {
    let names: &[&str] = match platform {
        PlatformId::KimiCode => &[KIMI_CODE_BASE_URL_ENV],
        PlatformId::OpenAiCodex => &[OPENAI_CODEX_BASE_URL_ENV],
        PlatformId::MoonshotCn => &[MOONSHOT_CN_BASE_URL_ENV],
        PlatformId::MoonshotAi => &[MOONSHOT_AI_BASE_URL_ENV],
        PlatformId::OpenAi => &[OPENAI_BASE_URL_ENV],
        PlatformId::Anthropic => &[ANTHROPIC_BASE_URL_ENV, ANTHROPIC_BASE_URL_ALIAS_ENV],
        _ => &[],
    };
    if !names.is_empty() {
        return names.iter().map(|name| (*name).to_string()).collect();
    }
    vec![format!(
        "GROK_{}_BASE_URL",
        platform.as_str().replace('-', "_").to_ascii_uppercase()
    )]
}

fn validate_env_keys(label: &str, field: &str, keys: &[String], issues: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    for key in keys {
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            issues.push(format!(
                "{label}: {field} contains invalid env name `{key}`"
            ));
        }
        if !seen.insert(key) {
            issues.push(format!("{label}: {field} contains duplicate `{key}`"));
        }
    }
}

fn valid_provider_token(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && !value.contains("--")
}

fn valid_http_base_url(value: &str) -> bool {
    value.trim() == value
        && !value.ends_with('/')
        && !value.chars().any(char::is_whitespace)
        && value
            .strip_prefix("https://")
            .or_else(|| value.strip_prefix("http://"))
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
}

fn validate_base_url_template(
    label: &str,
    field: &str,
    value: &str,
    allowed_env_keys: &[String],
    issues: &mut Vec<String>,
) {
    let names = match base_url_template_env_names(value) {
        Ok(names) => names,
        Err(()) => {
            issues.push(format!(
                "{label}: {field} contains a malformed environment placeholder"
            ));
            return;
        }
    };
    let allowed: std::collections::BTreeSet<&str> =
        allowed_env_keys.iter().map(String::as_str).collect();
    let mut materialized = value.to_string();
    for name in names {
        if !allowed.contains(name.as_str()) {
            issues.push(format!(
                "{label}: {field} placeholder `{{{name}}}` is not declared in runtime.base_url_template_env_keys"
            ));
        }
        materialized = materialized.replace(&format!("{{{name}}}"), "placeholder");
    }
    if normalize_runtime_base_url(&materialized, ProviderBaseUrlNormalization::None).is_none() {
        issues.push(format!(
            "{label}: {field} must materialize to an absolute HTTP(S) URL without userinfo or fragment"
        ));
    }
}

fn validate_route_spec(label: &str, route: &ProviderRouteSpec, issues: &mut Vec<String>) {
    if route.path.is_empty()
        || route.path.starts_with('/')
        || route.path.ends_with('/')
        || route.path.contains("..")
        || route.path.contains('?')
        || route.path.contains('#')
        || route.path.chars().any(char::is_whitespace)
    {
        issues.push(format!(
            "{label}: route.path must be a non-empty relative endpoint path without query/fragment/whitespace"
        ));
    }
    let mut normalized_header_names = std::collections::BTreeSet::new();
    for (name, value) in &route.headers {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            issues.push(format!("{label}: route header name `{name}` is invalid"));
        }
        let normalized_name = name.to_ascii_lowercase();
        if !normalized_header_names.insert(normalized_name.clone()) {
            issues.push(format!(
                "{label}: route headers contain a case-insensitive duplicate `{name}`"
            ));
        }
        if matches!(
            normalized_name.as_str(),
            "authorization" | "x-api-key" | "api-key" | "cf-aig-authorization"
        ) {
            issues.push(format!(
                "{label}: route header `{name}` conflicts with typed authentication metadata"
            ));
        }
        if value.contains(['\r', '\n']) {
            issues.push(format!("{label}: route header `{name}` contains a newline"));
        }
    }
    for (name, value) in &route.query_params {
        if name.trim().is_empty() || name.trim() != name {
            issues.push(format!(
                "{label}: route query key must not be blank or padded"
            ));
        }
        if value.contains(['\r', '\n']) {
            issues.push(format!(
                "{label}: route query value for `{name}` contains a newline"
            ));
        }
    }
}

fn parse_platform_catalog(
    json: &str,
    registry: &ProviderRegistry,
) -> Result<Vec<BuiltinPlatformModel>, ProviderAssetError> {
    use std::collections::BTreeMap;

    let file: CatalogFile = serde_json::from_str(json).map_err(|error| {
        ProviderAssetError::new(vec![format!(
            "platform_catalog.json is not valid JSON: {error}"
        )])
    })?;
    let mut issues = Vec::new();
    if file.version != PLATFORM_CATALOG_VERSION {
        issues.push(format!(
            "platform_catalog.json version {} is unsupported; expected {PLATFORM_CATALOG_VERSION}",
            file.version
        ));
    }
    if file.source.trim().is_empty() {
        issues.push("platform_catalog.json source must not be blank".into());
    }
    if file.models.is_empty() {
        issues.push("platform_catalog.json models must not be empty".into());
    }

    let mut keys = BTreeMap::<String, usize>::new();
    let mut models = Vec::with_capacity(file.models.len());
    for (index, row) in file.models.into_iter().enumerate() {
        let key = format!("{}/{}", row.platform, row.model);
        let label = format!("models[{index}] ({key})");
        if row.platform.trim() != row.platform || row.platform.is_empty() {
            issues.push(format!("{label}: platform must not be blank or padded"));
        }
        let provider = match registry.find(&row.platform) {
            Some(spec) if spec.id.as_str() == row.platform => {
                if spec.status != ProviderStatus::Active {
                    issues.push(format!("{label}: provider `{}` is not active", spec.id));
                }
                Some(spec)
            }
            Some(spec) => {
                issues.push(format!(
                    "{label}: catalog must use canonical provider id `{}` instead of alias `{}`",
                    spec.id, row.platform
                ));
                None
            }
            None => {
                issues.push(format!(
                    "{label}: unknown provider `{}` (missing from platform_registry.json)",
                    row.platform
                ));
                None
            }
        };
        let api_backend = match PlatformApiBackend::parse(&row.api_backend) {
            Some(backend) => Some(backend),
            None => {
                issues.push(format!(
                    "{label}: unknown api_backend `{}`",
                    row.api_backend
                ));
                None
            }
        };
        if let Some(backend) = api_backend {
            let compat_matches = matches!(
                (backend, &row.request_compat),
                (
                    PlatformApiBackend::ChatCompletions,
                    RequestCompat::ChatCompletions(_)
                ) | (PlatformApiBackend::Responses, RequestCompat::Responses(_))
                    | (PlatformApiBackend::Messages, RequestCompat::Messages(_))
                    | (
                        PlatformApiBackend::GoogleGenerateContent,
                        RequestCompat::GoogleGenerateContent(_),
                    )
                    | (
                        PlatformApiBackend::BedrockConverseStream,
                        RequestCompat::BedrockConverseStream(_),
                    )
                    | (PlatformApiBackend::PiMessages, RequestCompat::PiMessages(_))
            );
            if !compat_matches {
                issues.push(format!(
                    "{label}: request_compat protocol does not match api_backend `{}`",
                    row.api_backend
                ));
            }
            if let Some(spec) = provider {
                let expected_auth = match spec.credentials.auth {
                    ProviderAuthPlacement::Bearer => RouteAuth::Bearer,
                    ProviderAuthPlacement::XApiKey => RouteAuth::XApiKey,
                    ProviderAuthPlacement::ApiKey => RouteAuth::ApiKey,
                    ProviderAuthPlacement::CfAigAuthorization => RouteAuth::CfAigAuthorization,
                    ProviderAuthPlacement::XGoogApiKey => RouteAuth::XGoogApiKey,
                    ProviderAuthPlacement::ProtocolDefault
                        if backend == PlatformApiBackend::Messages =>
                    {
                        RouteAuth::XApiKey
                    }
                    ProviderAuthPlacement::ProtocolDefault
                        if backend == PlatformApiBackend::GoogleGenerateContent =>
                    {
                        RouteAuth::XGoogApiKey
                    }
                    ProviderAuthPlacement::ProtocolDefault => RouteAuth::Bearer,
                };
                if row.route.auth != expected_auth {
                    issues.push(format!(
                        "{label}: route auth {:?} does not match expected {:?}",
                        row.route.auth, expected_auth
                    ));
                }
                for query_key in spec.runtime.query_params_from_env.keys() {
                    if !row.route.query_params.contains_key(query_key) {
                        issues.push(format!(
                            "{label}: runtime query override `{query_key}` has no static route default"
                        ));
                    }
                }
            }
        }
        validate_route_spec(&label, &row.route, &mut issues);
        if let Some(previous) = keys.insert(key.clone(), index) {
            issues.push(format!(
                "{label}: duplicate catalog key (already used by models[{previous}])"
            ));
        }
        for (field, value) in [
            ("model", row.model.as_str()),
            ("name", row.name.as_str()),
            ("description", row.description.as_str()),
            ("source", row.source.as_str()),
        ] {
            if value.trim().is_empty() || value.trim() != value {
                issues.push(format!(
                    "{label}: {field} must be non-blank without surrounding whitespace"
                ));
            }
        }
        if row.context_window == 0 {
            issues.push(format!("{label}: context_window must be greater than zero"));
        }
        if row.max_completion_tokens == Some(0) {
            issues.push(format!(
                "{label}: max_completion_tokens must be greater than zero when present"
            ));
        }
        if let Some(base_url) = &row.base_url_override {
            if !valid_http_base_url(base_url) {
                issues.push(format!(
                    "{label}: base_url_override must be an absolute http(s) URL without whitespace or a trailing slash"
                ));
            }
            if let Some(provider) = provider {
                validate_base_url_template(
                    &label,
                    "base_url_override",
                    base_url,
                    &provider.runtime.base_url_template_env_keys,
                    &mut issues,
                );
            }
        }

        if let (Some(provider), Some(api_backend)) = (provider, api_backend) {
            let request_compat = apply_catalog_compat_overrides(
                provider.id.as_str(),
                &row.model,
                row.request_compat,
            );
            models.push(BuiltinPlatformModel {
                provider: provider.id.clone(),
                model: row.model,
                name: row.name,
                description: row.description,
                context_window: row.context_window,
                supports_reasoning_effort: row.supports_reasoning_effort,
                // Pi catalog ships `supported_in_api: true` for many providers,
                // but we must not show models in the picker until credentials
                // (env/config OAuth) are actually available. The shell's
                // `apply_platform_credentials` re-enables visibility when keys
                // resolve — unless `catalog_available` is false (EOL).
                supported_in_api: false,
                catalog_available: row.supported_in_api && !row.eol,
                picker_visible: !row.eol,
                eol: row.eol,
                max_completion_tokens: row.max_completion_tokens,
                api_backend,
                base_url_override: row.base_url_override,
                request_compat,
                route: row.route,
            });
        }
    }

    if issues.is_empty() {
        Ok(models)
    } else {
        Err(ProviderAssetError::new(issues))
    }
}

fn load_platform_catalog_models() -> Vec<BuiltinPlatformModel> {
    EMBEDDED_PROVIDER_ASSETS.catalog_models.clone()
}

/// Offline catalog. Primary sources are official Pi `packages/ai` generated
/// data and models.dev's OpenCode Go provider (`platform_catalog.json`).
/// Hand-maintained Kimi/Moonshot rows fill gaps only when the imported catalog
/// lacks that catalog key.
pub fn platform_builtin_models() -> &'static [BuiltinPlatformModel] {
    static MODELS: LazyLock<Vec<BuiltinPlatformModel>> = LazyLock::new(|| {
        let mut out: Vec<BuiltinPlatformModel> = load_platform_catalog_models();
        let mut existing: std::collections::HashMap<String, usize> = out
            .iter()
            .enumerate()
            .map(|(i, m)| (m.catalog_key(), i))
            .collect();
        // Hand-maintained Kimi/Moonshot fallbacks override the Pi catalog so
        // we keep canonical ids / descriptions. Kimi Code subscription uses
        // Anthropic Messages (same as official Pi kimi-coding).
        for m in kimi_moonshot_offline_fallbacks() {
            if let Some(idx) = existing.get(&m.catalog_key()) {
                out[*idx] = m;
            } else {
                existing.insert(m.catalog_key(), out.len());
                out.push(m);
            }
        }
        // OpenAI Codex (ChatGPT subscription) is not part of the Pi offline
        // catalog — always hand-maintained here.
        for m in openai_codex_offline_fallbacks() {
            if let Some(idx) = existing.get(&m.catalog_key()) {
                out[*idx] = m;
            } else {
                existing.insert(m.catalog_key(), out.len());
                out.push(m);
            }
        }
        // NVIDIA Integrate rows that the Pi snapshot does not yet list
        // (Lightning, Muse Glimmer, Laguna XS 2.1, Mistral-Nemotron).
        // Same request_compat as other NIM models.
        for m in nvidia_offline_fallbacks() {
            if let Some(idx) = existing.get(&m.catalog_key()) {
                out[*idx] = m;
            } else {
                existing.insert(m.catalog_key(), out.len());
                out.push(m);
            }
        }
        // Poolside-hosted inference (`inference.poolside.ai`) is not in Pi.
        for m in poolside_offline_fallbacks() {
            if let Some(idx) = existing.get(&m.catalog_key()) {
                out[*idx] = m;
            } else {
                existing.insert(m.catalog_key(), out.len());
                out.push(m);
            }
        }
        // Anthropic Claude (Pro/Max subscription OAuth) — hand-maintained.
        for m in anthropic_claude_offline_fallbacks() {
            if let Some(idx) = existing.get(&m.catalog_key()) {
                out[*idx] = m;
            } else {
                existing.insert(m.catalog_key(), out.len());
                out.push(m);
            }
        }
        // OpenRouter free-tier twins the Pi snapshot may omit (MiniMax M3
        // :free). Insert-only: never overwrite a Pi row that already exists.
        for m in openrouter_offline_fallbacks() {
            if existing.contains_key(&m.catalog_key()) {
                continue;
            }
            existing.insert(m.catalog_key(), out.len());
            out.push(m);
        }
        out
    });
    &MODELS
}

fn fallback_route(platform: PlatformId, backend: PlatformApiBackend) -> ProviderRouteSpec {
    let auth = if backend == PlatformApiBackend::GoogleGenerateContent {
        RouteAuth::XGoogApiKey
    } else if platform.uses_oauth() || platform == PlatformId::Nexus {
        RouteAuth::Bearer
    } else if backend == PlatformApiBackend::Messages || platform.uses_x_api_key() {
        RouteAuth::XApiKey
    } else {
        RouteAuth::Bearer
    };
    let mut headers = std::collections::BTreeMap::new();
    if backend == PlatformApiBackend::Messages {
        headers.insert(
            "anthropic-version".into(),
            ANTHROPIC_VERSION_HEADER_VALUE.into(),
        );
    }
    if platform == PlatformId::KimiCode {
        headers.insert("User-Agent".into(), "KimiCLI/1.5".into());
    }
    ProviderRouteSpec {
        path: backend.endpoint_path().into(),
        auth,
        headers,
        query_params: std::collections::BTreeMap::new(),
    }
}

/// Apply platform-wide request_compat fixes that older catalog rows may omit
/// (e.g. HYPER-LOCAL `supports_prompt_cache_key` / `agent_ready` for NVIDIA).
fn apply_catalog_compat_overrides(
    provider_id: &str,
    model_id: &str,
    mut compat: RequestCompat,
) -> RequestCompat {
    if provider_id == "nvidia"
        && let RequestCompat::ChatCompletions(ref mut chat) = compat
    {
        chat.supports_prompt_cache_key = false;
        chat.supports_store = false;
        chat.supports_developer_role = false;
        chat.supports_strict_mode = false;
        chat.supports_long_cache_retention = false;
        chat.max_tokens_field = MaxTokensField::MaxTokens;
        chat.agent_ready = nvidia_integrate_agent_ready(model_id);
        chat.supports_message_model_id = false;
        if model_id.contains("llama-3.1-70b") || model_id.contains("llama3-1-70b") {
            chat.max_parallel_tool_calls = Some(1);
        }
    }
    if provider_id == "poolside"
        && let RequestCompat::ChatCompletions(ref mut chat) = compat
    {
        chat.supports_prompt_cache_key = false;
        chat.supports_store = false;
        chat.supports_developer_role = false;
        chat.supports_strict_mode = false;
        chat.supports_long_cache_retention = false;
        chat.max_tokens_field = MaxTokensField::MaxTokens;
        chat.requires_reasoning_content_on_assistant_messages = true;
        chat.thinking_format = ThinkingFormat::QwenChatTemplate;
        chat.supports_message_model_id = false;
        chat.agent_ready = true;
    }
    compat
}

/// Clamp a requested max-completion budget to catalog and context limits.
///
/// Used when building sampler config and applying chat defaults so clients
/// never send `max_tokens` / `max_completion_tokens` above what the model
/// window allows (NVIDIA Nano 9B rejects values > `max_model_len`).
///
/// Returns `None` only when `requested` is `None` and there is no catalog
/// default to fill from. Context alone never invents a budget.
pub fn clamp_max_completion_tokens(
    requested: Option<u32>,
    catalog_max: Option<u32>,
    context_window: u64,
) -> Option<u32> {
    let ctx_cap = match u32::try_from(context_window) {
        Ok(v) if v > 0 => Some(v),
        _ => None,
    };
    let Some(mut value) = requested.or(catalog_max) else {
        return None;
    };
    if let Some(c) = catalog_max {
        value = value.min(c);
    }
    if let Some(ctx) = ctx_cap {
        value = value.min(ctx);
    }
    Some(value)
}

fn fallback_request_compat(
    platform: PlatformId,
    backend: PlatformApiBackend,
    model_id: &str,
) -> RequestCompat {
    match backend {
        PlatformApiBackend::ChatCompletions => {
            let mut compat = OpenAiCompletionsCompat::default();
            if matches!(platform, PlatformId::MoonshotCn | PlatformId::MoonshotAi) {
                compat.supports_store = false;
                compat.supports_developer_role = false;
                compat.supports_reasoning_effort = false;
                compat.max_tokens_field = MaxTokensField::MaxTokens;
                compat.supports_strict_mode = false;
                compat.supports_long_cache_retention = false;
            }
            if platform == PlatformId::DeepSeek {
                compat.supports_store = false;
                compat.supports_developer_role = false;
                compat.requires_reasoning_content_on_assistant_messages = true;
                compat.thinking_format = ThinkingFormat::DeepSeek;
            }
            // NVIDIA Integrate is a strict OpenAI-compatible gateway: unknown
            // body fields 400, max tokens must use `max_tokens`, and tool
            // loops are not yet agent-ready by default (see RC8 WP6).
            if platform == PlatformId::Nvidia {
                compat.supports_prompt_cache_key = false;
                compat.supports_store = false;
                compat.supports_developer_role = false;
                compat.supports_strict_mode = false;
                compat.supports_long_cache_retention = false;
                compat.max_tokens_field = MaxTokensField::MaxTokens;
                compat.agent_ready = nvidia_integrate_agent_ready(model_id);
                compat.supports_message_model_id = false;
                // Llama 3.1 70B on Integrate rejects multi tool-calls.
                if model_id.contains("llama-3.1-70b") || model_id.contains("llama3-1-70b") {
                    compat.max_parallel_tool_calls = Some(1);
                }
            }
            // Poolside-hosted Chat Completions: thinking on by default via
            // `chat_template_kwargs.enable_thinking`; preserve `reasoning_content`
            // on assistant follow-ups; `max_tokens` not `max_completion_tokens`.
            if platform == PlatformId::Poolside {
                compat.supports_prompt_cache_key = false;
                compat.supports_store = false;
                compat.supports_developer_role = false;
                compat.supports_strict_mode = false;
                compat.supports_long_cache_retention = false;
                compat.max_tokens_field = MaxTokensField::MaxTokens;
                compat.requires_reasoning_content_on_assistant_messages = true;
                compat.thinking_format = ThinkingFormat::QwenChatTemplate;
                compat.supports_message_model_id = false;
                compat.agent_ready = true;
            }
            RequestCompat::ChatCompletions(compat)
        }
        PlatformApiBackend::Responses => {
            let mut compat = OpenAiResponsesCompat::default();
            if platform == PlatformId::OpenAiCodex {
                compat.supports_openai_grammar_tools = true;
                compat.supports_tool_search = !model_id.contains("spark");
            }
            RequestCompat::Responses(compat)
        }
        PlatformApiBackend::Messages => {
            let mut compat = AnthropicMessagesCompat::default();
            if platform == PlatformId::KimiCode {
                compat.force_adaptive_thinking = true;
                compat.allow_empty_signature =
                    kimi_request_profile(model_id).is_some_and(kimi_allow_empty_thinking_signature);
            }
            if matches!(
                platform,
                PlatformId::Anthropic | PlatformId::AnthropicClaude
            ) {
                compat.supports_strict_tools = true;
                compat.supports_tool_references = !model_id.contains("haiku");
                if model_id.contains("opus-4-7") || model_id.contains("opus-4-8") {
                    compat.supports_temperature = false;
                }
            }
            RequestCompat::Messages(compat)
        }
        PlatformApiBackend::GoogleGenerateContent => {
            RequestCompat::GoogleGenerateContent(crate::GoogleGenerateContentCompat {
                supports_strict_tool_sampling: model_id.starts_with("gemini-3")
                    || model_id.starts_with("gemma-4")
                    || model_id == "gemini-flash-latest"
                    || model_id == "gemini-flash-lite-latest",
                thinking_level_map: BTreeMap::new(),
                thinking_budgets: BTreeMap::new(),
            })
        }
        PlatformApiBackend::BedrockConverseStream => {
            RequestCompat::BedrockConverseStream(crate::BedrockConverseStreamCompat {
                supports_strict_mode: false,
                thinking_level_map: BTreeMap::new(),
            })
        }
        PlatformApiBackend::PiMessages => RequestCompat::PiMessages(crate::PiMessagesCompat {}),
    }
}

/// OpenRouter rows the Pi snapshot omits. Catalog keys are
/// `openrouter/{wire_id}`.
fn openrouter_offline_fallbacks() -> Vec<BuiltinPlatformModel> {
    let mk = |model: &str, name: &str, desc: &str, ctx: u64, max_out: u32| BuiltinPlatformModel {
        provider: PlatformId::OpenRouter.provider_id(),
        model: model.into(),
        name: name.into(),
        description: desc.into(),
        context_window: ctx,
        supports_reasoning_effort: true,
        supported_in_api: true,
        catalog_available: true,
        picker_visible: true,
        eol: false,
        max_completion_tokens: Some(max_out),
        api_backend: PlatformApiBackend::ChatCompletions,
        base_url_override: None,
        request_compat: fallback_request_compat(
            PlatformId::OpenRouter,
            PlatformApiBackend::ChatCompletions,
            model,
        ),
        route: fallback_route(PlatformId::OpenRouter, PlatformApiBackend::ChatCompletions),
    };
    vec![
        mk(
            "minimax/minimax-m3:free",
            "MiniMax: MiniMax M3 (free)",
            "OpenRouter free MiniMax M3 (no credits required)",
            CTX_256K,
            131_072,
        ),
        mk(
            "thinkingmachines/inkling:free",
            "Thinking Machines: Inkling (free)",
            "OpenRouter free Inkling (no credits required)",
            524_288,
            131_072,
        ),
        mk(
            "nvidia/nemotron-3-ultra-550b-a55b",
            "NVIDIA: Nemotron 3 Ultra",
            "OpenRouter Nemotron 3 Ultra 550B (paid)",
            CTX_1M,
            65_536,
        ),
        mk(
            "nvidia/nemotron-3-ultra-550b-a55b:free",
            "NVIDIA: Nemotron 3 Ultra (free)",
            "OpenRouter free Nemotron 3 Ultra 550B",
            CTX_1M,
            65_536,
        ),
    ]
}

/// Anthropic Claude subscription models (`api.anthropic.com/v1/messages` via
/// OAuth bearer + `anthropic-beta: oauth-2025-04-20`).
///
/// These are picker seeds only — the user can `/model anthropic-claude/<id>`
/// with any Claude model id their subscription grants. Adjust the ids/context
/// windows here as Anthropic ships new models.
fn anthropic_claude_offline_fallbacks() -> Vec<BuiltinPlatformModel> {
    macro_rules! claude {
        ($id:literal, $name:literal, $desc:literal, $ctx:expr) => {
            BuiltinPlatformModel {
                provider: PlatformId::AnthropicClaude.provider_id(),
                model: $id.into(),
                name: $name.into(),
                description: $desc.into(),
                context_window: $ctx,
                supports_reasoning_effort: true,
                supported_in_api: false,
                catalog_available: true,
                picker_visible: true,
                eol: false,
                max_completion_tokens: MAX_TOK_32K,
                api_backend: PlatformApiBackend::Messages,
                base_url_override: None,
                request_compat: fallback_request_compat(
                    PlatformId::AnthropicClaude,
                    PlatformApiBackend::Messages,
                    $id,
                ),
                route: fallback_route(PlatformId::AnthropicClaude, PlatformApiBackend::Messages),
            }
        };
    }
    vec![
        claude!(
            "claude-opus-4-8",
            "Claude Opus 4.8 (subscription)",
            "Frontier Claude model for complex coding (Claude Pro/Max)",
            CTX_256K
        ),
        claude!(
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6 (subscription)",
            "Balanced Claude model for everyday coding (Claude Pro/Max)",
            CTX_1M
        ),
        claude!(
            "claude-haiku-4-5",
            "Claude Haiku 4.5 (subscription)",
            "Fast, affordable Claude model (Claude Pro/Max)",
            CTX_256K
        ),
    ]
}

/// OpenAI Codex subscription models (`chatgpt.com/backend-api/codex`).
///
/// Model lineup mirrors the Codex app-server `model/list` response and
/// official Pi `openai-codex` provider. The backend speaks the Responses
/// API with `store: false` + encrypted reasoning; GPT-5 family window is
/// 400k with 128k max output.
fn openai_codex_offline_fallbacks() -> Vec<BuiltinPlatformModel> {
    // Context window is for local budget UI only. Do **not** stamp
    // max_completion_tokens: ChatGPT Codex rejects `max_output_tokens`
    // (`{"detail":"Unsupported parameter: max_output_tokens"}`).
    const CTX_400K: u64 = 400_000;
    macro_rules! codex {
        ($id:literal, $name:literal, $desc:literal) => {
            BuiltinPlatformModel {
                provider: PlatformId::OpenAiCodex.provider_id(),
                model: $id.into(),
                name: $name.into(),
                description: $desc.into(),
                context_window: CTX_400K,
                supports_reasoning_effort: true,
                supported_in_api: false,
                catalog_available: true,
                picker_visible: true,
                eol: false,
                max_completion_tokens: None,
                api_backend: PlatformApiBackend::Responses,
                base_url_override: None,
                request_compat: fallback_request_compat(
                    PlatformId::OpenAiCodex,
                    PlatformApiBackend::Responses,
                    $id,
                ),
                route: fallback_route(PlatformId::OpenAiCodex, PlatformApiBackend::Responses),
            }
        };
    }
    vec![
        codex!(
            "gpt-5.6-sol",
            "GPT-5.6 Sol (ChatGPT)",
            "Latest frontier agentic coding model (ChatGPT subscription)"
        ),
        codex!(
            "gpt-5.6-terra",
            "GPT-5.6 Terra (ChatGPT)",
            "Balanced agentic coding model for everyday work (ChatGPT subscription)"
        ),
        codex!(
            "gpt-5.6-luna",
            "GPT-5.6 Luna (ChatGPT)",
            "Fast and affordable agentic coding model (ChatGPT subscription)"
        ),
        codex!(
            "gpt-5.5",
            "GPT-5.5 (ChatGPT)",
            "Frontier model for complex coding and research (ChatGPT subscription)"
        ),
        codex!(
            "gpt-5.4",
            "GPT-5.4 (ChatGPT)",
            "Strong model for everyday coding (ChatGPT subscription)"
        ),
        codex!(
            "gpt-5.4-mini",
            "GPT-5.4 Mini (ChatGPT)",
            "Small, fast model for simpler coding tasks (ChatGPT subscription)"
        ),
        codex!(
            "gpt-5.3-codex-spark",
            "GPT-5.3 Codex Spark (ChatGPT)",
            "Ultra-fast coding model (ChatGPT subscription)"
        ),
    ]
}

/// NVIDIA Integrate models missing from the imported Pi snapshot.
///
fn nvidia_wire_model_id(model: &str) -> String {
    match model {
        "muse-glimmer-30b" => "meta/muse-glimmer-30b".to_owned(),
        "laguna-xs-2.1" => "poolside/laguna-xs-2.1".to_owned(),
        "mistral-nemotron" => "mistralai/mistral-nemotron".to_owned(),
        "nemotron-3.5-lightning-30b-a3b" => "nvidia/nemotron-3.5-lightning-30b-a3b".to_owned(),
        "kimi-k3" => "moonshotai/kimi-k3".to_owned(),
        "deepseek-v4-pro-0813" => "deepseek-ai/deepseek-v4-pro-0813".to_owned(),
        "deepseek-v4-flash-0731" => "deepseek-ai/deepseek-v4-flash-0731".to_owned(),
        "deepseek-v4-pro" => "deepseek-ai/deepseek-v4-pro".to_owned(),
        "deepseek-v4-flash" => "deepseek-ai/deepseek-v4-flash".to_owned(),
        _ => model.to_owned(),
    }
}

/// Poolside-hosted inference expects the publisher-qualified model id
/// (`poolside/laguna-s-2.1`). Catalog keys stay `poolside/<short-id>`.
fn poolside_wire_model_id(model: &str) -> String {
    if model.starts_with("poolside/") {
        model.to_owned()
    } else {
        format!("poolside/{model}")
    }
}

/// Poolside-hosted Chat Completions (`https://inference.poolside.ai/v1`).
pub fn poolside_hosted_chat_compat(model_id: &str) -> RequestCompat {
    fallback_request_compat(
        PlatformId::Poolside,
        PlatformApiBackend::ChatCompletions,
        model_id,
    )
}

fn poolside_offline_fallbacks() -> Vec<BuiltinPlatformModel> {
    const MAX_OUT_32K: u32 = 32_768;
    let mk = |model: &str, name: &str, desc: &str, ctx: u64, eol: bool| BuiltinPlatformModel {
        provider: PlatformId::Poolside.provider_id(),
        model: model.into(),
        name: name.into(),
        description: desc.into(),
        context_window: ctx,
        supports_reasoning_effort: true,
        supported_in_api: false,
        catalog_available: !eol,
        picker_visible: !eol,
        eol,
        max_completion_tokens: Some(MAX_OUT_32K),
        api_backend: PlatformApiBackend::ChatCompletions,
        base_url_override: None,
        request_compat: fallback_request_compat(
            PlatformId::Poolside,
            PlatformApiBackend::ChatCompletions,
            model,
        ),
        route: fallback_route(PlatformId::Poolside, PlatformApiBackend::ChatCompletions),
    };
    vec![
        mk(
            "laguna-s-2.1",
            "Poolside Laguna S 2.1",
            "118B/8B-active MoE for long-horizon agentic coding (1M ctx, hosted Chat API)",
            CTX_1M,
            false,
        ),
        mk(
            "laguna-xs-2.1",
            "Poolside Laguna XS 2.1",
            "33B/3B-active MoE for fast agentic coding (256K ctx, hosted Chat API)",
            CTX_256K,
            false,
        ),
        // Poolside and OpenRouter both reject this id with HTTP 404; kept as an
        // EOL snapshot row so spawn/picker return a clear 410-class error.
        mk(
            "laguna-m.1",
            "Poolside Laguna M.1",
            "225B/23B-active MoE for complex multi-step coding (256K ctx, hosted Chat API)",
            CTX_256K,
            true,
        ),
    ]
}

/// NVIDIA Integrate Chat Completions compat (strict gateway).
///
/// Used for config.toml extras that occupy an `nvidia/…` catalog key without a
/// builtin `request_compat` snapshot. Hang models (Ultra, Llama 3.3 70B,
/// gpt-oss) stay `agent_ready=false`; current agentic NIMs (Lightning, Muse
/// Glimmer, DeepSeek V4, Kimi K3) are tool-ready.
pub fn nvidia_integrate_chat_compat(model_id: &str) -> RequestCompat {
    fallback_request_compat(
        PlatformId::Nvidia,
        PlatformApiBackend::ChatCompletions,
        model_id,
    )
}

/// True when the offline catalog marks this catalog key as HTTP 410 / withdrawn.
pub fn catalog_key_is_eol(catalog_key: &str) -> bool {
    let requested = catalog_key.trim();
    if requested.is_empty() {
        return false;
    }
    platform_builtin_models().iter().any(|model| {
        model.eol
            && (model.catalog_key() == requested
                || model.catalog_key().eq_ignore_ascii_case(requested))
    })
}

/// Poolside Laguna M.1 is rejected with HTTP 404 by both Poolside-hosted
/// inference and its OpenRouter clones. Provider returns 404 "No endpoints".
pub fn is_poolside_laguna_m1_eol_slug(requested: &str) -> bool {
    let lower = requested.trim().to_ascii_lowercase();
    if !lower.contains("laguna-m.1") && !lower.contains("laguna-m1") {
        return false;
    }
    lower.starts_with("poolside/") || lower.contains("/poolside/laguna-m")
}

/// NVIDIA Integrate GLM-5.2 (and aliases). Provider returns HTTP 410 Gone.
pub fn is_nvidia_glm_52_eol_slug(requested: &str) -> bool {
    let lower = requested.trim().to_ascii_lowercase();
    if !lower.contains("glm-5.2") {
        return false;
    }
    lower.starts_with("nvidia/")
        || lower.contains("/nvidia/z-ai/glm-5.2")
        || lower == "nvidia/z-ai/glm-5.2"
}

/// Whether an NVIDIA Integrate model is safe for tool-using agent loops.
///
/// Default is chat-only (Ultra / hang Llama / gpt-oss). Current build.nvidia.com
/// agentic NIMs opt in: Lightning, Muse Glimmer, Laguna XS, Mistral-Nemotron,
/// DeepSeek V4, Kimi K3.
pub fn nvidia_integrate_agent_ready(model_id: &str) -> bool {
    let m = model_id.to_ascii_lowercase();
    m.contains("nemotron-3.5-lightning")
        || m.contains("nemotron-3-5-lightning")
        || m.contains("muse-glimmer")
        || m.contains("laguna-xs")
        || m.contains("mistral-nemotron")
        || m.contains("deepseek-v4")
        || m.contains("kimi-k3")
        || m.contains("moonshotai/kimi")
}

/// Catalog keys follow `{provider}/{model}` so
/// `nvidia/nemotron-3.5-lightning-30b-a3b` becomes
/// `nvidia/nvidia/nemotron-3.5-lightning-30b-a3b` (same as Ultra/Super/Nano).
/// Third-party NIM ids keep their publisher prefix:
/// `meta/muse-glimmer-30b` → `nvidia/meta/muse-glimmer-30b`.
fn nvidia_offline_fallbacks() -> Vec<BuiltinPlatformModel> {
    const CTX_1M: u64 = 1_000_000;
    const CTX_256K: u64 = 262_144;
    const CTX_131K: u64 = 131_072;
    const CTX_128K: u64 = 128_000;
    const MAX_OUT_65K: u32 = 65_536;
    const MAX_OUT_32K: u32 = 32_768;
    const MAX_OUT_8K: u32 = 8_192;
    let mk = |model: &str, name: &str, desc: &str, ctx: u64, max_out: u32| BuiltinPlatformModel {
        provider: PlatformId::Nvidia.provider_id(),
        model: model.into(),
        name: name.into(),
        description: desc.into(),
        context_window: ctx,
        supports_reasoning_effort: true,
        supported_in_api: true,
        catalog_available: true,
        picker_visible: true,
        eol: false,
        max_completion_tokens: Some(max_out),
        api_backend: PlatformApiBackend::ChatCompletions,
        base_url_override: None,
        request_compat: fallback_request_compat(
            PlatformId::Nvidia,
            PlatformApiBackend::ChatCompletions,
            model,
        ),
        route: fallback_route(PlatformId::Nvidia, PlatformApiBackend::ChatCompletions),
    };
    let mut models = vec![
        mk(
            "nvidia/nemotron-3.5-lightning-30b-a3b",
            "Nemotron 3.5 Lightning 30B A3B",
            "Fast 30B/3B-active MoE on NVIDIA Integrate for specialized agentic tasks (1M ctx)",
            CTX_1M,
            MAX_OUT_65K,
        ),
        // Bare routing id so spawn_subagent(model=nvidia/nemotron-3.5-lightning-30b-a3b)
        // and [subagents.models] pins both resolve.
        mk(
            "nemotron-3.5-lightning-30b-a3b",
            "Nemotron 3.5 Lightning 30B A3B",
            "Fast 30B/3B-active MoE on NVIDIA Integrate for specialized agentic tasks (1M ctx)",
            CTX_1M,
            MAX_OUT_65K,
        ),
        mk(
            "meta/muse-glimmer-30b",
            "Muse Glimmer 30B",
            "Meta 30B dense multimodal agent model on NVIDIA Integrate (131K ctx)",
            CTX_131K,
            MAX_OUT_32K,
        ),
        mk(
            "muse-glimmer-30b",
            "Muse Glimmer 30B",
            "Meta 30B dense multimodal agent model on NVIDIA Integrate (131K ctx)",
            CTX_131K,
            MAX_OUT_32K,
        ),
        mk(
            "poolside/laguna-xs-2.1",
            "Poolside Laguna XS 2.1",
            "Poolside 33B/3B-active MoE for local agentic coding on NVIDIA Integrate (256K ctx)",
            CTX_256K,
            MAX_OUT_32K,
        ),
        mk(
            "laguna-xs-2.1",
            "Poolside Laguna XS 2.1",
            "Poolside 33B/3B-active MoE for local agentic coding on NVIDIA Integrate (256K ctx)",
            CTX_256K,
            MAX_OUT_32K,
        ),
        mk(
            "mistralai/mistral-nemotron",
            "Mistral-Nemotron",
            "Mistral + NVIDIA agentic coding / tool-calling model on NVIDIA Integrate (128K ctx)",
            CTX_128K,
            MAX_OUT_8K,
        ),
        mk(
            "mistral-nemotron",
            "Mistral-Nemotron",
            "Mistral + NVIDIA agentic coding / tool-calling model on NVIDIA Integrate (128K ctx)",
            CTX_128K,
            MAX_OUT_8K,
        ),
        mk(
            "moonshotai/kimi-k3",
            "Kimi K3",
            "Moonshot Kimi K3 hybrid MoE on NVIDIA Integrate for long-horizon coding and agents (1M ctx)",
            CTX_1M,
            MAX_OUT_65K,
        ),
        mk(
            "kimi-k3",
            "Kimi K3",
            "Moonshot Kimi K3 hybrid MoE on NVIDIA Integrate for long-horizon coding and agents (1M ctx)",
            CTX_1M,
            MAX_OUT_65K,
        ),
        mk(
            "deepseek-ai/deepseek-v4-pro-0813",
            "DeepSeek V4 Pro 0813",
            "DeepSeek V4 Pro (0813) 1.6T MoE on NVIDIA Integrate, 1M ctx",
            CTX_1M,
            MAX_OUT_65K,
        ),
        mk(
            "deepseek-v4-pro-0813",
            "DeepSeek V4 Pro 0813",
            "DeepSeek V4 Pro (0813) 1.6T MoE on NVIDIA Integrate, 1M ctx",
            CTX_1M,
            MAX_OUT_65K,
        ),
        mk(
            "deepseek-ai/deepseek-v4-flash-0731",
            "DeepSeek V4 Flash 0731",
            "DeepSeek V4 Flash (0731) 284B MoE on NVIDIA Integrate for long-context coding (256K ctx)",
            CTX_256K,
            MAX_OUT_65K,
        ),
        mk(
            "deepseek-v4-flash-0731",
            "DeepSeek V4 Flash 0731",
            "DeepSeek V4 Flash (0731) 284B MoE on NVIDIA Integrate for long-context coding (256K ctx)",
            CTX_256K,
            MAX_OUT_65K,
        ),
        mk(
            "deepseek-ai/deepseek-v4-pro",
            "DeepSeek V4 Pro",
            "DeepSeek V4 Pro on NVIDIA Integrate (1M ctx)",
            CTX_1M,
            MAX_OUT_65K,
        ),
        mk(
            "deepseek-ai/deepseek-v4-flash",
            "DeepSeek V4 Flash",
            "DeepSeek V4 Flash on NVIDIA Integrate (256K ctx)",
            CTX_256K,
            MAX_OUT_65K,
        ),
    ];
    // Bare slugs are compatibility aliases for config/subagent pins. Keep
    // them resolvable, but never present them as separate picker choices.
    for model in &mut models {
        if !model.model.contains('/') {
            // Bare ids are compatibility aliases only. NVIDIA's Integrate
            // endpoint requires the publisher-qualified id; keeping a bare
            // alias selectable causes provider 404s and makes certification
            // status ambiguous.
            model.picker_visible = false;
            model.catalog_available = false;
        }
    }
    models
}

fn kimi_moonshot_offline_fallbacks() -> Vec<BuiltinPlatformModel> {
    // ── Kimi Code subscription (api.kimi.com/coding/v1) ──────────────
    // Official Pi `kimi-coding` uses Anthropic Messages + forceAdaptiveThinking.
    // Canonical ids: k3, k2p7, kimi-for-coding-highspeed. Older open-platform
    // style ids remain as offline aliases for configs that still reference them.
    macro_rules! kimi {
        ($id:literal, $name:literal, $desc:literal, $ctx:expr, $effort:expr, $max_tok:expr) => {
            BuiltinPlatformModel {
                provider: PlatformId::KimiCode.provider_id(),
                model: $id.into(),
                name: $name.into(),
                description: $desc.into(),
                context_window: $ctx,
                supports_reasoning_effort: $effort,
                supported_in_api: false,
                catalog_available: true,
                picker_visible: true,
                eol: false,
                max_completion_tokens: $max_tok,
                api_backend: PlatformApiBackend::Messages,
                base_url_override: None,
                request_compat: fallback_request_compat(
                    PlatformId::KimiCode,
                    PlatformApiBackend::Messages,
                    $id,
                ),
                route: fallback_route(PlatformId::KimiCode, PlatformApiBackend::Messages),
            }
        };
    }
    let kimi_k3 = kimi!(
        "k3",
        "Kimi K3",
        "Official Pi catalog (kimi-coding); adaptive thinking; 1M context",
        CTX_1M,
        true,
        Some(131_072)
    );
    let kimi_k2p7 = kimi!(
        "k2p7",
        "Kimi K2.7 Code",
        "Official Pi catalog (kimi-coding); adaptive thinking; 256k context",
        CTX_256K,
        true,
        MAX_TOK_32K
    );
    let kimi_hs = kimi!(
        "kimi-for-coding-highspeed",
        "Kimi For Coding HighSpeed",
        "Official Pi catalog (kimi-coding); adaptive thinking; HyperSpeed",
        CTX_256K,
        true,
        MAX_TOK_32K
    );
    // Retired offline aliases (no longer listed in the picker):
    // kimi-k2.7-code, kimi-k2.7-code-highspeed, kimi-k2.6, kimi-k2.5.
    // Use k2p7 / kimi-for-coding-highspeed / k3 instead.
    let kimi_coding = kimi!(
        "kimi-for-coding",
        "Kimi for Coding",
        "Legacy Kimi Code subscription id (offline fallback)",
        CTX_256K,
        true,
        MAX_TOK_32K
    );

    // ── Moonshot open platform — current multimodal lineup ───────────
    // Official Model List (platform.kimi.ai/docs/models). Hidden until an API
    // key is configured; the shell's `apply_platform_credentials` reveals them.
    macro_rules! open {
        ($plat:ident, $id:literal, $name:literal, $desc:literal, $ctx:expr, $effort:expr) => {
            BuiltinPlatformModel {
                provider: PlatformId::$plat.provider_id(),
                model: $id.into(),
                name: $name.into(),
                description: $desc.into(),
                context_window: $ctx,
                supports_reasoning_effort: $effort,
                supported_in_api: false,
                catalog_available: true,
                picker_visible: true,
                eol: false,
                max_completion_tokens: MAX_TOK_32K,
                api_backend: PlatformApiBackend::ChatCompletions,
                base_url_override: None,
                request_compat: fallback_request_compat(
                    PlatformId::$plat,
                    PlatformApiBackend::ChatCompletions,
                    $id,
                ),
                route: fallback_route(PlatformId::$plat, PlatformApiBackend::ChatCompletions),
            }
        };
    }

    vec![
        // Subscription first (Pi canonical ids, then kimi-for-coding fallback).
        kimi_k3,
        kimi_k2p7,
        kimi_hs,
        kimi_coding,
        open!(
            MoonshotCn,
            "kimi-k3",
            "Kimi K3 (moonshot.cn)",
            "Flagship 1M context / always-thinking (offline fallback)",
            CTX_1M,
            true
        ),
        open!(
            MoonshotCn,
            "kimi-k2.7-code",
            "Kimi K2.7 Code (moonshot.cn)",
            "Dedicated coding model; thinking always on; 256k context",
            CTX_256K,
            false
        ),
        open!(
            MoonshotCn,
            "kimi-k2.7-code-highspeed",
            "Kimi K2.7 Code HighSpeed (moonshot.cn)",
            "HyperSpeed coding model (~180–260 tok/s); same quality as K2.7 Code",
            CTX_256K,
            false
        ),
        open!(
            MoonshotCn,
            "kimi-k2.6",
            "Kimi K2.6 (moonshot.cn)",
            "General multimodal; thinking on/off + preserved thinking; 256k",
            CTX_256K,
            false
        ),
        open!(
            MoonshotCn,
            "kimi-k2.5",
            "Kimi K2.5 (moonshot.cn)",
            "Multimodal agent model; thinking on/off (no preserved thinking); 256k",
            CTX_256K,
            false
        ),
        open!(
            MoonshotAi,
            "kimi-k3",
            "Kimi K3 (moonshot.ai)",
            "Flagship 1M context / always-thinking global (offline fallback)",
            CTX_1M,
            true
        ),
        open!(
            MoonshotAi,
            "kimi-k2.7-code",
            "Kimi K2.7 Code (moonshot.ai)",
            "Dedicated coding model; thinking always on; 256k context",
            CTX_256K,
            false
        ),
        open!(
            MoonshotAi,
            "kimi-k2.7-code-highspeed",
            "Kimi K2.7 Code HighSpeed (moonshot.ai)",
            "HyperSpeed coding model (~180–260 tok/s); same quality as K2.7 Code",
            CTX_256K,
            false
        ),
        open!(
            MoonshotAi,
            "kimi-k2.6",
            "Kimi K2.6 (moonshot.ai)",
            "General multimodal; thinking on/off + preserved thinking; 256k",
            CTX_256K,
            false
        ),
        open!(
            MoonshotAi,
            "kimi-k2.5",
            "Kimi K2.5 (moonshot.ai)",
            "Multimodal agent model; thinking on/off (no preserved thinking); 256k",
            CTX_256K,
            false
        ),
        // Deprecated aliases last.
        open!(
            MoonshotCn,
            "kimi-k2-turbo-preview",
            "Kimi K2 Turbo (deprecated, moonshot.cn)",
            "Deprecated K2 turbo alias — prefer kimi-k2.7-code / kimi-k2.6",
            CTX_256K,
            true
        ),
        open!(
            MoonshotCn,
            "kimi-k2-thinking-turbo",
            "Kimi K2 Thinking Turbo (deprecated, moonshot.cn)",
            "Deprecated K2 thinking alias — prefer kimi-k2.6 / kimi-k3",
            CTX_256K,
            true
        ),
        open!(
            MoonshotAi,
            "kimi-k2-turbo-preview",
            "Kimi K2 Turbo (deprecated, moonshot.ai)",
            "Deprecated K2 turbo alias — prefer kimi-k2.7-code / kimi-k2.6",
            CTX_256K,
            true
        ),
        open!(
            MoonshotAi,
            "kimi-k2-thinking-turbo",
            "Kimi K2 Thinking Turbo (deprecated, moonshot.ai)",
            "Deprecated K2 thinking alias — prefer kimi-k2.6 / kimi-k3",
            CTX_256K,
            true
        ),
    ]
}

// ── Per-model request-body profiles (platform.kimi.ai docs) ────────────────

/// How a Kimi/Moonshot model expects request fields.
///
/// Sources:
/// - platform.kimi.ai "Thinking Mode" + "K2.7 Code Parameters Differences" (Chat Completions)
/// - official Pi `kimi-coding` catalog: Anthropic Messages + `forceAdaptiveThinking`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KimiRequestProfile {
    /// `kimi-k3` / subscription `k3`: always thinks.
    /// - Chat Completions: top-level `reasoning_effort` (default `max`).
    /// - Messages (Pi): `thinking.type=adaptive` + `output_config.effort`
    ///   (`thinkingLevelMap` documents `max` only; default effort `max`).
    K3,
    /// Official Pi `k2p7` / `kimi-for-coding-highspeed` and open-platform
    /// `kimi-k2.7-code` (+ highspeed): thinking always on.
    /// - Chat Completions: fixed sampling; omit K2 `thinking` object.
    /// - Messages (Pi): `forceAdaptiveThinking` — adaptive + effort, no budget.
    K27Code,
    /// `kimi-k2.6`: `thinking.type` enabled/disabled; `thinking.keep` null|all.
    K26,
    /// `kimi-k2.5`: `thinking.type` only (no `keep`).
    K25,
    /// Older k2 turbo / thinking-turbo / kimi-for-coding — treat like always-thinking
    /// coding models (omit fixed-param fields; Messages adaptive when used).
    LegacyCoding,
}

/// Whether this profile uses Pi-style Anthropic adaptive thinking on the
/// Messages path (`thinking.type=adaptive` + `output_config.effort`).
pub fn kimi_force_adaptive_thinking(profile: KimiRequestProfile) -> bool {
    matches!(
        profile,
        KimiRequestProfile::K3 | KimiRequestProfile::K27Code | KimiRequestProfile::LegacyCoding
    )
}

/// Whether empty thinking `signature: ""` must be replayed (Pi
/// `compat.allowEmptySignature` for K3 / legacy kimi-for-coding).
pub fn kimi_allow_empty_thinking_signature(profile: KimiRequestProfile) -> bool {
    matches!(
        profile,
        KimiRequestProfile::K3 | KimiRequestProfile::LegacyCoding
    )
}

/// Classify a bare model id (or catalog key's model half) for request shaping.
pub fn kimi_request_profile(model_id: &str) -> Option<KimiRequestProfile> {
    // Accept both bare ids and `{platform}/{id}` catalog keys.
    let id = model_id
        .rsplit_once('/')
        .map(|(_, m)| m)
        .unwrap_or(model_id)
        .to_ascii_lowercase();
    match id.as_str() {
        "k3" | "kimi-k3" => Some(KimiRequestProfile::K3),
        // Official Pi subscription ids + open-platform aliases.
        "k2p7" | "kimi-k2.7-code" | "kimi-k2.7-code-highspeed" | "kimi-for-coding-highspeed" => {
            Some(KimiRequestProfile::K27Code)
        }
        "kimi-k2.6" => Some(KimiRequestProfile::K26),
        "kimi-k2.5" => Some(KimiRequestProfile::K25),
        "kimi-for-coding"
        | "kimi-k2-turbo-preview"
        | "kimi-k2-thinking-turbo"
        | "kimi-k2-thinking"
        | "kimi-k2-0905-preview"
        | "kimi-k2-0711-preview" => Some(KimiRequestProfile::LegacyCoding),
        _ if id.starts_with("kimi-k2.7") || id.starts_with("k2p7") => {
            Some(KimiRequestProfile::K27Code)
        }
        _ if id.starts_with("kimi-k2.6") => Some(KimiRequestProfile::K26),
        _ if id.starts_with("kimi-k2.5") => Some(KimiRequestProfile::K25),
        _ if id.starts_with("kimi-k3") || id == "k3" => Some(KimiRequestProfile::K3),
        _ => None,
    }
}

/// Kimi docs recommend ≥16k–32k max_tokens for thinking + tool loops.
pub const KIMI_DEFAULT_MAX_TOKENS: u32 = 32_768;

/// Whether the model rejects non-default temperature / top_p / penalties.
pub fn kimi_sampling_is_fixed(profile: KimiRequestProfile) -> bool {
    matches!(
        profile,
        KimiRequestProfile::K27Code | KimiRequestProfile::K26 | KimiRequestProfile::LegacyCoding
    )
}

// ── Live `/models` wire contract ────────────────────────────────────────────

/// Capability tags derived from the `/models` listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModelCapability {
    Thinking,
    AlwaysThinking,
    ImageIn,
    VideoIn,
}

/// One entry of `GET {base}/models` `data[]` (Kimi/Moonshot F4 shape).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WireModel {
    pub id: String,
    /// Nexus `/models` names this `context_window` (OpenAI) / `context_window`
    /// (Anthropic); accept both spellings so Nexus context is not lost.
    #[serde(default, alias = "context_window")]
    pub context_length: u64,
    /// Nexus reports max output via `max_completion_tokens` (OpenAI list) or
    /// `max_output_tokens` (Anthropic list). None for platforms that omit it.
    #[serde(default, alias = "max_completion_tokens", alias = "max_output_tokens")]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub supports_reasoning: bool,
    #[serde(default)]
    pub supports_image_in: bool,
    #[serde(default)]
    pub supports_video_in: bool,
    #[serde(default)]
    pub display_name: Option<String>,
    /// `"only"` → always-thinking (cannot disable).
    #[serde(default)]
    pub supports_thinking_type: Option<String>,
    #[serde(default)]
    pub think_efforts: Option<WireThinkEfforts>,
}

/// Selectable thinking levels (e.g. K3: low/high/max).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct WireThinkEfforts {
    #[serde(default)]
    pub support: bool,
    #[serde(default)]
    pub valid_efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WireModelsResponse {
    pub data: Vec<WireModel>,
}

impl WireModel {
    pub fn capabilities(&self) -> Vec<ModelCapability> {
        let mut caps = derive_capabilities(
            &self.id,
            self.supports_reasoning,
            self.supports_image_in,
            self.supports_video_in,
        );
        if self.supports_thinking_type.as_deref() == Some("only") {
            for cap in [ModelCapability::Thinking, ModelCapability::AlwaysThinking] {
                if !caps.contains(&cap) {
                    caps.push(cap);
                }
            }
            caps.sort();
        }
        caps
    }
}

pub fn derive_capabilities(
    id: &str,
    supports_reasoning: bool,
    supports_image_in: bool,
    supports_video_in: bool,
) -> Vec<ModelCapability> {
    let id_lower = id.to_lowercase();
    let mut caps = std::collections::BTreeSet::new();
    if supports_reasoning {
        caps.insert(ModelCapability::Thinking);
    }
    if id_lower.contains("thinking") {
        caps.insert(ModelCapability::Thinking);
        caps.insert(ModelCapability::AlwaysThinking);
    }
    if supports_image_in {
        caps.insert(ModelCapability::ImageIn);
    }
    if supports_video_in {
        caps.insert(ModelCapability::VideoIn);
    }
    // Current multimodal coding lineup + legacy k2* / Pi ids: thinking + vision.
    if id_lower.starts_with("kimi-k2")
        || id_lower == "k3"
        || id_lower.starts_with("kimi-k3")
        || id_lower == "k2p7"
        || id_lower.starts_with("k2p7")
        || id_lower == "kimi-for-coding"
        || id_lower == "kimi-for-coding-highspeed"
    {
        caps.insert(ModelCapability::Thinking);
        caps.insert(ModelCapability::ImageIn);
        caps.insert(ModelCapability::VideoIn);
    }
    // K2.7 Code / HighSpeed / K3 / Pi coding ids: thinking cannot be disabled.
    if id_lower.contains("k2.7-code")
        || id_lower == "k2p7"
        || id_lower.starts_with("k2p7")
        || id_lower == "k3"
        || id_lower.starts_with("kimi-k3")
        || id_lower == "kimi-for-coding"
        || id_lower == "kimi-for-coding-highspeed"
    {
        caps.insert(ModelCapability::AlwaysThinking);
    }
    caps.into_iter().collect()
}

/// Apply platform prefix filter. No-op when the platform has no filter.
pub fn filter_allowed_models(platform: PlatformId, models: Vec<WireModel>) -> Vec<WireModel> {
    let Some(prefixes) = platform.allowed_model_prefixes() else {
        return models;
    };
    models
        .into_iter()
        .filter(|m| prefixes.iter().any(|p| m.id.starts_with(p) || m.id == *p))
        .collect()
}

/// Alias for Phase-1 callers.
pub fn moonshot_builtin_models() -> &'static [BuiltinPlatformModel] {
    platform_builtin_models()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registry_covers_platform_enum_and_aliases() {
        let registry = provider_registry();
        assert_eq!(registry.version(), PLATFORM_REGISTRY_VERSION);
        assert!(registry.providers().len() >= PlatformId::ALL.len());
        assert!(!registry.source().is_empty());

        for platform in PlatformId::ALL {
            let spec = provider_spec(platform.as_str()).unwrap_or_else(|| {
                panic!("missing provider registry row for {}", platform.as_str())
            });
            assert_eq!(spec.id.as_str(), platform.as_str());
            assert_eq!(ProviderId::from(platform), spec.id);
            assert_eq!(spec.id.platform_id(), Some(platform));
            for alias in platform.aliases() {
                let alias_spec = provider_spec(alias)
                    .unwrap_or_else(|| panic!("missing alias {alias} for {}", platform.as_str()));
                assert_eq!(alias_spec.id, spec.id);
                assert_eq!(ProviderId::registered(alias), Some(spec.id.clone()));
            }
        }

        // These special providers were previously absent from the JSON file.
        for id in ["openai-codex", "nexus", "anthropic-claude", "poolside"] {
            assert!(provider_spec(id).is_some(), "missing {id}");
        }
    }

    #[test]
    fn wave1_registry_only_providers_have_complete_runtime_catalogs() {
        let expected = [
            ("ant-ling", 3usize),
            ("huggingface", 50),
            ("opencode", 58),
            ("qwen-token-plan", 15),
            ("qwen-token-plan-cn", 15),
            ("vercel-ai-gateway", 192),
            ("xiaomi", 6),
            ("xiaomi-token-plan-ams", 3),
            ("xiaomi-token-plan-cn", 3),
            ("xiaomi-token-plan-sgp", 3),
        ];
        for (provider_id, expected_count) in expected {
            let spec = provider_spec(provider_id).expect("Wave 1 provider is registered");
            assert_eq!(spec.status, ProviderStatus::Active);
            assert_eq!(spec.credentials.kind, ProviderCredentialKind::ApiKey);
            assert_eq!(spec.legacy_platform(), None);
            let rows: Vec<_> = platform_builtin_models()
                .iter()
                .filter(|model| model.provider.as_str() == provider_id)
                .collect();
            assert_eq!(rows.len(), expected_count, "{provider_id}");
            assert!(rows.iter().all(|model| !model.supported_in_api));
        }

        assert_eq!(
            parse_managed_model_key("ant-ling/Ling-2.6-flash"),
            Some((
                ProviderId::registered("ant-ling").unwrap(),
                "Ling-2.6-flash"
            ))
        );

        let opencode: Vec<_> = platform_builtin_models()
            .iter()
            .filter(|model| model.provider.as_str() == "opencode")
            .collect();
        assert_eq!(
            opencode
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::ChatCompletions)
                .count(),
            19
        );
        assert_eq!(
            opencode
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::Responses)
                .count(),
            20
        );
        assert_eq!(
            opencode
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::Messages)
                .count(),
            14
        );
        assert_eq!(
            opencode
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::GoogleGenerateContent)
                .count(),
            5
        );

        let bedrock: Vec<_> = platform_builtin_models()
            .iter()
            .filter(|model| model.provider.as_str() == "amazon-bedrock")
            .collect();
        assert_eq!(bedrock.len(), 114);
        assert!(bedrock.iter().all(|model| {
            model.api_backend == PlatformApiBackend::BedrockConverseStream
                && model.route.path == "model/{model}/converse-stream"
                && matches!(
                    model.request_compat,
                    RequestCompat::BedrockConverseStream(_)
                )
        }));
        let bedrock_spec = provider_spec("amazon-bedrock").expect("Bedrock registry row");
        assert_eq!(bedrock_spec.status, ProviderStatus::Active);
        assert_eq!(bedrock_spec.adapter, AdapterKind::BedrockConverseStream);
        let parity: serde_json::Value =
            serde_json::from_str(include_str!("../pi_provider_parity.json")).expect("parity json");
        let bedrock_parity = parity["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["pi_provider"] == "amazon-bedrock")
            .expect("bedrock parity row");
        assert_eq!(bedrock_parity["status"], "supported");
        assert_eq!(bedrock_parity["shared_model_count"], 114);
        assert_eq!(bedrock_parity["missing_from_hyper_count"], 0);
        let fable = bedrock
            .iter()
            .find(|model| model.model == "anthropic.claude-fable-5")
            .expect("Fable 5 Bedrock row");
        let RequestCompat::BedrockConverseStream(compat) = &fable.request_compat else {
            panic!("Fable 5 uses Bedrock compat")
        };
        assert_eq!(
            compat.thinking_level_map.get("xhigh"),
            Some(&Some("xhigh".into()))
        );

        let vercel: Vec<_> = platform_builtin_models()
            .iter()
            .filter(|model| model.provider.as_str() == "vercel-ai-gateway")
            .collect();
        assert!(vercel.iter().all(|model| {
            model.api_backend == PlatformApiBackend::Messages
                && model.route.auth == RouteAuth::XApiKey
                && model
                    .route
                    .headers
                    .get("anthropic-version")
                    .map(String::as_str)
                    == Some(ANTHROPIC_VERSION_HEADER_VALUE)
        }));

        let ant_ling = platform_builtin_models()
            .iter()
            .find(|model| model.provider.as_str() == "ant-ling")
            .expect("Ant Ling model");
        assert!(matches!(
            &ant_ling.request_compat,
            RequestCompat::ChatCompletions(compat)
                if compat.thinking_format == ThinkingFormat::AntLing
        ));
    }

    #[test]
    fn strict_catalog_validation_preserves_every_embedded_row() {
        let raw: CatalogFile =
            serde_json::from_str(PLATFORM_CATALOG_JSON).expect("embedded catalog parses");
        let parsed = parse_platform_catalog(PLATFORM_CATALOG_JSON, provider_registry())
            .expect("embedded catalog validates");
        assert_eq!(parsed.len(), raw.models.len());
    }

    #[test]
    fn catalog_v3_carries_protocol_compat_and_explicit_routes() {
        let models = platform_builtin_models();
        let openai = models
            .iter()
            .find(|model| model.catalog_key() == "openai/gpt-5")
            .expect("openai/gpt-5");
        assert!(matches!(openai.request_compat, RequestCompat::Responses(_)));
        assert_eq!(openai.route.path, "responses");
        assert_eq!(openai.route.auth, RouteAuth::Bearer);

        let deepseek = models
            .iter()
            .find(|model| model.catalog_key() == "deepseek/deepseek-v4-pro")
            .expect("deepseek/deepseek-v4-pro");
        let RequestCompat::ChatCompletions(compat) = &deepseek.request_compat else {
            panic!("DeepSeek must use Chat Completions compat")
        };
        assert_eq!(compat.thinking_format, ThinkingFormat::DeepSeek);
        assert_eq!(compat.max_tokens_field, MaxTokensField::MaxCompletionTokens);
        assert!(compat.requires_reasoning_content_on_assistant_messages);
        assert_eq!(deepseek.route.path, "chat/completions");
        assert_eq!(deepseek.route.auth, RouteAuth::Bearer);

        let mistral: Vec<_> = models
            .iter()
            .filter(|model| model.legacy_platform() == Some(PlatformId::Mistral))
            .collect();
        assert_eq!(mistral.len(), 30, "Pi Mistral catalog count");
        assert!(mistral.iter().all(|model| {
            model.api_backend == PlatformApiBackend::ChatCompletions
                && model.route.path == "chat/completions"
                && model.route.auth == RouteAuth::Bearer
                && model.resolved_base_url() == "https://api.mistral.ai/v1"
        }));
        assert!(mistral
            .iter()
            .any(|model| model.model == "mistral-small-2603" && model.supports_reasoning_effort));

        let minimax = models
            .iter()
            .find(|model| model.catalog_key() == "minimax/MiniMax-M2.7")
            .expect("minimax/MiniMax-M2.7");
        assert!(matches!(minimax.request_compat, RequestCompat::Messages(_)));
        assert_eq!(minimax.route.auth, RouteAuth::XApiKey);
        assert_eq!(
            minimax
                .route
                .headers
                .get("anthropic-version")
                .map(String::as_str),
            Some(ANTHROPIC_VERSION_HEADER_VALUE)
        );
    }

    #[test]
    fn strict_catalog_validation_rejects_unknown_provider_and_backend() {
        let mut unknown_provider: serde_json::Value =
            serde_json::from_str(PLATFORM_CATALOG_JSON).unwrap();
        unknown_provider["models"][0]["platform"] = serde_json::json!("not-a-provider");
        let error = validate_provider_assets(
            PLATFORM_REGISTRY_JSON,
            &serde_json::to_string(&unknown_provider).unwrap(),
        )
        .expect_err("unknown provider must fail");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.contains("unknown provider `not-a-provider`")),
            "{error}"
        );

        let mut unknown_backend: serde_json::Value =
            serde_json::from_str(PLATFORM_CATALOG_JSON).unwrap();
        unknown_backend["models"][0]["api_backend"] = serde_json::json!("magic-stream");
        let error = validate_provider_assets(
            PLATFORM_REGISTRY_JSON,
            &serde_json::to_string(&unknown_backend).unwrap(),
        )
        .expect_err("unknown backend must fail");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.contains("unknown api_backend `magic-stream`")),
            "{error}"
        );
    }

    #[test]
    fn strict_registry_validation_collects_metadata_errors() {
        let mut registry: serde_json::Value = serde_json::from_str(PLATFORM_REGISTRY_JSON).unwrap();
        registry["providers"][0]["display_name"] = serde_json::json!(" ");
        registry["providers"][0]["aliases"] = serde_json::json!(["kimi-coding", "kimi-coding"]);
        let error = validate_provider_assets(
            &serde_json::to_string(&registry).unwrap(),
            PLATFORM_CATALOG_JSON,
        )
        .expect_err("invalid registry metadata must fail");
        assert!(error.issues().len() >= 2, "{error}");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.contains("display_name")),
            "{error}"
        );
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.contains("duplicate alias")),
            "{error}"
        );
    }

    #[test]
    fn strict_registry_json_rejects_unknown_fields() {
        let mut registry: serde_json::Value = serde_json::from_str(PLATFORM_REGISTRY_JSON).unwrap();
        registry["providers"][0]["credential_typo"] = serde_json::json!(true);
        let error = validate_provider_assets(
            &serde_json::to_string(&registry).unwrap(),
            PLATFORM_CATALOG_JSON,
        )
        .expect_err("unknown registry fields must fail");
        assert!(
            error
                .to_string()
                .contains("unknown field `credential_typo`")
        );
    }

    #[test]
    fn registry_only_provider_loads_without_platform_enum_variant() {
        let mut registry: serde_json::Value = serde_json::from_str(PLATFORM_REGISTRY_JSON).unwrap();
        let providers = registry["providers"].as_array_mut().unwrap();
        let mut provider = providers
            .iter()
            .find(|row| row["id"] == "deepseek")
            .cloned()
            .expect("deepseek registry template");
        provider["id"] = serde_json::json!("test-provider");
        provider["pi_id"] = serde_json::json!("test-provider");
        provider["display_name"] = serde_json::json!("Test Provider");
        provider["aliases"] = serde_json::json!([]);
        provider["default_base_url"] = serde_json::json!("https://api.test-provider.invalid/v1");
        provider["base_url_env_keys"] = serde_json::json!(["GROK_TEST_PROVIDER_BASE_URL"]);
        provider["credentials"]["env_keys"] = serde_json::json!(["GROK_TEST_PROVIDER_API_KEY"]);
        providers.push(provider);

        let mut catalog: serde_json::Value = serde_json::from_str(PLATFORM_CATALOG_JSON).unwrap();
        let models = catalog["models"].as_array_mut().unwrap();
        let mut model = models
            .iter()
            .find(|row| row["platform"] == "deepseek")
            .cloned()
            .expect("deepseek catalog template");
        model["platform"] = serde_json::json!("test-provider");
        model["model"] = serde_json::json!("test-model");
        model["name"] = serde_json::json!("Test Model");
        model["description"] = serde_json::json!("Synthetic registry-only model");
        models.push(model);

        let assets = load_provider_assets(
            &serde_json::to_string(&registry).unwrap(),
            &serde_json::to_string(&catalog).unwrap(),
        )
        .expect("registry-only provider must validate");
        let spec = assets.registry.find("test-provider").unwrap();
        assert_eq!(spec.legacy_platform(), None);
        let model = assets
            .catalog_models
            .iter()
            .find(|model| model.catalog_key() == "test-provider/test-model")
            .expect("registry-only catalog row must be retained");
        assert_eq!(model.provider, spec.id);
        assert_eq!(model.legacy_platform(), None);
    }

    #[test]
    fn platform_roundtrip() {
        for p in PlatformId::ALL {
            assert_eq!(PlatformId::parse(p.as_str()), Some(p));
            assert!(!p.base_url().is_empty());
        }
        assert!(PlatformId::KimiCode.uses_oauth());
        assert!(PlatformId::OpenAiCodex.uses_oauth());
        assert!(!PlatformId::MoonshotCn.uses_oauth());
        assert_eq!(
            PlatformId::KimiCode.api_key_env_names(),
            &[
                KIMI_CODE_API_KEY_ENV,
                KIMI_API_KEY_ENV,
                KIMI_API_KEY_ALIAS_ENV,
            ]
        );
        assert!(PlatformId::OpenAiCodex.api_key_env_names().is_empty());
        assert!(!PlatformId::MoonshotCn.api_key_env_names().is_empty());
        assert_eq!(
            PlatformId::OpenAiCodex.oauth_host().as_deref(),
            Some("https://auth.openai.com")
        );
        assert_eq!(
            PlatformId::OpenAiCodex.base_url(),
            "https://chatgpt.com/backend-api/codex"
        );
        assert_eq!(
            PlatformId::OpenAiCodex.models_list_url(),
            "https://chatgpt.com/backend-api/codex/models"
        );
        assert!(
            PlatformId::OpenAiCodex
                .base_url_matches("https://chatgpt.com/backend-api/codex/responses")
        );
        assert!(!PlatformId::OpenAiCodex.base_url_matches("https://api.openai.com/v1"));
        assert_eq!(PlatformId::parse("openai"), Some(PlatformId::OpenAi));
        assert_eq!(PlatformId::parse("anthropic"), Some(PlatformId::Anthropic));
        assert!(PlatformId::Anthropic.uses_x_api_key());
        assert!(!PlatformId::OpenAi.uses_x_api_key());
        // Ollama Cloud live-syncs its `/models` listing once OLLAMA_API_KEY resolves.
        assert!(PlatformId::Ollama.live_models_list_enabled());
        assert!(PlatformId::KimiCode.live_models_list_enabled());
        assert!(!PlatformId::DeepSeek.live_models_list_enabled());
    }

    #[test]
    fn nexus_is_bearer_byok_with_live_discovery() {
        assert_eq!(PlatformId::parse("nexus"), Some(PlatformId::Nexus));
        assert_eq!(PlatformId::Nexus.as_str(), "nexus");
        assert_eq!(PlatformId::Nexus.display_name(), "Nexus");
        assert!(!PlatformId::Nexus.uses_oauth());
        // Bearer even for the Messages backend (Nexus is not Anthropic-native).
        assert!(!PlatformId::Nexus.uses_x_api_key());
        assert!(PlatformId::Nexus.live_models_list_enabled());
        assert_eq!(PlatformId::Nexus.base_url(), NEXUS_BASE_URL_DEFAULT);
        assert_eq!(
            PlatformId::Nexus.api_key_env_names(),
            &["GROK_NEXUS_API_KEY", "NEXUS_API_KEY"]
        );
    }

    #[test]
    fn opencode_go_is_api_key_subscription_with_mixed_protocol_catalog() {
        assert_eq!(
            PlatformId::parse("opencode-go"),
            Some(PlatformId::OpenCodeGo)
        );
        assert_eq!(
            PlatformId::parse("opencodego"),
            Some(PlatformId::OpenCodeGo)
        );
        assert_eq!(PlatformId::OpenCodeGo.display_name(), "OpenCode Go");
        assert_eq!(
            PlatformId::OpenCodeGo.base_url(),
            OPENCODE_GO_BASE_URL_DEFAULT
        );
        assert_eq!(
            PlatformId::OpenCodeGo.api_key_env_names(),
            &[OPENCODE_GO_API_KEY_ENV, OPENCODE_API_KEY_ENV]
        );
        assert!(!PlatformId::OpenCodeGo.uses_oauth());
        assert!(!PlatformId::OpenCodeGo.uses_x_api_key());
        assert_eq!(
            provider_spec("opencode-go")
                .unwrap()
                .credential_storage_group(),
            "opencode"
        );
        assert_eq!(
            provider_spec("opencode")
                .unwrap()
                .credential_storage_group(),
            "opencode"
        );
        assert!(!PlatformId::OpenCodeGo.live_models_list_enabled());
        assert!(
            PlatformId::OpenCodeGo
                .setup_hint()
                .contains("/providers opencode-go <api_key>")
        );

        let models: Vec<_> = platform_builtin_models()
            .iter()
            .filter(|model| model.legacy_platform() == Some(PlatformId::OpenCodeGo))
            .collect();
        assert_eq!(models.len(), 16, "current non-deprecated Go catalog");
        assert!(
            models.iter().all(|model| !model.supports_reasoning_effort),
            "Go docs do not define a portable reasoning-effort wire contract"
        );
        assert!(
            models
                .iter()
                .all(|model| model.resolved_base_url() == OPENCODE_GO_BASE_URL_DEFAULT)
        );

        let chat: std::collections::HashSet<_> = models
            .iter()
            .filter(|model| model.api_backend == PlatformApiBackend::ChatCompletions)
            .map(|model| model.model.as_str())
            .collect();
        assert_eq!(
            chat,
            std::collections::HashSet::from([
                "deepseek-v4-flash",
                "deepseek-v4-pro",
                "glm-5.1",
                "glm-5.2",
                "grok-4.5",
                "hy3",
                "kimi-k2.6",
                "kimi-k2.7-code",
                "kimi-k3",
                "mimo-v2.5",
                "mimo-v2.5-pro",
            ])
        );

        let messages: std::collections::HashSet<_> = models
            .iter()
            .filter(|model| model.api_backend == PlatformApiBackend::Messages)
            .map(|model| model.model.as_str())
            .collect();
        assert_eq!(
            messages,
            std::collections::HashSet::from([
                "minimax-m2.7",
                "minimax-m3",
                "qwen3.6-plus",
                "qwen3.7-max",
                "qwen3.7-plus",
            ])
        );
    }

    #[test]
    fn nexus_root_normalizes_client_view_suffixes() {
        for raw in [
            "https://nexuscore.now",
            "https://nexuscore.now/",
            "https://nexuscore.now/openai",
            "https://nexuscore.now/openai/",
            "https://nexuscore.now/openai/v1",
            "https://nexuscore.now/v1",
        ] {
            assert_eq!(nexus_normalize_root(raw), "https://nexuscore.now", "{raw}");
        }
        // Empty falls back to the compiled default.
        assert_eq!(nexus_normalize_root("   "), NEXUS_BASE_URL_DEFAULT);
        // A self-hosted root with a path prefix is preserved (minus client view).
        assert_eq!(
            nexus_normalize_root("https://gw.example.com/nexus/openai/v1"),
            "https://gw.example.com/nexus"
        );
    }

    #[test]
    fn nexus_per_backend_bases_match_gateway() {
        let r = "https://nexuscore.now";
        assert_eq!(nexus_chat_base(r), "https://nexuscore.now/openai/v1");
        assert_eq!(nexus_messages_base(r), "https://nexuscore.now/v1");
        assert_eq!(nexus_responses_base(r), "https://nexuscore.now/v1");
    }

    #[test]
    fn wire_model_accepts_context_window_alias() {
        let wire: WireModel = serde_json::from_value(serde_json::json!({
            "id": "claude-opus-4-8",
            "context_window": 1_048_576,
            "max_output_tokens": 128_000
        }))
        .expect("nexus wire model parses");
        assert_eq!(wire.context_length, 1_048_576);
        assert_eq!(wire.max_output_tokens, Some(128_000));
    }

    #[test]
    fn azure_runtime_resolves_resource_version_and_pi_deployment_map() {
        let provider = provider_spec("azure-openai-responses").expect("Azure provider");
        let static_query = BTreeMap::from([("api-version".to_string(), "v1".to_string())]);
        let runtime = provider.resolve_runtime_with(
            &provider.default_base_url,
            "gpt-5",
            &static_query,
            |name| match name {
                "AZURE_OPENAI_RESOURCE_NAME" => Some("my-resource".into()),
                "AZURE_OPENAI_API_VERSION" => Some("2026-07-01-preview".into()),
                "AZURE_OPENAI_DEPLOYMENT_NAME_MAP" => {
                    Some("gpt-4=legacy, gpt-5=my-gpt5-prod, malformed".into())
                }
                _ => None,
            },
        );
        assert!(runtime.ready);
        assert_eq!(
            runtime.base_url,
            "https://my-resource.openai.azure.com/openai/v1"
        );
        assert_eq!(
            runtime.query_params.get("api-version").map(String::as_str),
            Some("2026-07-01-preview")
        );
        assert_eq!(runtime.wire_model_id, "my-gpt5-prod");
    }

    #[test]
    fn azure_base_normalization_matches_locked_pi_rules() {
        for (raw, expected) in [
            (
                "https://demo.openai.azure.com",
                "https://demo.openai.azure.com/openai/v1",
            ),
            (
                "https://demo.openai.azure.com/openai",
                "https://demo.openai.azure.com/openai/v1",
            ),
            (
                "https://demo.openai.azure.com/openai/v1/responses",
                "https://demo.openai.azure.com/openai/v1",
            ),
            (
                "https://demo.cognitiveservices.azure.com/",
                "https://demo.cognitiveservices.azure.com/openai/v1",
            ),
            (
                "https://demo.ai.azure.com",
                "https://demo.ai.azure.com/openai/v1",
            ),
        ] {
            assert_eq!(
                normalize_azure_openai_base_url(raw).as_deref(),
                Some(expected),
                "{raw}"
            );
        }
        assert_eq!(
            normalize_azure_openai_base_url("https://proxy.example.com/custom").as_deref(),
            Some("https://proxy.example.com/custom")
        );
        assert!(normalize_azure_openai_base_url("not-a-url").is_none());
    }

    #[test]
    fn cloudflare_template_is_locked_until_safe_ids_resolve() {
        let provider = provider_spec("cloudflare-ai-gateway").expect("Cloudflare provider");
        let base = "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai";
        let missing = provider.resolve_runtime_with(base, "gpt-5", &BTreeMap::new(), |_| None);
        assert!(!missing.ready);

        let unsafe_value =
            provider.resolve_runtime_with(base, "gpt-5", &BTreeMap::new(), |name| match name {
                "CLOUDFLARE_ACCOUNT_ID" => Some("account/escape".into()),
                "CLOUDFLARE_GATEWAY_ID" => Some("gateway".into()),
                _ => None,
            });
        assert!(!unsafe_value.ready);

        let ready =
            provider.resolve_runtime_with(base, "gpt-5", &BTreeMap::new(), |name| match name {
                "CLOUDFLARE_ACCOUNT_ID" => Some("account-123".into()),
                "CLOUDFLARE_GATEWAY_ID" => Some("gateway_456".into()),
                _ => None,
            });
        assert!(ready.ready);
        assert_eq!(
            ready.base_url,
            "https://gateway.ai.cloudflare.com/v1/account-123/gateway_456/openai"
        );
    }

    #[test]
    fn wave_two_catalog_has_all_azure_and_cloudflare_routes() {
        let models = platform_builtin_models();
        let azure: Vec<_> = models
            .iter()
            .filter(|model| model.provider.as_str() == "azure-openai-responses")
            .collect();
        assert_eq!(azure.len(), 38);
        assert!(azure.iter().all(|model| {
            model.api_backend == PlatformApiBackend::Responses
                && model.route.auth == RouteAuth::ApiKey
                && model
                    .route
                    .query_params
                    .get("api-version")
                    .map(String::as_str)
                    == Some("v1")
        }));

        let gateway: Vec<_> = models
            .iter()
            .filter(|model| model.provider.as_str() == "cloudflare-ai-gateway")
            .collect();
        assert_eq!(gateway.len(), 42);
        assert!(
            gateway
                .iter()
                .all(|model| model.route.auth == RouteAuth::CfAigAuthorization)
        );
        assert_eq!(
            gateway
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::Messages)
                .count(),
            18
        );
        assert_eq!(
            gateway
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::Responses)
                .count(),
            19
        );
        assert_eq!(
            gateway
                .iter()
                .filter(|model| model.api_backend == PlatformApiBackend::ChatCompletions)
                .count(),
            5
        );

        let workers: Vec<_> = models
            .iter()
            .filter(|model| model.provider.as_str() == "cloudflare-workers-ai")
            .collect();
        assert_eq!(workers.len(), 13);
        assert!(workers.iter().all(|model| {
            model.api_backend == PlatformApiBackend::ChatCompletions
                && model.route.auth == RouteAuth::Bearer
        }));
    }

    #[test]
    fn strict_catalog_rejects_auth_header_conflicts_and_unknown_templates() {
        let mut catalog: serde_json::Value = serde_json::from_str(PLATFORM_CATALOG_JSON).unwrap();
        let rows = catalog["models"].as_array_mut().unwrap();
        let gateway = rows
            .iter_mut()
            .find(|row| row["platform"] == "cloudflare-ai-gateway")
            .unwrap();
        gateway["route"]["headers"]["Authorization"] = serde_json::json!("Bearer static");
        let error = validate_provider_assets(
            PLATFORM_REGISTRY_JSON,
            &serde_json::to_string(&catalog).unwrap(),
        )
        .expect_err("static auth headers must fail validation");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.contains("typed authentication metadata")),
            "{error}"
        );

        let mut registry: serde_json::Value = serde_json::from_str(PLATFORM_REGISTRY_JSON).unwrap();
        let azure = registry["providers"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["id"] == "azure-openai-responses")
            .unwrap();
        azure["default_base_url"] =
            serde_json::json!("https://{UNDECLARED_RESOURCE}.openai.azure.com/openai/v1");
        let error = validate_provider_assets(
            &serde_json::to_string(&registry).unwrap(),
            PLATFORM_CATALOG_JSON,
        )
        .expect_err("undeclared URL placeholders must fail validation");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.contains("UNDECLARED_RESOURCE")),
            "{error}"
        );
    }

    #[test]
    fn managed_key_roundtrip() {
        let key = PlatformId::KimiCode.managed_model_key("kimi-for-coding");
        assert_eq!(key, "kimi-code/kimi-for-coding");
        assert_eq!(
            parse_managed_model_key(&key),
            Some((PlatformId::KimiCode.provider_id(), "kimi-for-coding"))
        );
    }

    #[test]
    fn base_url_matches_host() {
        assert!(PlatformId::KimiCode.base_url_matches("https://api.kimi.com/coding/v1"));
        assert!(PlatformId::KimiCode.base_url_matches("https://api.kimi.com/coding/v1/chat"));
        assert!(!PlatformId::KimiCode.base_url_matches("https://api.moonshot.cn/v1"));
    }

    #[test]
    fn normalize_kimi_code_base_url_adds_v1_for_pi_style() {
        assert_eq!(
            normalize_kimi_code_base_url("https://api.kimi.com/coding"),
            "https://api.kimi.com/coding/v1"
        );
        assert_eq!(
            normalize_kimi_code_base_url("https://api.kimi.com/coding/"),
            "https://api.kimi.com/coding/v1"
        );
        // Already Grok-style — leave alone.
        assert_eq!(
            normalize_kimi_code_base_url("https://api.kimi.com/coding/v1"),
            "https://api.kimi.com/coding/v1"
        );
        assert_eq!(
            normalize_kimi_code_base_url("https://api.kimi.com/coding/v1/"),
            "https://api.kimi.com/coding/v1"
        );
    }

    #[test]
    fn normalize_anthropic_sdk_base_url_adds_version_path() {
        assert_eq!(
            normalize_anthropic_sdk_base_url("https://gateway.example.com/coding"),
            "https://gateway.example.com/coding/v1"
        );
        assert_eq!(
            normalize_anthropic_sdk_base_url("https://gateway.example.com/coding/"),
            "https://gateway.example.com/coding/v1"
        );
        assert_eq!(
            normalize_anthropic_sdk_base_url("https://gateway.example.com/coding/v1"),
            "https://gateway.example.com/coding/v1"
        );
        assert_eq!(
            normalize_anthropic_sdk_base_url("https://gateway.example.com/coding/v1/messages"),
            "https://gateway.example.com/coding/v1"
        );
    }

    #[test]
    fn anthropic_base_url_honors_claude_alias_and_grok_precedence() {
        let claude_style = PlatformId::Anthropic.base_url_with(|name| {
            (name == ANTHROPIC_BASE_URL_ALIAS_ENV)
                .then(|| "https://gateway.example.com/coding/".to_string())
        });
        assert_eq!(claude_style, "https://gateway.example.com/coding/v1");

        let grok_override = PlatformId::Anthropic.base_url_with(|name| match name {
            ANTHROPIC_BASE_URL_ENV => Some("https://grok.example.com/custom/v1".to_string()),
            ANTHROPIC_BASE_URL_ALIAS_ENV => Some("https://ignored.example.com/coding".to_string()),
            _ => None,
        });
        assert_eq!(grok_override, "https://grok.example.com/custom/v1");
        assert_eq!(
            PlatformId::Anthropic.base_url_with(|_| None),
            ANTHROPIC_BASE_URL_DEFAULT
        );
    }

    #[test]
    fn anthropic_auth_token_precedes_standard_api_key_alias() {
        assert_eq!(
            PlatformId::Anthropic.api_key_env_names(),
            &[
                ANTHROPIC_API_KEY_ENV,
                ANTHROPIC_AUTH_TOKEN_ENV,
                ANTHROPIC_API_KEY_ALIAS_ENV,
            ]
        );
    }

    #[test]
    fn builtins_have_unique_catalog_keys() {
        let mut keys = std::collections::HashSet::new();
        for m in platform_builtin_models() {
            assert!(
                keys.insert(m.catalog_key()),
                "duplicate {}",
                m.catalog_key()
            );
        }
    }

    #[test]
    fn zai_coding_platform_is_international_code_plan() {
        assert_eq!(PlatformId::parse("zai-coding"), Some(PlatformId::ZaiCoding));
        assert_eq!(
            PlatformId::parse("zai-code-plan"),
            Some(PlatformId::ZaiCoding)
        );
        assert_eq!(PlatformId::ZaiCoding.as_str(), "zai-coding");
        assert_eq!(PlatformId::ZaiCoding.display_name(), "Z.AI Coding Plan");
        assert_eq!(
            PlatformId::ZaiCoding.default_base_url(),
            "https://api.z.ai/api/coding/paas/v4"
        );
        assert_eq!(
            PlatformId::Zai.default_base_url(),
            "https://api.z.ai/api/paas/v4"
        );
        assert_eq!(
            PlatformId::ZaiCodingCn.default_base_url(),
            "https://open.bigmodel.cn/api/coding/paas/v4"
        );
        let zai_coding: Vec<_> = platform_builtin_models()
            .iter()
            .filter(|m| m.legacy_platform() == Some(PlatformId::ZaiCoding))
            .map(|m| m.model.as_str())
            .collect();
        assert!(zai_coding.contains(&"glm-5.2"));
        assert!(zai_coding.contains(&"glm-5.1"));
        assert_eq!(zai_coding.len(), 6);
    }

    /// Mirror of the live `api.kimi.com/coding/v1/models` K3 entry
    /// (fetched 2026-07): `supports_thinking_type: "only"` plus a
    /// `think_efforts` block with low/high/max and a max default.
    #[test]
    fn wire_model_parses_live_k3_think_efforts() {
        let json = serde_json::json!({
            "id": "k3",
            "created": 1_761_264_000,
            "object": "model",
            "display_name": "K3",
            "type": "model",
            "context_length": 1_048_576,
            "supports_reasoning": true,
            "supports_image_in": true,
            "supports_video_in": true,
            "supports_thinking_type": "only",
            "think_efforts": {
                "support": true,
                "valid_efforts": ["low", "high", "max"],
                "default_effort": "max"
            }
        });
        let wire: WireModel = serde_json::from_value(json).expect("k3 wire parses");
        assert_eq!(wire.id, "k3");
        assert_eq!(wire.context_length, 1_048_576);
        assert_eq!(wire.display_name.as_deref(), Some("K3"));
        assert_eq!(wire.supports_thinking_type.as_deref(), Some("only"));
        let think = wire.think_efforts.as_ref().expect("think_efforts present");
        assert!(think.support);
        assert_eq!(think.valid_efforts, ["low", "high", "max"]);
        assert_eq!(think.default_effort.as_deref(), Some("max"));
        let caps = wire.capabilities();
        assert!(caps.contains(&ModelCapability::Thinking));
        assert!(caps.contains(&ModelCapability::AlwaysThinking));
        assert!(caps.contains(&ModelCapability::ImageIn));
        assert!(caps.contains(&ModelCapability::VideoIn));
    }

    #[test]
    fn filter_allowed_keeps_open_platform_kimi_family() {
        let models = vec![
            WireModel {
                id: "kimi-k3".into(),
                context_length: 1_048_576,
                max_output_tokens: None,
                supports_reasoning: true,
                supports_image_in: true,
                supports_video_in: true,
                display_name: Some("Kimi K3".into()),
                supports_thinking_type: None,
                think_efforts: None,
            },
            WireModel {
                id: "moonshot-v1-8k".into(),
                context_length: 8_192,
                max_output_tokens: None,
                supports_reasoning: false,
                supports_image_in: false,
                supports_video_in: false,
                display_name: None,
                supports_thinking_type: None,
                think_efforts: None,
            },
            WireModel {
                id: "kimi-k2-turbo-preview".into(),
                context_length: 262_144,
                max_output_tokens: None,
                supports_reasoning: true,
                supports_image_in: true,
                supports_video_in: true,
                display_name: None,
                supports_thinking_type: None,
                think_efforts: None,
            },
        ];
        let kept = filter_allowed_models(PlatformId::MoonshotCn, models);
        let ids: Vec<_> = kept.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["kimi-k3", "kimi-k2-turbo-preview"]);
    }

    #[test]
    fn subscription_filter_is_noop() {
        let models = vec![WireModel {
            id: "k3".into(),
            context_length: 1_048_576,
            max_output_tokens: None,
            supports_reasoning: true,
            supports_image_in: true,
            supports_video_in: true,
            display_name: Some("K3".into()),
            supports_thinking_type: Some("only".into()),
            think_efforts: None,
        }];
        let kept = filter_allowed_models(PlatformId::KimiCode, models);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "k3");
    }

    #[test]
    fn models_list_url_appends_models() {
        let url = PlatformId::KimiCode.models_list_url();
        assert!(url.ends_with("/models"), "{url}");
        assert!(url.contains("kimi.com"), "{url}");
    }

    #[test]
    fn minimax_catalog_row_carries_base_url_override() {
        let models = platform_builtin_models();
        let m = models
            .iter()
            .find(|m| m.catalog_key() == "minimax/MiniMax-M2.7")
            .expect("minimax/MiniMax-M2.7 in catalog");
        assert_eq!(
            m.base_url_override.as_deref(),
            Some("https://api.minimax.io/anthropic"),
            "MiniMax Messages backend must keep the catalog base_url_override"
        );
        assert_eq!(m.api_backend, PlatformApiBackend::Messages);
        // Grok joins `{base}/messages`; SDK-style `/anthropic` roots need `/v1`.
        assert_eq!(m.resolved_base_url(), "https://api.minimax.io/anthropic/v1");
        let cn = models
            .iter()
            .find(|m| m.catalog_key() == "minimax-cn/MiniMax-M2.7")
            .expect("minimax-cn/MiniMax-M2.7 in catalog");
        assert_eq!(
            cn.base_url_override.as_deref(),
            Some("https://api.minimaxi.com/anthropic")
        );
        assert_eq!(
            cn.resolved_base_url(),
            "https://api.minimaxi.com/anthropic/v1"
        );
    }

    #[test]
    fn fireworks_messages_rows_resolve_to_v1_base() {
        let models = platform_builtin_models();
        let m = models
            .iter()
            .find(|m| {
                m.legacy_platform() == Some(PlatformId::Fireworks)
                    && m.api_backend == PlatformApiBackend::Messages
            })
            .expect("at least one Fireworks Messages catalog row");
        assert_eq!(
            m.base_url_override.as_deref(),
            Some("https://api.fireworks.ai/inference")
        );
        assert_eq!(
            m.resolved_base_url(),
            "https://api.fireworks.ai/inference/v1"
        );
    }

    #[test]
    fn normalize_messages_sdk_base_url_covers_sdk_and_versioned_forms() {
        assert_eq!(
            normalize_messages_sdk_base_url("https://api.minimax.io/anthropic"),
            "https://api.minimax.io/anthropic/v1"
        );
        assert_eq!(
            normalize_messages_sdk_base_url("https://api.fireworks.ai/inference/v1"),
            "https://api.fireworks.ai/inference/v1"
        );
        assert_eq!(
            normalize_messages_sdk_base_url("https://api.fireworks.ai/inference/v1/messages"),
            "https://api.fireworks.ai/inference/v1"
        );
    }

    #[test]
    fn offline_catalog_includes_official_open_platform_lineup() {
        let keys: std::collections::HashSet<_> = platform_builtin_models()
            .iter()
            .map(|m| m.catalog_key())
            .collect();
        for id in [
            "moonshot-cn/kimi-k3",
            "moonshot-cn/kimi-k2.7-code",
            "moonshot-cn/kimi-k2.7-code-highspeed",
            "moonshot-cn/kimi-k2.6",
            "moonshot-cn/kimi-k2.5",
            "moonshot-ai/kimi-k3",
            "moonshot-ai/kimi-k2.7-code",
            "moonshot-ai/kimi-k2.7-code-highspeed",
            "moonshot-ai/kimi-k2.6",
            "moonshot-ai/kimi-k2.5",
            "kimi-code/k3",
            "kimi-code/k2p7",
            "kimi-code/kimi-for-coding-highspeed",
            "kimi-code/kimi-for-coding",
            "openai/gpt-4.1",
            "openai/gpt-5",
            "anthropic/claude-sonnet-4-5",
            "anthropic/claude-opus-4-5",
            "anthropic/claude-opus-4-8",
            "openrouter/openai/gpt-4o",
            "deepseek/deepseek-v4-flash",
            "groq/llama-3.3-70b-versatile",
            "ollama/gpt-oss:120b",
            "ollama/kimi-k2.7-code",
            "ollama/deepseek-v4-pro",
            "ollama/deepseek-v4-flash",
            "ollama/deepseek-v4-flash:0731",
            "fireworks/accounts/fireworks/models/deepseek-v4-flash",
            "fireworks/accounts/fireworks/models/deepseek-v4-flash-0731",
        ] {
            assert!(keys.contains(id), "missing offline fallback {id}");
        }
        for key in [
            "ollama/deepseek-v4-flash",
            "ollama/deepseek-v4-flash:0731",
            "ollama/deepseek-v4-pro",
            "fireworks/accounts/fireworks/models/deepseek-v4-flash-0731",
        ] {
            let m = platform_builtin_models()
                .iter()
                .find(|m| m.catalog_key() == key)
                .unwrap_or_else(|| panic!("missing {key}"));
            assert_eq!(
                m.context_window, 1_000_000,
                "{key}: DeepSeek V4 is 1M context"
            );
            assert_eq!(
                m.max_completion_tokens,
                Some(384_000),
                "{key}: DeepSeek V4 max output is 384K"
            );
        }
        let fw_flash_0731 = platform_builtin_models()
            .iter()
            .find(|m| {
                m.catalog_key() == "fireworks/accounts/fireworks/models/deepseek-v4-flash-0731"
            })
            .expect("fireworks deepseek-v4-flash-0731");
        assert_eq!(
            fw_flash_0731.api_backend,
            PlatformApiBackend::Messages,
            "Fireworks DeepSeek V4 Flash uses Anthropic Messages like the Pi flash row"
        );
        assert!(
            platform_builtin_models().len() >= 100,
            "expected full Pi-derived catalog, got {}",
            platform_builtin_models().len()
        );
        let anth = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "anthropic/claude-sonnet-4-5")
            .expect("claude-sonnet-4-5");
        assert_eq!(anth.api_backend, PlatformApiBackend::Messages);
        let oai = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "openai/gpt-5")
            .expect("gpt-5");
        assert_eq!(oai.api_backend, PlatformApiBackend::Responses);
        for key in [
            "kimi-code/k3",
            "kimi-code/k2p7",
            "kimi-code/kimi-for-coding-highspeed",
        ] {
            let m = platform_builtin_models()
                .iter()
                .find(|m| m.catalog_key() == key)
                .unwrap_or_else(|| panic!("missing {key}"));
            assert_eq!(
                m.api_backend,
                PlatformApiBackend::Messages,
                "{key}: official Pi kimi-coding uses anthropic-messages"
            );
            assert!(!m.supported_in_api, "{key} starts hidden until OAuth");
            assert!(
                m.supports_reasoning_effort,
                "{key} supports adaptive effort"
            );
        }
    }

    #[test]
    fn request_profiles_cover_official_ids() {
        assert_eq!(
            kimi_request_profile("kimi-k3"),
            Some(KimiRequestProfile::K3)
        );
        assert_eq!(kimi_request_profile("k3"), Some(KimiRequestProfile::K3));
        assert_eq!(
            kimi_request_profile("kimi-code/k3"),
            Some(KimiRequestProfile::K3)
        );
        assert_eq!(
            kimi_request_profile("k2p7"),
            Some(KimiRequestProfile::K27Code)
        );
        assert_eq!(
            kimi_request_profile("kimi-code/k2p7"),
            Some(KimiRequestProfile::K27Code)
        );
        assert_eq!(
            kimi_request_profile("kimi-for-coding-highspeed"),
            Some(KimiRequestProfile::K27Code)
        );
        assert_eq!(
            kimi_request_profile("kimi-k2.7-code"),
            Some(KimiRequestProfile::K27Code)
        );
        assert_eq!(
            kimi_request_profile("kimi-k2.7-code-highspeed"),
            Some(KimiRequestProfile::K27Code)
        );
        assert_eq!(
            kimi_request_profile("moonshot-cn/kimi-k2.6"),
            Some(KimiRequestProfile::K26)
        );
        assert_eq!(
            kimi_request_profile("kimi-k2.5"),
            Some(KimiRequestProfile::K25)
        );
        assert_eq!(
            kimi_request_profile("kimi-for-coding"),
            Some(KimiRequestProfile::LegacyCoding)
        );
        assert!(kimi_sampling_is_fixed(KimiRequestProfile::K27Code));
        assert!(!kimi_sampling_is_fixed(KimiRequestProfile::K3));
        assert!(kimi_force_adaptive_thinking(KimiRequestProfile::K3));
        assert!(kimi_force_adaptive_thinking(KimiRequestProfile::K27Code));
        assert!(kimi_allow_empty_thinking_signature(KimiRequestProfile::K3));
        assert!(!kimi_allow_empty_thinking_signature(
            KimiRequestProfile::K27Code
        ));
    }

    #[test]
    fn nvidia_fallback_request_compat_disables_openai_only_fields() {
        let compat = fallback_request_compat(
            PlatformId::Nvidia,
            PlatformApiBackend::ChatCompletions,
            "nvidia/nemotron-3-super-120b-a12b",
        );
        let RequestCompat::ChatCompletions(chat) = compat else {
            panic!("NVIDIA uses chat completions");
        };
        assert!(!chat.supports_prompt_cache_key);
        assert!(!chat.supports_store);
        assert!(!chat.supports_developer_role);
        assert!(!chat.supports_strict_mode);
        assert!(!chat.supports_long_cache_retention);
        assert_eq!(chat.max_tokens_field, MaxTokensField::MaxTokens);
        assert!(!chat.agent_ready);
        assert!(!chat.supports_message_model_id);
        assert!(chat.max_parallel_tool_calls.is_none());
    }

    #[test]
    fn nvidia_fallback_marks_llama_70b_single_tool_call() {
        let compat = fallback_request_compat(
            PlatformId::Nvidia,
            PlatformApiBackend::ChatCompletions,
            "meta/llama-3.1-70b-instruct",
        );
        let RequestCompat::ChatCompletions(chat) = compat else {
            panic!("expected chat completions");
        };
        assert_eq!(chat.max_parallel_tool_calls, Some(1));
        assert!(!chat.supports_prompt_cache_key);
        assert!(!chat.agent_ready);
        assert!(!chat.supports_message_model_id);
    }

    #[test]
    fn nvidia_catalog_rows_get_platform_compat_overrides() {
        let nano = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/nvidia/nvidia-nemotron-nano-9b-v2")
            .expect("nvidia nano 9b");
        let RequestCompat::ChatCompletions(chat) = &nano.request_compat else {
            panic!("nano 9b is chat completions");
        };
        assert!(!chat.supports_prompt_cache_key);
        assert!(!chat.supports_store);
        assert!(!chat.supports_developer_role);
        assert!(!chat.supports_strict_mode);
        assert_eq!(chat.max_tokens_field, MaxTokensField::MaxTokens);
        assert!(!chat.agent_ready);
        assert!(!chat.supports_message_model_id);
        // Catalog max must not exceed NVIDIA max_model_len (128000).
        if let Some(max_tok) = nano.max_completion_tokens {
            assert!(
                max_tok <= 128_000,
                "nano 9b max_completion_tokens={max_tok} exceeds 128000"
            );
        }

        let llama = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/meta/llama-3.1-70b-instruct")
            .expect("nvidia llama 3.1 70b");
        let RequestCompat::ChatCompletions(llama_chat) = &llama.request_compat else {
            panic!("llama is chat completions");
        };
        assert_eq!(llama_chat.max_parallel_tool_calls, Some(1));
    }

    #[test]
    fn nvidia_lightning_is_in_builtin_catalog() {
        let keys: std::collections::HashSet<_> = platform_builtin_models()
            .iter()
            .map(|m| m.catalog_key())
            .collect();
        assert!(
            keys.contains("nvidia/nvidia/nemotron-3.5-lightning-30b-a3b"),
            "catalog convention slug missing: {keys:?}"
        );
        assert!(
            keys.contains("nvidia/nemotron-3.5-lightning-30b-a3b"),
            "short NVIDIA Integrate slug missing"
        );
        let lightning = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/nvidia/nemotron-3.5-lightning-30b-a3b")
            .expect("lightning");
        let RequestCompat::ChatCompletions(chat) = &lightning.request_compat else {
            panic!("lightning is chat completions");
        };
        assert!(!chat.supports_prompt_cache_key);
        assert_eq!(chat.max_tokens_field, MaxTokensField::MaxTokens);
        assert_eq!(lightning.context_window, 1_000_000);
        assert!(
            chat.agent_ready,
            "Lightning is an agentic NIM and must be spawnable for write work"
        );
    }

    #[test]
    fn nvidia_integrate_muse_poolside_mistral_nemotron_are_in_builtin_catalog() {
        let keys: std::collections::HashSet<_> = platform_builtin_models()
            .iter()
            .map(|m| m.catalog_key())
            .collect();
        for key in [
            "nvidia/meta/muse-glimmer-30b",
            "nvidia/muse-glimmer-30b",
            "nvidia/poolside/laguna-xs-2.1",
            "nvidia/laguna-xs-2.1",
            "nvidia/mistralai/mistral-nemotron",
            "nvidia/mistral-nemotron",
        ] {
            assert!(keys.contains(key), "missing NVIDIA Integrate slug {key}");
        }

        let glimmer = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/meta/muse-glimmer-30b")
            .expect("muse glimmer");
        let RequestCompat::ChatCompletions(chat) = &glimmer.request_compat else {
            panic!("muse glimmer is chat completions");
        };
        assert!(!chat.supports_prompt_cache_key);
        assert_eq!(chat.max_tokens_field, MaxTokensField::MaxTokens);
        assert_eq!(glimmer.context_window, 131_072);
        assert_eq!(glimmer.max_completion_tokens, Some(32_768));
        assert!(chat.agent_ready, "Muse Glimmer is an agentic NIM");

        let laguna = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/poolside/laguna-xs-2.1")
            .expect("laguna xs");
        assert_eq!(laguna.context_window, 262_144);
        assert_eq!(laguna.max_completion_tokens, Some(32_768));

        let laguna_s = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "poolside/laguna-s-2.1")
            .expect("laguna s");
        assert_eq!(laguna_s.max_completion_tokens, Some(32_768));

        let nemotron = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/mistralai/mistral-nemotron")
            .expect("mistral-nemotron");
        assert_eq!(nemotron.context_window, 128_000);
        assert_eq!(nemotron.max_completion_tokens, Some(8_192));

        for canonical in [
            "nvidia/meta/muse-glimmer-30b",
            "nvidia/poolside/laguna-xs-2.1",
            "nvidia/mistralai/mistral-nemotron",
            "nvidia/moonshotai/kimi-k3",
            "nvidia/deepseek-ai/deepseek-v4-pro-0813",
            "nvidia/deepseek-ai/deepseek-v4-flash-0731",
        ] {
            assert!(
                platform_builtin_models()
                    .iter()
                    .find(|model| model.catalog_key() == canonical)
                    .is_some_and(|model| model.picker_visible),
                "canonical Nvidia row should remain picker-visible: {canonical}"
            );
        }
        for alias in [
            "nvidia/muse-glimmer-30b",
            "nvidia/laguna-xs-2.1",
            "nvidia/mistral-nemotron",
            "nvidia/nemotron-3.5-lightning-30b-a3b",
            "nvidia/kimi-k3",
            "nvidia/deepseek-v4-pro-0813",
            "nvidia/deepseek-v4-flash-0731",
        ] {
            assert!(
                platform_builtin_models()
                    .iter()
                    .find(|model| model.catalog_key() == alias)
                    .is_some_and(|model| !model.picker_visible),
                "Nvidia compatibility alias should be hidden: {alias}"
            );
        }
    }

    #[test]
    fn poolside_hosted_catalog_is_picker_visible_with_qualified_wire_ids() {
        assert_eq!(PlatformId::parse("poolside"), Some(PlatformId::Poolside));
        assert_eq!(PlatformId::Poolside.display_name(), "Poolside");
        assert_eq!(
            PlatformId::Poolside.base_url(),
            "https://inference.poolside.ai/v1"
        );
        assert_eq!(
            PlatformId::Poolside.api_key_env_names(),
            &["GROK_POOLSIDE_API_KEY", "POOLSIDE_API_KEY"]
        );
        assert!(!PlatformId::Poolside.live_models_list_enabled());
        let spec = provider_spec("poolside").expect("poolside registry row");
        assert_eq!(spec.display_name, "Poolside");
        assert_eq!(spec.default_base_url, "https://inference.poolside.ai/v1");
        assert!(spec.accepts_api_key());

        for (key, wire, ctx) in [
            ("poolside/laguna-s-2.1", "poolside/laguna-s-2.1", CTX_1M),
            ("poolside/laguna-xs-2.1", "poolside/laguna-xs-2.1", CTX_256K),
        ] {
            let model = platform_builtin_models()
                .iter()
                .find(|m| m.catalog_key() == key)
                .unwrap_or_else(|| panic!("missing {key}"));
            assert!(model.picker_visible, "{key} should be picker-visible");
            assert!(model.catalog_available);
            assert!(!model.eol);
            assert_eq!(model.context_window, ctx);
            assert_eq!(model.resolved_runtime().wire_model_id, wire);
            match &model.request_compat {
                RequestCompat::ChatCompletions(c) => {
                    assert!(c.agent_ready);
                    assert!(c.requires_reasoning_content_on_assistant_messages);
                    assert_eq!(c.thinking_format, ThinkingFormat::QwenChatTemplate);
                    assert_eq!(c.max_tokens_field, MaxTokensField::MaxTokens);
                    assert!(!c.supports_prompt_cache_key);
                    assert!(!c.supports_message_model_id);
                }
                other => panic!("{key} expected chat completions compat, got {other:?}"),
            }
        }
    }

    #[test]
    fn poolside_laguna_m1_is_catalog_eol() {
        let model = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "poolside/laguna-m.1")
            .expect("poolside laguna-m.1 row");
        assert!(model.eol);
        assert!(!model.picker_visible);
        assert!(!model.catalog_available);
        assert!(catalog_key_is_eol("poolside/laguna-m.1"));
        // OpenRouter clone shares the fate so neither route 404s at the provider.
        assert!(is_poolside_laguna_m1_eol_slug(
            "openrouter/poolside/laguna-m.1"
        ));
        assert!(!is_poolside_laguna_m1_eol_slug("poolside/laguna-s-2.1"));
    }

    #[test]
    fn nvidia_aliases_use_canonical_wire_model_ids() {
        for (alias, canonical) in [
            ("nvidia/muse-glimmer-30b", "meta/muse-glimmer-30b"),
            ("nvidia/laguna-xs-2.1", "poolside/laguna-xs-2.1"),
            ("nvidia/mistral-nemotron", "mistralai/mistral-nemotron"),
            ("nvidia/kimi-k3", "moonshotai/kimi-k3"),
            (
                "nvidia/deepseek-v4-pro-0813",
                "deepseek-ai/deepseek-v4-pro-0813",
            ),
            (
                "nvidia/deepseek-v4-flash-0731",
                "deepseek-ai/deepseek-v4-flash-0731",
            ),
        ] {
            let model = platform_builtin_models()
                .into_iter()
                .find(|model| model.catalog_key() == alias)
                .expect("NVIDIA alias");
            assert_eq!(model.resolved_runtime().wire_model_id, canonical);
        }
    }

    #[test]
    fn nvidia_glm_52_is_catalog_eol() {
        let glm = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/z-ai/glm-5.2")
            .expect("nvidia glm-5.2 snapshot row must remain");
        assert!(glm.eol, "historical snapshot stays, marked EOL");
        assert!(!glm.catalog_available);
        assert!(!glm.picker_visible);
        assert!(catalog_key_is_eol("nvidia/z-ai/glm-5.2"));
        assert!(is_nvidia_glm_52_eol_slug("nvidia/z-ai/glm-5.2"));
        assert!(is_nvidia_glm_52_eol_slug("nvidia/glm-5.2"));
        assert!(
            !is_nvidia_glm_52_eol_slug("openrouter/z-ai/glm-5.2"),
            "OpenRouter GLM-5.2 is not the NVIDIA 410 row"
        );
    }

    #[test]
    fn openrouter_minimax_m3_free_and_ultra_are_cataloged() {
        let keys: std::collections::HashSet<_> = platform_builtin_models()
            .iter()
            .map(|m| m.catalog_key())
            .collect();
        assert!(
            keys.contains("openrouter/minimax/minimax-m3:free"),
            "MiniMax M3 free slug must be spawnable"
        );
        assert!(
            keys.contains("openrouter/thinkingmachines/inkling:free"),
            "Inkling free slug must be spawnable"
        );
        assert!(
            keys.contains("openrouter/nvidia/nemotron-3-ultra-550b-a55b"),
            "OpenRouter Nemotron Ultra paid slug"
        );
        assert!(
            keys.contains("openrouter/nvidia/nemotron-3-ultra-550b-a55b:free"),
            "OpenRouter Nemotron Ultra free slug"
        );
        let free = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "openrouter/minimax/minimax-m3:free")
            .expect("m3 free");
        assert!(free.catalog_available);
        assert!(free.picker_visible);
        assert!(!free.eol);
    }

    #[test]
    fn nvidia_kimi_k3_and_deepseek_v4_are_in_builtin_catalog() {
        let keys: std::collections::HashSet<_> = platform_builtin_models()
            .iter()
            .map(|m| m.catalog_key())
            .collect();
        for key in [
            "nvidia/moonshotai/kimi-k3",
            "nvidia/deepseek-ai/deepseek-v4-pro-0813",
            "nvidia/deepseek-ai/deepseek-v4-flash-0731",
            "nvidia/deepseek-ai/deepseek-v4-pro",
            "nvidia/deepseek-ai/deepseek-v4-flash",
        ] {
            assert!(keys.contains(key), "missing NVIDIA Integrate slug {key}");
            let row = platform_builtin_models()
                .iter()
                .find(|m| m.catalog_key() == key)
                .unwrap();
            let RequestCompat::ChatCompletions(chat) = &row.request_compat else {
                panic!("{key} is chat completions");
            };
            assert!(chat.agent_ready, "{key} must be agent-ready for write work");
            assert_eq!(chat.max_tokens_field, MaxTokensField::MaxTokens);
        }
        let pro = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/deepseek-ai/deepseek-v4-pro-0813")
            .unwrap();
        assert_eq!(pro.context_window, 1_000_000);
        assert_eq!(
            pro.resolved_runtime().wire_model_id,
            "deepseek-ai/deepseek-v4-pro-0813"
        );
        let kimi = platform_builtin_models()
            .iter()
            .find(|m| m.catalog_key() == "nvidia/moonshotai/kimi-k3")
            .unwrap();
        assert_eq!(kimi.resolved_runtime().wire_model_id, "moonshotai/kimi-k3");
    }

    #[test]
    fn nvidia_hang_and_ultra_models_stay_chat_only() {
        for key in [
            "nvidia/meta/llama-3.3-70b-instruct",
            "nvidia/openai/gpt-oss-120b",
            "nvidia/nvidia/nemotron-3-ultra-550b-a55b",
            "nvidia/meta/llama-3.1-8b-instruct",
        ] {
            let row = platform_builtin_models()
                .iter()
                .find(|m| m.catalog_key() == key)
                .unwrap_or_else(|| panic!("missing {key}"));
            assert!(!row.eol, "{key} is chat-only, not deleted");
            let RequestCompat::ChatCompletions(chat) = &row.request_compat else {
                panic!("{key} is chat completions");
            };
            assert!(
                !chat.agent_ready,
                "{key} must stay agent_ready=false (chat-only)"
            );
        }
    }

    #[test]
    fn clamp_max_completion_tokens_takes_min_of_requested_catalog_context() {
        assert_eq!(
            clamp_max_completion_tokens(Some(200_000), Some(131_072), 128_000),
            Some(128_000)
        );
        assert_eq!(
            clamp_max_completion_tokens(Some(4_096), Some(131_072), 128_000),
            Some(4_096)
        );
        assert_eq!(
            clamp_max_completion_tokens(None, Some(8_192), 128_000),
            Some(8_192)
        );
        assert_eq!(clamp_max_completion_tokens(Some(100), None, 0), Some(100));
        assert_eq!(clamp_max_completion_tokens(None, None, 128_000), None);
    }
}
