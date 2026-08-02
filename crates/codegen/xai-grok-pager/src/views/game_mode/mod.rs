//! Game Mode — terminal-native office view of Supervisor + subagent desks.
//!
//! Spec: `docs/design-game-mode-rc11.md`
//! Toggle: `Shift+G` (`ActionId::ToggleGameMode`).

mod compose;
mod layout;
mod monitor;
mod render;
mod sprites;
mod sprites_pixel;
mod state;
mod wall;

pub use compose::{
    OFFICE_BG_PNG, compose_cell_frame, encode_png, load_office_background, scale_bg_to_cells,
};
pub use layout::{GameLayout, GameTier, SpriteSet, compute as compute_layout, game_tier};
pub use render::render_game_mode;
pub use sprites_pixel::{DevPalette, sprite_developer_at_desk, sprite_developer_walk};
pub use state::{
    ActorPhase, DESK_COUNT, DeskAgentSnapshot, DeskSlot, GameModeState, SupervisorPhase,
};
pub use wall::WallMode;

use crate::app::agent_view::AgentView;
use crate::app::subagent::SubagentInfo;
use std::time::Duration;

/// Build snapshots from live subagent map for Game Mode sync.
pub fn snapshots_from_subagents(
    sessions: &std::collections::HashMap<String, SubagentInfo>,
) -> Vec<DeskAgentSnapshot> {
    let mut out: Vec<DeskAgentSnapshot> = sessions
        .values()
        .map(|info| {
            let running = info.is_running();
            let failed = info
                .status
                .as_ref()
                .map(|s| {
                    let s = s.as_ref();
                    s == "failed" || s == "cancelled"
                })
                .unwrap_or(false);
            let tokens = info
                .tokens_used
                .or_else(|| info.usage.as_ref().map(|u| u.totals.total_tokens))
                .unwrap_or(0);
            let tool_calls = info.tool_call_count.or(info.tool_calls).unwrap_or(0);
            let label = if !info.description.is_empty() {
                info.description.as_ref().to_string()
            } else {
                info.subagent_type.as_ref().to_string()
            };
            DeskAgentSnapshot {
                child_session_id: info.child_session_id.as_ref().to_string(),
                label,
                subagent_type: info.subagent_type.as_ref().to_string(),
                running,
                failed: failed && !running,
                elapsed: info.display_elapsed(),
                tokens,
                tool_calls,
                activity: info.activity_label.clone().unwrap_or_default(),
            }
        })
        .collect();
    // Stable order for deterministic seating of first-seen agents.
    out.sort_by(|a, b| a.child_session_id.cmp(&b.child_session_id));
    out
}

/// Whether the main agent (Supervisor) is actively working a turn.
pub fn supervisor_is_working(agent: &AgentView) -> bool {
    agent.session.state.is_turn_running() || agent.session.state.is_cancelling()
}

/// Sync + tick Game Mode from the live agent view. Call while open on each draw/tick.
///
/// Uses the same [`compute_layout`] path as render so tier (Compact vs office)
/// matches painting (status strip is peeled inside `compute`).
pub fn sync_game_mode(agent: &mut AgentView, stage_width: u16, stage_height: u16) {
    if !agent.game_mode.open {
        return;
    }
    let snaps = snapshots_from_subagents(&agent.subagent_sessions);
    let working = supervisor_is_working(agent);
    let area = ratatui::layout::Rect::new(0, 0, stage_width, stage_height);
    let layout = compute_layout(area);
    let tier = layout.tier;
    agent.game_mode.sync_from_snapshots(&snaps, working, tier);

    // ~8–10 Hz is enough for walk/blink; full recompose is cheap at cell res.
    let elapsed = agent.game_mode.last_tick.elapsed();
    if elapsed >= Duration::from_millis(120) {
        agent.game_mode.tick_anim(tier);
    }
}
