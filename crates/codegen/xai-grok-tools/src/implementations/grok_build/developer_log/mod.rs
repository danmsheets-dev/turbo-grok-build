//! `developer_log` — file a structured product incident for Turbo maintainers.
//!
//! Writes into `$GROK_HOME/developer-log/` (deduped by fingerprint, redacted).
//! Operators export packs with `turbo issues export`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use xai_tool_runtime::{Cwd, SessionContext};

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const DEVELOPER_LOG_TOOL_NAME: &str = "developer_log";

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeveloperLogInput {
    #[schemars(
        description = "Short product-facing title (what broke or is missing), e.g. 'Subagent worktree path unusable after complete'."
    )]
    pub title: String,

    #[schemars(
        description = "1–3 sentence summary of the product issue and impact on agents/users."
    )]
    pub summary: String,

    #[schemars(
        description = "Stable error class for dedup/triage: worktree_tombstone | isolation_fallback | subagent_stall | protocol_deser | provider_400 | provider_auth | tool_schema | land_conflict | mcp_connect | catalog_stale | docs_gap | feature_gap | perf_regression | work_lost_risk | unknown"
    )]
    pub error_class: String,

    #[serde(default)]
    #[schemars(
        description = "Optional kind: bug | product_friction | feature_gap | provider_compat | docs_gap | perf | unknown"
    )]
    pub kind: Option<String>,

    #[serde(default)]
    #[schemars(description = "Optional severity: p0 | p1 | p2 | p3 (defaults from error_class)")]
    pub severity: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Product components, e.g. [\"subagent\",\"worktree\",\"lifecycle\"]"
    )]
    pub component: Option<Vec<String>>,

    #[serde(default)]
    #[schemars(description = "Ordered repro steps when known")]
    pub repro_steps: Option<Vec<String>>,

    #[serde(default)]
    #[schemars(description = "What should have happened")]
    pub expected: Option<String>,

    #[serde(default)]
    #[schemars(description = "What actually happened")]
    pub actual: Option<String>,

    #[serde(default)]
    #[schemars(description = "Suggested product fix for maintainers")]
    pub suggested_fix: Option<String>,

    #[serde(default)]
    #[schemars(description = "Optional tags")]
    pub tags: Option<Vec<String>>,

    #[serde(default)]
    #[schemars(description = "Provider id if relevant, e.g. platform/nvidia")]
    pub provider: Option<String>,

    #[serde(default)]
    #[schemars(description = "Model id if relevant")]
    pub model: Option<String>,

    #[serde(default)]
    #[schemars(description = "Related subagent id")]
    pub subagent_id: Option<String>,

    #[serde(default)]
    #[schemars(description = "Path to subagent meta.json or other durable evidence")]
    pub meta_path: Option<String>,

    #[serde(default)]
    #[schemars(description = "Git snapshot ref if known")]
    pub snapshot_ref: Option<String>,

    #[serde(default)]
    #[schemars(description = "Path to changes.patch if known")]
    pub patch_path: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Optional explicit fingerprint for dedup. Prefer leaving empty so the store computes one from error_class + components."
    )]
    pub fingerprint: Option<String>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DeveloperLogOutput {
    pub success: bool,
    pub incident_id: String,
    pub fingerprint: String,
    pub is_new: bool,
    pub occurrence_count: u32,
    pub severity: String,
    pub error_class: String,
    pub path: String,
    pub message: String,
}

impl xai_tool_runtime::ToolOutput for DeveloperLogOutput {
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
pub struct DeveloperLogTool;

impl crate::types::tool_metadata::ToolMetadata for DeveloperLogTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"REQUIRED for Turbo product issues: file a structured incident into the Auto Developer Log for maintainers.

**Always call this tool** when Turbo product behavior blocks you or surprises supervisors — worktrees deleted/hard to find, land/diff pollution, provider deser failures, isolation fallback, stalls, MCP failures, missing features, docs gaps. Do not rely on chat alone for product bugs.

Rules:
- Prefer a stable `error_class` (worktree_tombstone, work_lost_risk, subagent_stall, protocol_deser, provider_400, feature_gap, docs_gap, land_conflict, isolation_fallback, mcp_connect, unknown, …).
- One call per distinct product issue; the store dedups by fingerprint and increments occurrence_count.
- Never include secrets, API keys, tokens, or full unredacted prompts.
- Do not spam: if you already filed this fingerprint in the session, skip unless new evidence.

Storage: default `$GROK_HOME/developer-log`; override with `GROK_DEVELOPER_LOG_DIR` or `turbo issues set-dir <path>`.
Operators review with `turbo issues list`, `turbo issues export`, `turbo issues path`."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        // Writes only under ~/.grok/developer-log; not the user workspace.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool_metadata::ToolMetadata;

