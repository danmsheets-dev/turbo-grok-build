//! `browser_fill` — type into a snapshot uid. Refuses OTP/password-shaped values.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_FILL_TOOL_NAME: &str = "browser_fill";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserFillInput {
    /// Snapshot uid from the last `browser_snapshot`.
    #[schemars(
        description = "Snapshot uid from the last browser_snapshot (e.g. \"4-17\"). Epoch-index, not a positional \"2\"."
    )]
    pub uid: String,
    /// Value to insert. OTP / password-shaped values are rejected.
    #[schemars(
        description = "Text to type into the element. One-time passwords and password-shaped secrets are rejected."
    )]
    pub value: String,
}

#[derive(Debug, Default)]
pub struct BrowserFillTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserFillTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Fill a text field in the Turbo Agent WebView by snapshot uid. Refuses OTP/PIN (6–8 digits) and password-shaped values. Never automate 2FA — the human types secrets in the Agent window."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserFillTool {
    type Args = BrowserFillInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_FILL_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_FILL_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.browser_fill", skip_all, fields(uid = %input.uid))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserFillInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        handle.fill(&input.uid, &input.value).await?;
        Ok(super::text_output(format!("Filled uid {}", input.uid)))
    }
}
