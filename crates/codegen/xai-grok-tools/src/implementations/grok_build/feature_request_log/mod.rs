//! `feature_request_log` — file a structured product capability request.
//!
//! Writes into `$GROK_HOME/feature-request-log/` (deduped by fingerprint, redacted).
//! Operators export packs with `turbo features export`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use xai_tool_runtime::{Cwd, SessionContext};

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const FEATURE_REQUEST_LOG_TOOL_NAME: &str = "feature_request_log";

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FeatureRequestLogInput {
    #[schemars(
        description = "Short product title for the missing capability, e.g. 'Keep-N concurrent art workers scheduler'."
    )]
    pub title: String,

    #[schemars(
        description = "1–3 sentence summary of the capability gap and why agents need it during harness work."
    )]
    pub summary: String,

    #[schemars(
        description = "Stable request class for dedup/triage: tool_surface | workflow | subagent | ui_ux | provider_model | mcp_integration | documentation | performance | api_surface | scheduler | extensibility | other"
    )]
    pub request_class: String,

    #[serde(default)]
    #[schemars(
        description = "Optional priority: must_have | should_have | nice_to_have | exploratory (defaults from request_class)"
    )]
    pub priority: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Product components, e.g. [\"subagent\",\"land\",\"scheduler\"]"
    )]
    pub component: Option<Vec<String>>,

    #[serde(default)]
    #[schemars(description = "Concrete harness / user scenario that needs this capability")]
    pub use_case: Option<String>,

    #[serde(default)]
    #[schemars(description = "What agents do today without the feature (manual steps, hacks)")]
    pub current_workaround: Option<String>,

    #[serde(default)]
    #[schemars(description = "Desired product behavior or API shape")]
    pub proposed_behavior: Option<String>,

    #[serde(default)]
    #[schemars(description = "Optional acceptance criteria (short bullets)")]
    pub acceptance_criteria: Option<Vec<String>>,

    #[serde(default)]
    #[schemars(description = "Optional tags")]
    pub tags: Option<Vec<String>>,

    #[serde(default)]
    #[schemars(description = "Provider id if relevant")]
    pub provider: Option<String>,

    #[serde(default)]
    #[schemars(description = "Model id if relevant")]
    pub model: Option<String>,

    #[serde(default)]
    #[schemars(description = "Related subagent id")]
    pub subagent_id: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Optional explicit fingerprint for dedup. Prefer leaving empty so the store computes one from request_class + title + components."
    )]
    pub fingerprint: Option<String>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FeatureRequestLogOutput {
    pub success: bool,
    pub request_id: String,
    pub fingerprint: String,
    pub is_new: bool,
    pub occurrence_count: u32,
    pub priority: String,
    pub request_class: String,
    pub path: String,
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for FeatureRequestLogOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct FeatureRequestLogTool;

impl crate::types::tool_metadata::ToolMetadata for FeatureRequestLogTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"File a structured product **feature request** when harness work needs a Turbo capability that does not exist yet (missing tool, workflow, scheduler keep-N, land merge helper, UI affordance, etc.).

Use this for **missing product surface**, not for bugs:
- Bugs / friction / broken behavior → `developer_log`
- Missing capability agents need to complete work → `feature_request_log`

Rules:
- Prefer a stable `request_class` (tool_surface, workflow, subagent, scheduler, ui_ux, …).
- One call per distinct request; the store dedups by fingerprint and increments occurrence_count.
- Include `use_case` and `current_workaround` when known so product can prioritize.
- Never include secrets, tokens, or full unredacted prompts.

Storage: default `$GROK_HOME/feature-request-log`; override with `GROK_FEATURE_REQUEST_LOG_DIR` or `turbo features set-dir <path>`.
Operators review with `turbo features list`, `turbo features export`, `turbo features path`."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        // Writes only under ~/.grok/feature-request-log; not the user workspace.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool_metadata::ToolMetadata;

    #[test]
    fn tool_name_is_registered_constant() {
        assert_eq!(FEATURE_REQUEST_LOG_TOOL_NAME, "feature_request_log");
        let t = FeatureRequestLogTool;
        assert!(t.description_template().contains("feature_request_log"));
        assert!(t.description_template().contains("Turbo"));
    }
}

impl xai_tool_runtime::Tool for FeatureRequestLogTool {
    type Args = FeatureRequestLogInput;
    type Output = FeatureRequestLogOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(FEATURE_REQUEST_LOG_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            FEATURE_REQUEST_LOG_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.feature_request_log",
        skip_all,
        fields(request_class = %input.request_class, title = %input.title)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: FeatureRequestLogInput,
    ) -> Result<FeatureRequestLogOutput, xai_tool_runtime::ToolError> {
        if !xai_grok_developer_log::fr_is_enabled() {
            return Err(xai_tool_runtime::ToolError::custom(
                "feature_request_log_disabled",
                format!(
                    "Feature Request Log is disabled ({}=0). Enable it to file capability requests.",
                    xai_grok_developer_log::FR_ENABLED_ENV
                ),
            ));
        }

        let request_class = xai_grok_developer_log::RequestClass::parse(&input.request_class)
            .unwrap_or(xai_grok_developer_log::RequestClass::Other);
        let priority = input
            .priority
            .as_deref()
            .and_then(xai_grok_developer_log::RequestPriority::parse);

        let session_id = ctx.get::<SessionContext>().map(|s| s.0.clone());
        let cwd_hash = ctx.get::<Cwd>().map(|c| {
            xai_grok_config::encode_cwd_dirname(&c.0.to_string_lossy())
        });

        let request = xai_grok_developer_log::FeatureRequestReport {
            title: input.title,
            summary: input.summary,
            request_class,
            priority,
            component: input.component.unwrap_or_default(),
            use_case: input.use_case,
            current_workaround: input.current_workaround,
            proposed_behavior: input.proposed_behavior,
            acceptance_criteria: input.acceptance_criteria.unwrap_or_default(),
            environment: xai_grok_developer_log::Environment {
                session_id,
                subagent_id: input.subagent_id,
                provider: input.provider,
                model: input.model.clone(),
                cwd_hash,
                ..Default::default()
            },
            evidence: xai_grok_developer_log::Evidence {
                related_events: vec!["agent.feature_request_log".into()],
                ..Default::default()
            },
            source: xai_grok_developer_log::agent_source(
                FEATURE_REQUEST_LOG_TOOL_NAME,
                input.model,
            ),
            tags: input.tags.unwrap_or_default(),
            fingerprint: input.fingerprint,
        };

        let result = tokio::task::spawn_blocking(move || {
            xai_grok_developer_log::FeatureRequestStore::default().report(request)
        })
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "feature_request_log_join",
                format!("feature_request_log task failed: {e}"),
            )
        })?
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("feature_request_log_failed", e.to_string())
        })?;

        let action = if result.is_new {
            "Created"
        } else {
            "Updated"
        };
        let message = format!(
            "{action} Feature Request Log entry `{}` (fingerprint `{}`, occurrences={}, priority={}, class={}). Path: {}. Review with `turbo features show {}` or `turbo features export`.",
            result.request_id,
            result.fingerprint,
            result.occurrence_count,
            result.priority.as_str(),
            result.request_class.as_str(),
            result.path,
            result.request_id,
        );

        Ok(FeatureRequestLogOutput {
            success: true,
            request_id: result.request_id,
            fingerprint: result.fingerprint,
            is_new: result.is_new,
            occurrence_count: result.occurrence_count,
            priority: result.priority.as_str().to_string(),
            request_class: result.request_class.as_str().to_string(),
            path: result.path,
            message,
        })
    }
}
