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
    #[schemars(
        description = "Snapshot uid from the last browser_snapshot (e.g. \"4-17\"). Epoch-index, not a positional \"2\"."
    )]
    pub uid: String,
    /// Confirm a click whose accessible name looks like submit / buy / pay / delete / post / send / apply / connect / message.
    #[serde(default)]
    #[schemars(
        description = "Set true after the user approved a submit/buy/pay/delete/post/send/apply/connect/message click. Default false. Sign in does not need confirm — the human types secrets in the window."
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
        "Click an element in the Turbo Agent WebView by snapshot uid. Call browser_snapshot first. Clicks whose name looks like submit/buy/pay/delete/post/send/apply/connect/message require confirm=true after the user approves. Sign in does not. Unknown uids fail closed."
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
        let result = handle.click(&input.uid, input.confirm).await?;
        Ok(super::text_output(format!(
            "Clicked uid {} — now at {} (title: {})",
            input.uid, result.url, result.title
        )))
    }
}

/// Accessible names that look like a consequential action.
///
/// Word-boundary matched, not raw `contains`: "Postal code" and "Payment
/// history" are not submit buttons, and treating them as such trains the model
/// to reach for `confirm=true` reflexively — which is how a real gate stops
/// meaning anything.
pub(super) fn click_name_needs_confirm(name: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "submit",
        "buy",
        "pay",
        "purchase",
        "order",
        "checkout",
        "confirm",
        "delete",
        "remove",
        "discard",
        "post",
        "send",
        "publish",
        "share",
        "transfer",
        "withdraw",
        "subscribe",
        "unsubscribe",
        "accept",
        "agree",
        "approve",
        "install",
        "uninstall",
        "deploy",
        "merge",
        "apply",
        "connect",
        "follow",
        "invite",
        "message",
        "login",
        "logout",
        "signup",
        "signin",
    ];
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().any(|word| {
        NEEDLES.iter().any(|needle| {
            // Match the word itself or a plural ("orders"), never a longer
            // unrelated word ("postal") and never a gerund: action buttons are
            // imperative ("Send"), while "-ing" forms are prose ("Ordering
            // information").
            *word == *needle
                || word
                    .strip_prefix(needle)
                    .is_some_and(|rest| matches!(rest, "s" | "es"))
        })
    })
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
    // A fallback snapshot numbers its uids over the accessibility tree, not the
    // tagged DOM. Acting on one would click a different element than the one
    // whose name was just checked.
    if !snapshot.source.uids_are_actionable() {
        return Err(ToolError::invalid_arguments(
            "The last browser_snapshot came from the accessibility-tree fallback, whose uids are \
             read-only. Call browser_snapshot again to get actionable uids."
                .to_owned(),
        ));
    }
    let Some(node) = snapshot.nodes.iter().find(|n| n.uid == uid) else {
        let hint = if uid.chars().all(|c| c.is_ascii_digit()) {
            " Uids look like 4-17 (epoch-index from the latest snapshot), not a positional 2."
        } else {
            ""
        };
        return Err(ToolError::invalid_arguments(format!(
            "Unknown snapshot uid {uid}.{hint} Call browser_snapshot and use a current uid."
        )));
    };
    if confirm || skip_click_confirm() || !click_name_needs_confirm(&node.name) {
        return Ok(());
    }
    Err(ToolError::invalid_arguments(format!(
        "Click on {:?} looks like a consequential action (submit / pay / delete / post / send). \
         Ask the user, then retry browser_click with confirm=true.",
        node.name
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_browser::{AxNode, SnapshotSource};

    fn snapshot(source: SnapshotSource, uid: &str, name: &str) -> SnapshotResult {
        SnapshotResult {
            url: "https://example.com/".into(),
            title: "Example".into(),
            source,
            nodes: vec![AxNode {
                uid: uid.into(),
                role: "button".into(),
                name: name.into(),
                value: None,
                focused: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn consequential_names_need_confirmation() {
        for name in [
            "Submit",
            "Buy now",
            "Pay $40",
            "Delete account",
            "Place order",
            "Checkout",
            "Confirm",
            "Transfer funds",
            "Publish",
            "Send message",
            "Remove",
            "I agree",
            "Withdraw",
            "Merge pull request",
            "Apply now",
            "Apply with Indeed",
            "Easy Apply",
            "Connect",
            "Follow",
            "Invite",
            "Message",
        ] {
            assert!(click_name_needs_confirm(name), "must gate {name:?}");
        }
    }

    #[test]
    fn ordinary_names_do_not() {
        // Substring matching gated all of these; over-prompting is how a gate
        // stops being read.
        for name in [
            "Postal code",
            "Payment history",
            "Sender",
            "Compose",
            "Ordering information",
            "Deleted items",
            "More information",
            "Search",
            "Documentation",
            "Sign in",
            "Sign in with email",
            "Continue with Google",
        ] {
            assert!(!click_name_needs_confirm(name), "must not gate {name:?}");
        }
    }

    #[test]
    fn fallback_snapshot_uids_are_refused() {
        let snap = snapshot(SnapshotSource::AxFallback, "ax-1", "More information");
        let err = check_click_against_snapshot(Some(&snap), "ax-1", false).unwrap_err();
        assert!(err.to_string().contains("read-only"), "{err}");
        // Even with confirm: the uid still does not point at that element.
        assert!(check_click_against_snapshot(Some(&snap), "ax-1", true).is_err());
    }

    #[test]
    fn positional_uid_is_explained() {
        let snap = snapshot(SnapshotSource::Dom, "1-1", "Search");
        let err = check_click_against_snapshot(Some(&snap), "2", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("4-17") || msg.contains("epoch"), "{msg}");
    }

    #[test]
    fn missing_snapshot_fails_closed() {
        let err = check_click_against_snapshot(None, "1-1", true).unwrap_err();
        assert!(err.to_string().contains("browser_snapshot"), "{err}");
    }

    #[test]
    fn dom_snapshot_allows_a_benign_click() {
        let snap = snapshot(SnapshotSource::Dom, "1-1", "More information");
        assert!(check_click_against_snapshot(Some(&snap), "1-1", false).is_ok());
        let gated = snapshot(SnapshotSource::Dom, "1-1", "Delete account");
        assert!(check_click_against_snapshot(Some(&gated), "1-1", false).is_err());
        assert!(check_click_against_snapshot(Some(&gated), "1-1", true).is_ok());
    }
}
