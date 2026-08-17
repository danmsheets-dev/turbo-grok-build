//! `browser_eval` — evaluate a JS function expression; JSON result only.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};

pub const BROWSER_EVAL_TOOL_NAME: &str = "browser_eval";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserEvalInput {
    /// Function expression, e.g. `() => document.title`.
    #[schemars(
        description = "JavaScript function expression that returns a JSON-serializable value, e.g. () => document.title. Result is capped at 20_000 bytes."
    )]
    pub function: String,
}

#[derive(Debug, Default)]
pub struct BrowserEvalTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserEvalTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Evaluate a JavaScript function expression in the Turbo Agent WebView and return JSON. Pass a function expression such as () => document.title. Do not dump the whole DOM. Result size is capped."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserEvalTool {
    type Args = BrowserEvalInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_EVAL_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_EVAL_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_eval", skip_all)]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserEvalInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let value = handle.eval(input.function).await?;
        Ok(super::json_output(&value))
    }
}
