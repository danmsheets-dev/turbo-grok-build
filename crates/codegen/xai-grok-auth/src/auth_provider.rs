//! Credential dependency-inversion seam for outbound HTTP made by the
//! data-collector. Shell installs `ShellAuthCredentialProvider` wrapping
//! `AuthManager` + `TokenRefresher`; data-collector code holds an
//! `Arc<dyn AuthCredentialProvider>`.

use reqwest::RequestBuilder;

use crate::visibility::HttpAuth;

/// Optional list of capability scopes that gate which agent tools a stored
/// credential may be used for (e.g. `["mcp:sentry", "mcp:resend"]`).
///
/// `None` = legacy/unscoped credential (fully backward compatible — current
/// behavior, usable everywhere). `Some(Vec::new())` = explicitly unusable by
/// agents. `Some([...])` = restricted to the listed scopes.
pub type CredentialScopes = Option<Vec<String>>;

/// A single credential entry as it round-trips through `auth.json`-style stores.
///
/// `scopes` is `None` for legacy files (fully backward compatible) and
/// `Some([])` for an explicitly locked-down credential. The field is omitted
/// from serialization when `None` so old files round-trip unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CredentialEntry {
    /// Bearer token carried by this credential entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Optional scope allow-list (see [`CredentialScopes`] for semantics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: CredentialScopes,
}

impl CredentialEntry {
    /// Create a legacy/unscoped credential entry (backward compatible).
    pub fn unscoped(token: impl Into<String>) -> Self {
        Self {
            token: Some(token.into()),
            scopes: None,
        }
    }

    /// Create a scoped credential entry. Empty list = unusable by agents.
    pub fn scoped(token: impl Into<String>, scopes: Vec<String>) -> Self {
        Self {
            token: Some(token.into()),
            scopes: Some(scopes),
        }
    }

    /// `true` when this credential is permitted to satisfy `requested_scope`.
    /// - `None` scopes → legacy/unscoped → allowed for any scope.
    /// - `Some([])` → explicitly locked-down → never allowed.
    /// - `Some(list)` → allowed only when `requested_scope` is in `list`.
    pub fn allowed_for(&self, requested_scope: &str) -> bool {
        match &self.scopes {
            None => true,
            Some(scopes) => scopes.iter().any(|s| s == requested_scope),
        }
    }
}

/// Pure scope-enforcement check for a credential entry.
///
/// `None` scopes (legacy) always pass; `Some([])` always fails; `Some(list)`
/// passes only when `requested_scope` appears in `list`.
///
/// This is the V1 helper for scope gating. Call sites that inject per-server
/// env from stored credentials should guard with this (see the MCP env-injection
/// TODO in the shell extension — see report for status).
pub fn credential_allowed_for(entry: &CredentialEntry, requested_scope: &str) -> bool {
    entry.allowed_for(requested_scope)
}

/// Snapshot of the currently effective credentials. Used by callers
/// that build their own header maps (the OTel OTLP exporter) or that
/// need the bearer prefix for 401-attribution telemetry.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CredentialSnapshot {
    /// Bearer token. `None` when no auth is configured (CI / `--api-key` headless).
    pub token: Option<String>,
    /// User identifier matching the bearer token's owner. `None` when no auth
    /// is configured or when the underlying provider has no concept of user
    /// identity (`StaticAuthCredentialProvider`). Read by the OTel layer to
    /// populate the `user.id` resource attribute.
    pub user_id: Option<String>,
    /// Team identifier from OAuth. `None` for personal accounts or when
    /// no auth is configured.
    pub team_id: Option<String>,
    /// `uuidv5(NAMESPACE_OID, deployment_key)`, set only for deployment-key auth.
    pub deployment_id: Option<String>,
    /// `uuidv5(NAMESPACE_OID, api_key)`, set only for `AuthMode::ApiKey`.
    pub api_key_id: Option<String>,
    /// Org id from the OIDC `organizationId` claim; `None` for personal / deployment-key auth.
    pub organization_id: Option<String>,
    /// Optional scope allow-list for this credential (secrets vault v1).
    /// `None` = legacy/unscoped (backward compatible); `Some([])` = unusable
    /// by scoped agents; `Some(list)` = restricted to listed scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: CredentialScopes,
}

/// Source of truth for outbound auth on data-collector requests.
///
/// Supertrait of `HttpAuth` so a single impl satisfies both this trait
/// (refresh-aware snapshot + 401 recovery) and the visibility seam
/// (header construction). Callers add headers via `HttpAuth::apply`.
#[async_trait::async_trait]
pub trait AuthCredentialProvider: HttpAuth + Send + Sync + 'static {
    /// Return the current credential snapshot. Implementations should
    /// issue a cheap disk re-read (`AuthManager::refresh`) before
    /// snapshotting so callers see updates from sibling processes
    /// (`grok-desktop`, `grok login`). The `token` field MUST mirror
    /// the bearer that `HttpAuth::apply` would send on the wire so
    /// 401-attribution prefixes match the actual request.
    fn snapshot(&self) -> CredentialSnapshot;

    /// Attempt to obtain a fresh token. Returns `true` if a different
    /// token was obtained -- caller should retry the failed request once.
    /// Returns `false` if no refresher is configured or refresh failed.
    async fn refresh_after_unauthorized(&self) -> bool;

    /// Whether `X-XAI-Token-Auth` should be sent with the bearer token.
    /// `false` for deployment keys (bare Bearer), `true` for user/OAuth tokens.
    /// See `GrokAuthCredentials::apply()` for the wire format contract.
    fn needs_token_auth_header(&self) -> bool {
        true
    }

    /// Whether the provider holds a credential worth a real outbound attempt —
    /// an unexpired token (in memory or on disk), or a static key. Default
    /// `true` always attempts.
    fn has_usable_credential(&self) -> bool {
        true
    }
}

