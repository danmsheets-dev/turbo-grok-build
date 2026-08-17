//! `browser_click` — click a snapshot uid in the Agent WebView.

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use xai_grok_browser::SnapshotResult;
use xai_tool_runtime::ToolError;

pub const BROWSER_CLICK_TOOL_NAME: &str = "browser_click";

/// Set to `1` to skip submit/buy/pay/delete/post/send click confirmation.
pub const GROK_BROWSER_SKIP_CLICK_CONFIRM_ENV: &str = "GROK_BROWSER_SKIP_CLICK_CONFIRM";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserClickInput {
    /// Snapshot uid from the last `browser_snapshot`.
    #[schemars(description = "Snapshot uid from the last browser_snapshot (e.g. \"1\").")]
    pub uid: String,
    /// Confirm a click whose accessible name looks like submit / buy / pay / delete / post / send.
    #[serde(default)]
    #[schemars(
        description = "Set true after the user approved a submit/buy/pay/delete/post/send click. Default false."
    )]
    pub confirm: bool,
}

#[derive(Debug, Default)]
pub struct BrowserClickTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserClickTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Click an element in the Turbo Agent WebView by snapshot uid. Call browser_snapshot first. Clicks whose name looks like submit/buy/pay/delete/post/send require confirm=true after the user approves. Unknown uids fail closed."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserClickTool {
    type Args = BrowserClickInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_CLICK_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_CLICK_TOOL_NAME,
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

    #[tracing::instrument(name = "tool.browser_click", skip_all, fields(uid = %input.uid))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserClickInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        handle.click(&input.uid, input.confirm).await?;
        Ok(super::text_output(format!("Clicked uid {}", input.uid)))
    }
}

/// Accessible names that look like a submit / pay / delete action (plan regex).
pub(super) fn click_name_needs_confirm(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const NEEDLES: &[&str] = &["submit", "buy", "pay", "delete", "post", "send"];
    NEEDLES.iter().any(|needle| n.contains(needle))
}

fn skip_click_confirm() -> bool {
    std::env::var(GROK_BROWSER_SKIP_CLICK_CONFIRM_ENV).is_ok_and(|v| v == "1")
}

/// Fail closed without a cached snapshot. Require `confirm` for submit-shaped names.
pub(super) fn check_click_against_snapshot(
    snapshot: Option<&SnapshotResult>,
    uid: &str,
    confirm: bool,
) -> Result<(), ToolError> {
    let Some(snapshot) = snapshot else {
        return Err(ToolError::invalid_arguments(
            "Call browser_snapshot first, then retry browser_click. \
             Click requires a snapshot so submit/pay/delete actions can be confirmed."
                .to_owned(),
        ));
    };
    let Some(node) = snapshot.nodes.iter().find(|n| n.uid == uid) else {
        return Err(ToolError::invalid_arguments(format!(
            "Unknown snapshot uid {uid}. Call browser_snapshot and use a current uid."
        )));
    };
    if confirm || skip_click_confirm() || !click_name_needs_confirm(&node.name) {
        return Ok(());
    }
    Err(ToolError::invalid_arguments(format!(
        "Click on {:?} looks like a submit/pay/delete action. Ask the user, then retry \
         browser_click with confirm=true.",
        node.name
    )))
}
