//! `/steer <text>` — inject mid-turn guidance into the running agent.
//!
//! Send-now already exists as cancel-and-send (`Action::SendPromptNow`, the
//! send-now chord, and queue-row Interject). `/steer` is the named operator
//! slash: it injects without canceling when a turn is running, and sends as a
//! normal prompt when idle.

use xai_grok_tools::implementations::grok_build::STEER_TOOL_NAME;

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Same 16 KiB cap as the `steer` tool. Duplicated here because the tool
/// constant is crate-private; keep the numbers identical.
const STEER_MAX_BYTES: usize = 16 * 1024;

/// Parse `/steer` args. Empty text is an error; oversize text is refused.
pub fn parse_steer_args(args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Err("Usage: /steer <text>".to_owned());
    }
    if trimmed.len() > STEER_MAX_BYTES {
        return Err(format!(
            "Steer text is {} bytes; max is {STEER_MAX_BYTES} (16 KiB).",
            trimmed.len()
        ));
    }
    Ok(trimmed.to_string())
}

/// Inject mid-turn guidance (or send a prompt when idle).
pub struct SteerCommand;

impl SlashCommand for SteerCommand {
    fn name(&self) -> &str {
        STEER_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Steer the running agent without canceling"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/steer <text>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<text>")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        match parse_steer_args(args) {
            Ok(text) => CommandResult::Action(Action::Steer(text)),
            Err(msg) => CommandResult::Error(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;
    use crate::slash::command::CommandResult;

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

    fn run(args: &str) -> CommandResult {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            session_cwd: None,
            pager_state: PagerLocalSnapshot::default(),
        };
        SteerCommand.run(&mut ctx, args)
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(parse_steer_args("").is_err());
        assert!(parse_steer_args("   ").is_err());
        assert!(matches!(run(""), CommandResult::Error(msg) if msg.contains("/steer")));
    }

    #[test]
    fn parse_trims_and_dispatches_steer_action() {
        assert_eq!(
            parse_steer_args("  stay inside crates/foo  ").unwrap(),
            "stay inside crates/foo"
        );
        match run("stay inside crates/foo") {
            CommandResult::Action(Action::Steer(text)) => {
                assert_eq!(text, "stay inside crates/foo");
            }
            other => panic!("expected Action::Steer, got {other:?}"),
        }
    }

    #[test]
    fn parse_refuses_oversize_text() {
        let huge = "x".repeat(STEER_MAX_BYTES + 1);
        let err = parse_steer_args(&huge).unwrap_err();
        assert!(err.contains("16 KiB"), "{err}");
        assert!(matches!(run(&huge), CommandResult::Error(_)));
    }
}
