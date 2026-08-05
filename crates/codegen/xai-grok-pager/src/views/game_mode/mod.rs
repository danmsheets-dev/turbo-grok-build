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
pub use layout::{
    GameLayout, GameTier, SpriteSet, compute as compute_layout, game_tier, stage_rect,
};
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
use crate::app::app_view::SLOW_TICK_INTERVAL;
use crate::app::subagent::SubagentInfo;
use std::time::Duration;

/// Jitter margin below [`SLOW_TICK_INTERVAL`] tolerated by the animation gate.
///
/// The event loop cannot deliver a Slow tick *earlier* than the interval, but it
/// can be a hair late/early after clamping — without a margin a gate equal to the
/// interval would drop every other tick.
const ANIM_TICK_JITTER: Duration = Duration::from_millis(8);

/// Minimum wall time between animation steps, derived from [`SLOW_TICK_INTERVAL`]
/// so the two cannot drift apart.
///
/// Must stay `<= SLOW_TICK_INTERVAL` (pinned by `anim_gate_fires_every_slow_tick`):
/// a hardcoded 90 ms gate sat *above* the 83 ms interval and halved the effective
/// animation rate to ~6 Hz (RC16 BUG-2).
fn anim_tick_gate() -> Duration {
    SLOW_TICK_INTERVAL.saturating_sub(ANIM_TICK_JITTER)
}

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
///
/// `stage_width`/`stage_height` are the **stage** dims — the paint area with the
/// status strip already peeled ([`stage_rect`]) — so the tier here equals the
/// painted tier by construction. Do not pass a raw paint area: `compute_layout`
/// would peel a second time and drop the tick tier a row (RC16 BUG-1).
pub fn sync_game_mode(agent: &mut AgentView, stage_width: u16, stage_height: u16) -> bool {
    if !agent.game_mode.open {
        return false;
    }
    let waiting_on_user =
        !agent.permission_queue.is_empty() || agent.question_view.is_some();
    let working = supervisor_is_working(agent);
    let tier = game_tier(ratatui::layout::Rect::new(0, 0, stage_width, stage_height));

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

    // ~12 Hz walk/blink — one anim step per TickDemand::Slow tick.
    let elapsed = agent.game_mode.last_tick.elapsed();
    if elapsed >= anim_tick_gate() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    /// BUG-1 regression: [`sync_game_mode`] takes the **stage**, so a 19-row
    /// paint area (stage 18 = `MIN_STAGE_H`, tier Normal) must not run the
    /// Compact snap-complete branch. Peeling the status strip a second time made
    /// the tick tier Compact and wiped every walk/celebrate at that height while
    /// the paint layer still drew full office art.
    #[test]
    fn tick_sync_keeps_walks_at_min_stage_height() {
        let paint_area = Rect::new(0, 0, 100, 19);
        assert_eq!(compute_layout(paint_area).tier, GameTier::Normal);
        let stage = stage_rect(paint_area);

        let mut agent =
            crate::app::agent_view::test_agent_view(None, std::path::PathBuf::from("."));
        agent.game_mode.open = true;
        agent.game_mode.desks[0].child_session_id = Some("child-1".to_string());
        agent.game_mode.desks[0].phase = ActorPhase::Celebrate;
        agent.game_mode.handoff_queue.push_back(0);

        sync_game_mode(&mut agent, stage.width, stage.height);

        assert_eq!(
            agent.game_mode.desks[0].phase,
            ActorPhase::Celebrate,
            "tick-path tier snap-cleared a walk the paint path renders"
        );
        assert_eq!(agent.game_mode.handoff_queue.len(), 1);
    }

    /// BUG-2 regression: the animation gate must never sit above
    /// [`SLOW_TICK_INTERVAL`], or every other Slow tick is dropped and the office
    /// (plus the tick-derived wall clock) runs at half rate.
    #[test]
    fn anim_gate_fires_every_slow_tick() {
        let gate = anim_tick_gate();
        assert!(
            gate <= SLOW_TICK_INTERVAL,
            "anim gate {gate:?} > SLOW_TICK_INTERVAL {SLOW_TICK_INTERVAL:?}"
        );
        // ...and not so far below it that a Fast tick would double the rate.
        assert!(
            gate * 2 > SLOW_TICK_INTERVAL,
            "anim gate {gate:?} too far below SLOW_TICK_INTERVAL {SLOW_TICK_INTERVAL:?}"
        );
    }
}