/// Static credential provider. Used by tests and by callers that pass a
/// raw `&str` token with no `AuthManager` available.
///
/// `apply()` delegates to the underlying `HttpAuth::apply()`.
/// `refresh_after_unauthorized()` always returns `false`.
///
/// `bearer` is the wire bearer the inner `HttpAuth` will send in the
/// `Authorization` header. Stored alongside the inner so `snapshot().token`
/// returns the same prefix that goes out on the wire (used by
/// 401-attribution telemetry). `None` when no bearer is configured.
pub struct StaticAuthCredentialProvider {
    inner: Box<dyn HttpAuth>,
    bearer: Option<String>,
}

impl StaticAuthCredentialProvider {
    /// Wrap `inner` so callers see it as an `AuthCredentialProvider`. Pass
    /// the bearer token that `inner.apply()` will send in the `Authorization`
    /// header so `snapshot().token` reflects the wire bearer truthfully.
    pub fn new(inner: Box<dyn HttpAuth>, bearer: Option<String>) -> Self {
        Self { inner, bearer }
    }
}

impl std::fmt::Debug for StaticAuthCredentialProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticAuthCredentialProvider")
            .field("has_bearer", &self.bearer.is_some())
            .finish()
    }
}

impl HttpAuth for StaticAuthCredentialProvider {
    fn apply(&self, builder: RequestBuilder, base_url: &str) -> RequestBuilder {
        self.inner.apply(builder, base_url)
    }
}

#[async_trait::async_trait]
impl AuthCredentialProvider for StaticAuthCredentialProvider {
    fn snapshot(&self) -> CredentialSnapshot {
        CredentialSnapshot {
            token: self.bearer.clone(),
            ..Default::default()
        }
    }

    async fn refresh_after_unauthorized(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unscoped_entry_allows_any_scope() {
        let entry = CredentialEntry::unscoped("tok");
        assert!(entry.allowed_for("mcp:sentry"));
        assert!(entry.allowed_for("mcp:resend"));
        assert!(entry.allowed_for("anything"));
    }

    #[test]
    fn empty_scopes_blocks_all() {
        let entry = CredentialEntry::scoped("tok", vec![]);
        assert!(!entry.allowed_for("mcp:sentry"));
        assert!(!entry.allowed_for("anything"));
    }

    #[test]
    fn scoped_entry_allows_only_listed() {
        let entry = CredentialEntry::scoped("tok", vec!["mcp:sentry".into(), "mcp:resend".into()]);
        assert!(entry.allowed_for("mcp:sentry"));
        assert!(entry.allowed_for("mcp:resend"));
        assert!(!entry.allowed_for("mcp:other"));
    }

    #[test]
    fn credential_allowed_for_matches_entry_helper() {
        let unscoped = CredentialEntry::unscoped("tok");
        assert!(credential_allowed_for(&unscoped, "mcp:sentry"));

        let empty = CredentialEntry::scoped("tok", vec![]);
        assert!(!credential_allowed_for(&empty, "mcp:sentry"));

        let scoped = CredentialEntry::scoped("tok", vec!["mcp:sentry".into()]);
        assert!(credential_allowed_for(&scoped, "mcp:sentry"));
        assert!(!credential_allowed_for(&scoped, "mcp:resend"));
    }

    #[test]
    fn legacy_json_round_trips_without_scopes_field() {
        // A legacy credential entry (no `scopes` key) must deserialize with
        // scopes=None and re-serialize without a `scopes` key.
        let legacy = r#"{"token":"tok"}"#;
        let entry: CredentialEntry = serde_json::from_str(legacy).unwrap();
        assert_eq!(entry.scopes, None);
        let out = serde_json::to_string(&entry).unwrap();
        assert!(
            !out.contains("scopes"),
            "legacy round-trip emitted scopes key: {out}"
        );
    }

    #[test]
    fn empty_scopes_round_trips_as_empty_array() {
        let json = r#"{"token":"tok","scopes":[]}"#;
        let entry: CredentialEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.scopes, Some(vec![]));
        let out = serde_json::to_string(&entry).unwrap();
        assert!(out.contains(r#""scopes":[]"#), "scopes not preserved: {out}");
    }

    #[test]
    fn scoped_entry_round_trips() {
        let json = r#"{"token":"tok","scopes":["mcp:sentry","mcp:resend"]}"#;
        let entry: CredentialEntry = serde_json::from_str(json).unwrap();
        assert_eq!(
            entry.scopes,
            Some(vec!["mcp:sentry".to_string(), "mcp:resend".to_string()])
        );
        let out = serde_json::to_string(&entry).unwrap();
        assert!(out.contains("mcp:sentry"), "scopes not preserved: {out}");
        assert!(out.contains("mcp:resend"), "scopes not preserved: {out}");
    }

    #[test]
    fn snapshot_scopes_default_none() {
        let snap = CredentialSnapshot::default();
        assert_eq!(snap.scopes, None);
    }
}
