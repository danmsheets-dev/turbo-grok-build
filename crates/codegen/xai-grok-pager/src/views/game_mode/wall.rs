//! Large wall display state machine.

use super::state::DeskAgentSnapshot;

/// High-level wall banner mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallMode {
    Standby,
    Working,
    SupervisorBusy,
    WorkFinished,
    NeedsAttention,
    WaitingOnYou,
}

impl WallMode {
    pub fn title(self) -> &'static str {
        match self {
            Self::Standby => "WAITING FOR ORDERS",
            Self::Working => "WORKING",
            Self::SupervisorBusy => "SUPERVISOR BUSY",
            Self::WorkFinished => "WORK FINISHED",
            Self::NeedsAttention => "NEEDS ATTENTION",
            Self::WaitingOnYou => "WAITING ON YOU",
        }
    }

    pub fn is_success_pulse(self) -> bool {
        matches!(self, Self::WorkFinished)
    }
}

/// Derive wall mode from live agent snapshots + supervisor + session flags.
///
/// `attention_active` is a brief sticky window after a failure (not forever).
/// Running work outranks sticky attention so the wall shows WORKING while
/// other agents continue.
pub fn compute_wall_mode(
    agents: &[DeskAgentSnapshot],
    supervisor_working: bool,
    had_success: bool,
    handoff_in_flight: bool,
    attention_active: bool,
) -> WallMode {
    let any_running = agents.iter().any(|a| a.running) || handoff_in_flight;
    if any_running {
        return WallMode::Working;
    }
    if supervisor_working {
        return WallMode::SupervisorBusy;
    }
    if attention_active {
        return WallMode::NeedsAttention;
    }
    if had_success {
        return WallMode::WorkFinished;
    }
    WallMode::Standby
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn agent(running: bool, failed: bool) -> DeskAgentSnapshot {
        DeskAgentSnapshot {
            child_session_id: "c".into(),
            label: "c".into(),
            subagent_type: "general".into(),
            running,
            failed,
            elapsed: Duration::ZERO,
            tokens: 0,
            tool_calls: 0,
            activity: String::new(),
        }
    }

    #[test]
    fn work_finished_requires_success() {
        assert_eq!(
            compute_wall_mode(&[], false, false, false, false),
            WallMode::Standby
        );
        assert_eq!(
            compute_wall_mode(&[], false, true, false, false),
            WallMode::WorkFinished
        );
    }

    #[test]
    fn working_beats_finished_and_attention() {
        assert_eq!(
            compute_wall_mode(&[agent(true, false)], false, true, false, true),
            WallMode::Working
        );
    }

    #[test]
    fn failed_needs_attention_when_sticky() {
        assert_eq!(
            compute_wall_mode(&[agent(false, true)], false, false, false, true),
            WallMode::NeedsAttention
        );
        assert_eq!(
            compute_wall_mode(&[agent(false, true)], false, false, false, false),
            WallMode::Standby
        );
    }
}