    #[test]
    fn tool_name_and_description_are_turbo_branded() {
        assert_eq!(DEVELOPER_LOG_TOOL_NAME, "developer_log");
        let t = DeveloperLogTool;
        assert!(
            t.description_template().contains("Turbo"),
            "description must brand Turbo, not Hyper only"
        );
        assert!(!t.description_template().contains("Hyper product"));
    }
}

impl xai_tool_runtime::Tool for DeveloperLogTool {
    type Args = DeveloperLogInput;
    type Output = DeveloperLogOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(DEVELOPER_LOG_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            DEVELOPER_LOG_TOOL_NAME,
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
        name = "tool.developer_log",
        skip_all,
        fields(error_class = %input.error_class, title = %input.title)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: DeveloperLogInput,
    ) -> Result<DeveloperLogOutput, xai_tool_runtime::ToolError> {
        if !xai_grok_developer_log::is_enabled() {
            return Err(xai_tool_runtime::ToolError::custom(
                "developer_log_disabled",
                format!(
                    "Auto Developer Log is disabled ({}=0). Enable it to file product issues.",
                    xai_grok_developer_log::ENABLED_ENV
                ),
            ));
        }

        let error_class = xai_grok_developer_log::ErrorClass::parse(&input.error_class)
            .unwrap_or(xai_grok_developer_log::ErrorClass::Unknown);
        let kind = input
            .kind
            .as_deref()
            .and_then(xai_grok_developer_log::IncidentKind::parse);
        let severity = input
            .severity
            .as_deref()
            .and_then(xai_grok_developer_log::Severity::parse);

        let session_id = ctx.get::<SessionContext>().map(|s| s.0.clone());
        let cwd_hash = ctx.get::<Cwd>().map(|c| {
            xai_grok_config::encode_cwd_dirname(&c.0.to_string_lossy())
        });

        let request = xai_grok_developer_log::ReportRequest {
            title: input.title,
            summary: input.summary,
            kind,
            severity,
            error_class,
            component: input.component.unwrap_or_default(),
            environment: xai_grok_developer_log::Environment {
                session_id,
                subagent_id: input.subagent_id,
                provider: input.provider,
                model: input.model.clone(),
                cwd_hash,
                ..Default::default()
            },
            repro: xai_grok_developer_log::Repro {
                steps: input.repro_steps.unwrap_or_default(),
                expected: input.expected,
                actual: input.actual,
                confidence: xai_grok_developer_log::ReproConfidence::Medium,
            },
            evidence: xai_grok_developer_log::Evidence {
                meta_path: input.meta_path,
                snapshot_ref: input.snapshot_ref,
                patch_path: input.patch_path,
                related_events: vec!["agent.developer_log".into()],
                ..Default::default()
            },
            suggested_fix: input.suggested_fix,
            source: xai_grok_developer_log::Source {
                reporter: xai_grok_developer_log::ReporterKind::Agent,
                auto: false,
                reporter_model: input.model,
                tool: Some(DEVELOPER_LOG_TOOL_NAME.into()),
                detector: None,
            },
            tags: input.tags.unwrap_or_default(),
            fingerprint: input.fingerprint,
        };

        // File I/O is sync and fast; run off the async executor.
        let result = tokio::task::spawn_blocking(move || {
            xai_grok_developer_log::DeveloperLogStore::default().report(request)
        })
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "developer_log_join",
                format!("developer_log task failed: {e}"),
            )
        })?
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom("developer_log_failed", e.to_string())
        })?;

        let action = if result.is_new {
            "Created"
        } else {
            "Updated"
        };
        let message = format!(
            "{action} Auto Developer Log incident `{}` (fingerprint `{}`, occurrences={}, severity={}). Path: {}. Review with `turbo issues show {}` or `turbo issues export`.",
            result.incident_id,
            result.fingerprint,
            result.occurrence_count,
            result.severity.as_str(),
            result.path,
            result.incident_id,
        );

        Ok(DeveloperLogOutput {
            success: true,
            incident_id: result.incident_id,
            fingerprint: result.fingerprint,
            is_new: result.is_new,
            occurrence_count: result.occurrence_count,
            severity: result.severity.as_str().to_string(),
            error_class: result.error_class.as_str().to_string(),
            path: result.path,
            message,
        })
    }
}
