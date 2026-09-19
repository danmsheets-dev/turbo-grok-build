//! ACP `authenticate` handlers for third-party subscription OAuth.
//!
//! These methods persist under their own `auth.json` scopes and must not
//! replace the xAI `AuthManager` session. The pager already sends
//! `openai-codex` / `kimi-code` / … as `AuthenticateRequest.method_id`;
//! without this path the agent returned `invalid_params`.

use agent_client_protocol as acp;
use agent_client_protocol::AuthenticateResponse;
use xai_grok_telemetry::session_ctx::log_event;

use super::{AuthRequestMeta, MvpAgent, emit_login_span};
use crate::agent::auth_method;
use crate::auth::{AuthChannels, openai_codex::CodexLoginMethod, radius::RadiusLoginMethod};

impl MvpAgent {
    pub(super) async fn authenticate_third_party(
        &self,
        arguments: acp::AuthenticateRequest,
    ) -> Result<AuthenticateResponse, acp::Error> {
        let method_id = arguments.method_id.0.to_string();
        let auth_meta = AuthRequestMeta::from_json(arguments.meta.as_ref());
        tracing::info!(
            method = %method_id,
            headless = auth_meta.headless,
            "auth: third-party subscription login"
        );
        xai_grok_telemetry::unified_log::info(
            "auth: third-party subscription login",
            None,
            Some(serde_json::json!({
                "method": method_id,
                "headless": auth_meta.headless,
            })),
        );

        let mut cancelled = false;
        let client_seq = auth_meta.request_seq;
        let use_oauth = auth_meta.use_oauth;
        let auth_result = if !auth_meta.headless {
            let (url_tx, url_rx) = tokio::sync::oneshot::channel();
            let (code_tx, code_rx) = tokio::sync::mpsc::channel(1);
            let (cancel, _guard) = self.interactive_auth.begin(
                Some(crate::auth::single_flight::AttemptChannels::new(
                    code_tx, url_rx,
                )),
                client_seq,
            );
            let channels = AuthChannels {
                url_tx: Some(url_tx),
                code_rx,
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    cancelled = true;
                    Err(anyhow::anyhow!("Authentication cancelled"))
                }
                r = run_third_party_login(&method_id, Some(channels), use_oauth) => r,
            }
        } else {
            let (cancel, _guard) = self.interactive_auth.begin(None, client_seq);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    cancelled = true;
                    Err(anyhow::anyhow!("Authentication cancelled"))
                }
                r = run_third_party_login(&method_id, None, use_oauth) => r,
            }
        };

        let auth = auth_result.map_err(|e| {
            emit_login_span(
                false,
                &method_id,
                None,
                Some(if cancelled {
                    "login_cancelled"
                } else {
                    "login_flow_failed"
                }),
            );
            let mut err = acp::Error::auth_required();
            err.message = e.to_string();
            err
        })?;

        // Keep an existing xAI session method. Third-party credentials live in
        // their own auth.json scope; overwriting grok.com/cached_token would
        // stop xAI refresh and hide session-gated catalog rows.
        if !self.is_session_based_auth() {
            self.set_auth_method(arguments.method_id.clone());
        }
        self.models_manager.restamp_platform_credentials();
        emit_login_span(true, &method_id, Some(auth.user_id.as_str()), None);
        log_event(xai_grok_telemetry::events::Login {
            auth_method: method_id,
            user_id: Some(auth.user_id.clone()),
        });
        Ok(self.auth_response_with_meta())
    }
}

async fn run_third_party_login(
    method_id: &str,
    channels: Option<AuthChannels>,
    _use_oauth: bool,
) -> anyhow::Result<crate::auth::GrokAuth> {
    match method_id {
        auth_method::KIMI_CODE_METHOD_ID => {
            crate::auth::kimi::run_kimi_code_login_with_channels(channels).await
        }
        auth_method::OPENAI_CODEX_METHOD_ID => {
            crate::auth::openai_codex::run_openai_codex_login(channels, CodexLoginMethod::Browser)
                .await
        }
        auth_method::ANTHROPIC_CLAUDE_METHOD_ID => {
            crate::auth::anthropic_claude::run_anthropic_claude_login(channels).await
        }
        auth_method::GITHUB_COPILOT_METHOD_ID => {
            crate::auth::github_copilot::run_github_copilot_login_with_channels(None, channels)
                .await
        }
        auth_method::RADIUS_METHOD_ID => {
            crate::auth::radius::run_radius_login_with_channels(
                None,
                RadiusLoginMethod::Browser,
                channels,
            )
            .await
        }
        other => anyhow::bail!("unsupported auth method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_login_ids_are_the_advertised_subscription_methods() {
        for id in [
            auth_method::KIMI_CODE_METHOD_ID,
            auth_method::OPENAI_CODEX_METHOD_ID,
            auth_method::ANTHROPIC_CLAUDE_METHOD_ID,
            auth_method::GITHUB_COPILOT_METHOD_ID,
            auth_method::RADIUS_METHOD_ID,
        ] {
            assert!(
                auth_method::AuthMethodKind::from_id(&acp::AuthMethodId::new(id))
                    .is_third_party_subscription(),
                "{id} must dispatch as third-party subscription login"
            );
        }
        assert!(
            !auth_method::AuthMethodKind::from_id(&acp::AuthMethodId::new(
                auth_method::GROK_COM_METHOD_ID
            ))
            .is_third_party_subscription()
        );
    }
}
