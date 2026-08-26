//! `/rollback [receipt_id]` — revert the last undoable agent write receipt.
//!
//! Receipts were added in rc8 (`receipts` / `rollback` tools). This slash is
//! the operator surface: it restores the newest undoable edit receipt for the
//! session, or a specific `rcpt-...` id. Bash receipts stay audit-only.

use xai_grok_tools::implementations::grok_build::ROLLBACK_TOOL_NAME;

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Parse `/rollback` args.
///
/// `None` means "newest undoable edit". `last` / `latest` are aliases for that.
/// Any other non-empty token is treated as a receipt id.
pub fn parse_rollback_args(args: &str) -> Option<String> {
    let trimmed = args.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("last")
        || trimmed.eq_ignore_ascii_case("latest")
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// Revert the last (or named) undoable edit receipt.
pub struct RollbackCommand;

impl SlashCommand for RollbackCommand {
    fn name(&self) -> &str {
        ROLLBACK_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Revert the last undoable agent write"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/rollback [receipt_id]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[receipt_id]")
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }
        CommandResult::Action(Action::RollbackLast {
            receipt_id: parse_rollback_args(args),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;
    use crate::slash::command::CommandResult;
    use agent_client_protocol as acp;

    static DEFAULT_BUNDLE_STATE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn run(args: &str, session: bool) -> CommandResult {
        let models = ModelState::default();
        let sid = acp::SessionId::from("s1".to_string());
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: session.then_some(&sid),
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            session_cwd: None,
            pager_state: PagerLocalSnapshot::default(),
        };
        RollbackCommand.run(&mut ctx, args)
    }

    #[test]
    fn parse_empty_and_last_mean_newest() {
        assert_eq!(parse_rollback_args(""), None);
        assert_eq!(parse_rollback_args("  "), None);
        assert_eq!(parse_rollback_args("last"), None);
        assert_eq!(parse_rollback_args("LATEST"), None);
    }

    #[test]
    fn parse_receipt_id() {
        assert_eq!(
            parse_rollback_args("  rcpt-abc  ").as_deref(),
            Some("rcpt-abc")
        );
    }

    #[test]
    fn no_session_errors() {
        assert!(matches!(
            run("", false),
            CommandResult::Error(msg) if msg.contains("No active session")
        ));
    }

    #[test]
    fn with_session_dispatches_last() {
        match run("", true) {
            CommandResult::Action(Action::RollbackLast { receipt_id }) => {
                assert!(receipt_id.is_none());
            }
            other => panic!("expected RollbackLast, got {other:?}"),
        }
    }

    #[test]
    fn with_session_dispatches_named_id() {
        match run("rcpt-xyz", true) {
            CommandResult::Action(Action::RollbackLast { receipt_id }) => {
                assert_eq!(receipt_id.as_deref(), Some("rcpt-xyz"));
            }
            other => panic!("expected RollbackLast, got {other:?}"),
        }
    }
}
