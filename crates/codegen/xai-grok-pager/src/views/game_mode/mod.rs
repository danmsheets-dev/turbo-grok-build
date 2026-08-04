//! Game Mode — terminal-native office view of Supervisor + subagent desks.
//!
//! Spec: `docs/design-game-mode-rc11.md`
//! Toggle: `Ctrl+G` (`ActionId::ToggleGameMode`).

mod compose;
mod layout;
mod monitor;
mod render;
mod sprites;
mod sprites_pixel;
mod state;
mod wall;

pub use compose::{
    OFFICE_BG_PNG, compose_cell_frame, compose_cell_frame_into, encode_png, load_office_background,
    scale_bg_to_cells, scale_bg_to_cells_with_scale,
};
pub use layout::{GameLayout, GameTier, SpriteSet, compute as compute_layout, game_tier};
pub use render::render_game_mode;
pub use sprites_pixel::{
    DevPalette, PIXEL_SCALE, effective_pixel_scale, pixel_scale, sprite_developer_at_desk,
    sprite_developer_walk,
};
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

/// Sync + optional tick Game Mode. Returns whether a redraw is warranted.
///
/// **Single-owner:** prefer calling from [`crate::app::app_view::AppView::tick`].
/// Paint path should call only when [`GameModeState::needs_paint_sync`].
///
/// PERF: AppView keeps Game Mode on [`crate::app::app_view::TickDemand::Slow`]
/// (~12 Hz). Pixel recompose is fingerprint-gated; redraw is dirty-gated so
/// frozen idle rooms do not force a full office paint every Slow tick.
pub fn sync_game_mode(agent: &mut AgentView, stage_width: u16, stage_height: u16) -> bool {
    if !agent.game_mode.open {
        return false;
    }
    let waiting_on_user =
        !agent.permission_queue.is_empty() || agent.question_view.is_some();
    let working = supervisor_is_working(agent);
    let area = ratatui::layout::Rect::new(0, 0, stage_width, stage_height);
    let layout = compute_layout(area);
    let tier = layout.tier;

    let wall_before = agent.game_mode.wall;
    let seats_before = agent.game_mode.active_desk_count();
    let phase_sig_before = phase_signature(&agent.game_mode);

    let snaps = snapshots_from_subagents(&agent.subagent_sessions);
    agent
        .game_mode
        .sync_from_snapshots(&snaps, working, tier, waiting_on_user);

    if agent.game_mode.wall != wall_before
        || agent.game_mode.active_desk_count() != seats_before
        || phase_signature(&agent.game_mode) != phase_sig_before
    {
        agent.game_mode.mark_redraw_dirty();
    }

    agent.game_mode.last_sync_at = Some(std::time::Instant::now());

    // ~10–12 Hz walk/blink (matches TickDemand::Slow).
    let elapsed = agent.game_mode.last_tick.elapsed();
    if elapsed >= Duration::from_millis(90) {
        agent.game_mode.tick_anim(tier);
    }

    agent.game_mode.take_redraw_dirty()
}

fn phase_signature(state: &GameModeState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for d in &state.desks {
        d.child_session_id.hash(&mut h);
        (d.phase as u8).hash(&mut h);
        d.failed.hash(&mut h);
    }
    (state.supervisor as u8).hash(&mut h);
    h.finish()
}
