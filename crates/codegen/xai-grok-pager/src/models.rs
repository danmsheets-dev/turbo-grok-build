//! `hyper models` / `grok models` subcommand.
//!
//! Served from the local model + platform registry — does **not** spawn the
//! full agent shell (that path previously aborted with exit 255 during
//! teardown). Exit codes: 0 success, 1 failure.

use anyhow::Result;
use xai_grok_shell::agent::config::Config as AgentConfig;
use xai_grok_shell::cli_models::AuthStatus;

use crate::app::cli::ModelsArgs;

/// Billing classification for a model id (H8).
///
/// - `default` — native xAI routes
/// - `subscription` — Codex / ChatGPT / Claude Pro subscription-backed platforms
/// - `pay-per-token` — direct provider API platforms billed per token
/// - `provider-key` — user-supplied API key for a third-party platform
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingClass {
    Default,
    Subscription,
    PayPerToken,
    ProviderKey,
}

impl BillingClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Subscription => "subscription",
            Self::PayPerToken => "pay-per-token",
            Self::ProviderKey => "provider-key",
        }
    }
}

/// Classify a catalog model id by its platform prefix.
pub fn billing_for_model_id(model_id: &str) -> BillingClass {
    let platform = model_id.split('/').next().unwrap_or(model_id);
    match platform {
        // Native xAI / grok routes (no platform prefix, or explicit xai).
        "grok" | "xai" => BillingClass::Default,
        // Subscription-backed OAuth platforms.
        "openai-codex" | "anthropic-claude" | "kimi-code" | "opencode-go" | "github-copilot" => {
            BillingClass::Subscription
        }
        // Direct provider API-key platforms (pay-per-token).
        "openai" | "anthropic" | "google" | "gemini" | "mistral" | "deepseek" | "groq"
        | "together" | "fireworks" | "openrouter" | "amazon-bedrock" | "azure" | "azure-openai" => {
            BillingClass::PayPerToken
        }
        // Everything else with a platform prefix is user-key BYOK.
        other if model_id.contains('/') && other != model_id => BillingClass::ProviderKey,
        // Unprefixed ids are the native default catalog.
        _ => BillingClass::Default,
    }
}

/// Platform id for JSON (left of `/`, or `"xai"` for unprefixed).
fn platform_for_model_id(model_id: &str) -> &str {
    if let Some((p, _)) = model_id.split_once('/') {
        p
    } else {
        "xai"
    }
}

pub async fn list_available_models(agent_config: &AgentConfig, args: &ModelsArgs) -> Result<()> {
    // Registry-backed catalog — no agent shell spawn. Spawning the full shell
    // for a listing used to leave exit 255 from teardown/abort; the catalog
    // is static data and does not need a live session.
    let models = xai_grok_shell::agent::config::resolve_model_list(agent_config, None);
    let default_model = agent_config
        .default_model_override
        .clone()
        .or_else(|| agent_config.models.default.clone())
        .or_else(|| models.keys().next().cloned())
        .unwrap_or_else(|| "grok-4.5".to_string());

    if args.json {
        let mut entries = Vec::with_capacity(models.len());
        for id in models.keys() {
            let billing = billing_for_model_id(id);
            entries.push(serde_json::json!({
                "id": id,
                "platform": platform_for_model_id(id),
                "billing": billing.as_str(),
                "route": platform_for_model_id(id),
                "default": id == &default_model,
            }));
        }
        // Stable order for harnesses.
        entries.sort_by(|a, b| {
            a["id"]
                .as_str()
                .unwrap_or("")
                .cmp(b["id"].as_str().unwrap_or(""))
        });
        let out = serde_json::json!({
            "schemaVersion": 1,
            "defaultModel": default_model,
            "models": entries,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    match AuthStatus::resolve(agent_config) {
        AuthStatus::ApiKey => println!("You are using XAI_API_KEY."),
        AuthStatus::LoggedIn(host) => println!("You are logged in with {}.", host),
        AuthStatus::ModelCredentials(model) => {
            println!("Model '{model}' is using its own API key.");
        }
        AuthStatus::DeploymentKey => println!("You are authenticated via deployment key."),
        AuthStatus::NotAuthenticated => println!("You are not authenticated."),
    }
    println!();

    println!("Default model: {default_model}");
    println!();
    println!("Available models:");
    let mut ids: Vec<_> = models.keys().cloned().collect();
    ids.sort();
    for id in ids {
        let billing = billing_for_model_id(&id);
        let mark = if id == default_model { "*" } else { "-" };
        // Annotate billing so subscription routes cannot be confused with
        // pay-per-token twins (e.g. openai-codex/* vs openai/*).
        println!("  {mark} {id} ({})", billing.as_str());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn billing_classifies_subscription_vs_pay_per_token() {
        assert_eq!(
            billing_for_model_id("openai-codex/gpt-5.6-luna"),
            BillingClass::Subscription
        );
        assert_eq!(
            billing_for_model_id("openai/gpt-5"),
            BillingClass::PayPerToken
        );
        assert_eq!(billing_for_model_id("grok-4.5"), BillingClass::Default);
        assert_eq!(
            billing_for_model_id("anthropic-claude/claude-sonnet-4"),
            BillingClass::Subscription
        );
        assert_eq!(
            billing_for_model_id("anthropic/claude-sonnet-4"),
            BillingClass::PayPerToken
        );
    }
}
