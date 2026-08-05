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
    ActorPhase, DESK_COUNT, DeskAgentSnapshot, DeskSlot, GameModeState, HoverTarget,
    McpRackSnapshot, SupervisorPhase, SupervisorSnapshot,
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

/// Minimum wall time between full snapshot rebuilds.
///
/// PERF (RC16 PERF-3): the tick path had no throttle at all, so a pending
/// permission — which pushes `tick_demand` to Fast (~30 Hz) and is *exactly*
/// the state the wall advertises as "WAITING ON YOU" — tripled the rebuild
/// rate while nothing changed. Gate on the Slow cadence instead, independent
/// of what the app demands; `tick_anim` keeps its own [`anim_tick_gate`].
fn sync_gate() -> Duration {
    SLOW_TICK_INTERVAL.saturating_sub(ANIM_TICK_JITTER)
}

/// Build snapshots from live subagent map for Game Mode sync.
pub fn snapshots_from_subagents(
    sessions: &std::collections::HashMap<String, SubagentInfo>,
) -> Vec<DeskAgentSnapshot> {
    snapshots_filtered(sessions, |_| true)
}

/// [`snapshots_from_subagents`] restricted to the entries the room can act on.
///
/// PERF (RC16 PERF-2): `subagent_sessions` is insert-only — it grows for the
/// whole session — and every rebuild allocated ~4 Strings per entry plus a
/// sort. Completed subagents that no longer hold a desk cannot change anything
/// downstream ([`crate::views::game_mode::wall::compute_wall_mode`] only reads
/// `running`, seating only reads running agents), so they are dropped.
/// Running, seated (celebrate/handoff still in flight), overflow-queued and
/// failed entries are all kept — the failure path needs a finished snapshot to
/// arm the attention window and run the fail beat.
fn snapshots_filtered(
    sessions: &std::collections::HashMap<String, SubagentInfo>,
    keep: impl Fn(&SubagentInfo) -> bool,
) -> Vec<DeskAgentSnapshot> {
    let mut out: Vec<DeskAgentSnapshot> = sessions
        .values()
        .filter(|info| keep(info))
        .map(|info| {
            let running = info.is_running();
            let failed = info_failed(info);
            let tokens = info
                .tokens_used
                .or_else(|| info.usage.as_ref().map(|u| u.totals.total_tokens))
                .unwrap_or(0);
            // Progress can lag the terminal notification, so once the child is
            // finished the authoritative final count wins. Mirrors the Tasks
            // pane (`views::tasks_pane::format_subagent_metrics`); Game Mode
            // had the precedence inverted and left a stale live count on the
            // desk HUD of a finished agent (RC16 B10).
            let tool_calls = if info.finished {
                info.tool_calls.or(info.tool_call_count)
            } else {
                info.tool_call_count.or(info.tool_calls)
            }
            .unwrap_or(0);
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

/// Terminal status the room renders as a failure (fail beat + attention window).
///
/// Delegates to the dashboard's classifier so the two vocabularies cannot drift
/// apart: the inline `"failed" | "cancelled"` match here already missed
/// `"error"`, which would have celebrated a failed subagent and walked it to
/// the supervisor for a handoff (RC16 B9).
fn info_failed(info: &SubagentInfo) -> bool {
    matches!(
        crate::views::dashboard::classify_subagent(info),
        crate::views::dashboard::RowState::Failed
    )
}

/// Order-independent signature of everything a sync reads out of the subagent
/// map, plus the per-call sync inputs.
///
/// PERF (RC16 PERF-2): hashing the map in place is allocation-free, so an
/// unchanged map costs one O(entries) pass instead of a full snapshot rebuild
/// (~4 Strings per entry) plus a sort and O(desks x entries) scans.
///
/// `elapsed` is bucketed to whole seconds on purpose: it ticks continuously for
/// every running agent, and the desk HUD renders `mm:ss`. Sub-second drift must
/// not defeat the skip, but the whole-second edge must still refresh the timer.
fn subagent_signature(
    sessions: &std::collections::HashMap<String, SubagentInfo>,
    supervisor_working: bool,
    waiting_on_user: bool,
    tier: GameTier,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut acc = sessions.len() as u64;
    for info in sessions.values() {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        info.child_session_id.as_ref().hash(&mut h);
        info.finished.hash(&mut h);
        info.status.as_deref().hash(&mut h);
        info.description.as_ref().hash(&mut h);
        info.subagent_type.as_ref().hash(&mut h);
        info.activity_label.as_deref().hash(&mut h);
        info.tokens_used.hash(&mut h);
        info.usage
            .as_ref()
            .map(|u| u.totals.total_tokens)
            .hash(&mut h);
        info.tool_call_count.hash(&mut h);
        info.tool_calls.hash(&mut h);
        info.display_elapsed().as_secs().hash(&mut h);
        // Iteration order of a HashMap is not stable across inserts; fold
        // commutatively so a rehash alone never reads as a change.
        acc = acc.wrapping_add(h.finish());
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    acc.hash(&mut h);
    supervisor_working.hash(&mut h);
    waiting_on_user.hash(&mut h);
    (tier as u8).hash(&mut h);
    h.finish()
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
/// PERF: the snapshot rebuild is gated twice (RC16 PERF-2/PERF-3): at most once
/// per [`sync_gate`] regardless of the caller's cadence, and then only when
/// [`subagent_signature`] or the room's in-flight state actually changed.
/// `tick_anim` keeps its own [`anim_tick_gate`] and still runs on every call.
///
/// PERF: AppView keeps Game Mode on [`crate::app::app_view::TickDemand::Slow`]
/// (~12 Hz) **only while the room can animate** ([`GameModeState::needs_animation_tick`]).
/// A frozen office drops to [`crate::app::app_view::TickDemand::Ambient`]
/// (~0.33 Hz) for the coffee sip / steam / wall clock
/// ([`GameModeState::needs_ambient_tick`], RC16 §4 #7), and to `None` outright
/// once the pixel office is not on screen (RC16 PERF-1). Pixel recompose is
/// fingerprint-gated; redraw is dirty-gated so frozen idle rooms do not force a
/// full office paint every Slow tick.
///
/// `stage_width`/`stage_height` are the **stage** dims — the paint area with the
/// status strip already peeled ([`stage_rect`]) — so the tier here equals the
/// painted tier by construction. Do not pass a raw paint area: `compute_layout`
/// would peel a second time and drop the tick tier a row (RC16 BUG-1).
pub fn sync_game_mode(agent: &mut AgentView, stage_width: u16, stage_height: u16) -> bool {
    if !agent.game_mode.open {
        return false;
    }
    let tier = game_tier(ratatui::layout::Rect::new(0, 0, stage_width, stage_height));

    let sync_due = match agent.game_mode.last_sync_at {
        None => true,
        Some(t) => t.elapsed() >= sync_gate(),
    };
    if sync_due {
        let waiting_on_user = !agent.permission_queue.is_empty() || agent.question_view.is_some();
        let working = supervisor_is_working(agent);
        // Overlay data, refreshed on every due sync — deliberately *outside*
        // the rebuild skip below, which only proves the subagent map and the
        // room are unchanged and says nothing about the model, the turn timer
        // or the context window (see [`SupervisorSnapshot`]).
        refresh_supervisor_snapshot(agent, working, waiting_on_user);
        refresh_mcp_snapshot(agent);
        let sig = subagent_signature(&agent.subagent_sessions, working, waiting_on_user, tier);
        // Identical inputs + a room with nothing in flight ⇒ the sync is a
        // provable no-op; skip the rebuild entirely (RC16 PERF-2).
        let skippable =
            agent.game_mode.last_sync_sig == Some(sig) && agent.game_mode.room_is_settled();
        if !skippable {
            rebuild_from_subagents(agent, working, tier, waiting_on_user);
        }
        agent.game_mode.last_sync_sig = Some(sig);
        agent.game_mode.last_sync_at = Some(std::time::Instant::now());
    }

    // ~12 Hz walk/blink — one anim step per TickDemand::Slow tick.
    let elapsed = agent.game_mode.last_tick.elapsed();
    if elapsed >= anim_tick_gate() {
        agent.game_mode.tick_anim(tier);
    }

    agent.game_mode.take_redraw_dirty()
}

/// Refresh the Supervisor hover card's data from the live agent.
///
/// Overlay-only by construction: nothing here marks redraw dirty (that would
/// un-park a frozen room — RC16 PERF-1) and nothing here reaches
/// [`GameModeState::visual_fingerprint`]. The card is repainted by whatever
/// already repaints the office, which is guaranteed while any of this can move:
/// a running turn puts the Supervisor in `Working`, and that both animates the
/// office and holds `needs_animation_tick`.
///
/// Cost is one `String` clone for the model name per due sync (~12/s while the
/// room is awake, zero while parked); `branch` is only cloned when it changes.
fn refresh_supervisor_snapshot(agent: &mut AgentView, working: bool, waiting_on_user: bool) {
    let model = agent.session.models.current_model_name();
    // Only meaningful while a turn is actually running: `turn_started_at`
    // outlives the turn, and a finished turn's elapsed would sit frozen on the
    // card of a parked room.
    let turn_elapsed = if working { agent.turn_elapsed() } else { None };
    let (used, total, pct) = agent
        .context_state
        .as_ref()
        .map(|c| (c.used, c.total, c.usage_pct))
        .unwrap_or((0, 0, 0));
    let branch = (agent.game_mode.supervisor_info.branch.as_deref()
        != agent.current_branch.as_deref())
    .then(|| agent.current_branch.clone());
    let snap = &mut agent.game_mode.supervisor_info;
    snap.model = model;
    snap.turn_elapsed = turn_elapsed;
    snap.context_used = used;
    snap.context_total = total;
    snap.context_pct = pct;
    snap.waiting_on_user = waiting_on_user;
    if let Some(branch) = branch {
        snap.branch = branch;
    }
}

/// Whether Game Mode should ask the shell for this agent's MCP server list.
///
/// The Ctrl+G toggle dispatches no [`crate::app::actions::Effect`] at all, and
/// `mcp_status_cache` is otherwise only filled by opening `/mcps` — so an
/// office opened in a fresh session would show its rack tooltip the startup
/// counts forever. The caller ([`crate::app::app_view::AppView::tick`]) owns
/// the request because only it can reach `pending_effects`; it also re-checks
/// `agent_has_pending_mcps_fetch` so a modal fetch already in flight is not
/// duplicated, and flips `mcp_fetch_dispatched` only once it has actually
/// pushed the effect. One request per Game Mode open, never a per-tick storm.
pub fn wants_mcp_list_fetch(agent: &AgentView) -> bool {
    agent.game_mode.open
        && !agent.game_mode.mcp_fetch_dispatched
        && agent.mcp_status_cache.is_none()
}

/// Mark the one-shot `mcp/list` request as sent for this Game Mode open.
pub fn mark_mcp_list_fetch_dispatched(agent: &mut AgentView) {
    agent.game_mode.mcp_fetch_dispatched = true;
}

/// Refresh the MCP rack hover card's data from the live agent.
///
/// Overlay-only, exactly like [`refresh_supervisor_snapshot`]: nothing here
/// marks redraw dirty and nothing here reaches
/// [`GameModeState::visual_fingerprint`] (the composed LEDs read
/// `rack_active_until`, which is room state, not this snapshot).
///
/// Cost is a generation compare per due sync. The rows are only cloned when
/// `AgentView::mcp_status_gen` moves — i.e. on an `mcp/list` response or a
/// `server_status` push, both rare — so the steady state is one `u64` compare
/// plus two `u32` copies, not a `Vec<String>` clone at ~12 Hz.
fn refresh_mcp_snapshot(agent: &mut AgentView) {
    let rows_gen = agent.mcp_status_gen;
    let rows = (agent.game_mode.mcp_info.rows_gen != rows_gen)
        .then(|| agent.mcp_status_cache.clone().unwrap_or_default());
    let (connected, total, active) = agent
        .mcp_init_progress
        .as_ref()
        .map(|p| (p.connected, p.total, p.is_visible()))
        .unwrap_or((0, 0, false));
    let snap = &mut agent.game_mode.mcp_info;
    if let Some(rows) = rows {
        snap.servers = rows;
        snap.rows_gen = rows_gen;
    }
    snap.init_connected = connected;
    snap.init_total = total;
    snap.init_active = active;
}

/// Rebuild snapshots, reseat the room, and mark redraw when the office changed.
fn rebuild_from_subagents(
    agent: &mut AgentView,
    working: bool,
    tier: GameTier,
    waiting_on_user: bool,
) {
    let wall_before = agent.game_mode.wall;
    let seats_before = agent.game_mode.active_desk_count();
    // Hashed once per sync instead of twice: the post-sync value is cached and
    // reused as the "before" of the next sync. `tick_anim` drops the cache
    // whenever it may have moved a phase, so it is never stale (RC16 PERF-2).
    let phase_sig_before = agent
        .game_mode
        .last_phase_sig
        .unwrap_or_else(|| phase_signature(&agent.game_mode));

    let room = &agent.game_mode;
    let snaps = snapshots_filtered(&agent.subagent_sessions, |info| {
        info.is_running()
            || info_failed(info)
            || room
                .desks
                .iter()
                .any(|d| d.child_session_id.as_deref() == Some(info.child_session_id.as_ref()))
            || room
                .door_queue
                .iter()
                .any(|id| id.as_str() == info.child_session_id.as_ref())
    });
    agent
        .game_mode
        .sync_from_snapshots(&snaps, working, tier, waiting_on_user);
    agent.game_mode.sync_rebuilds = agent.game_mode.sync_rebuilds.wrapping_add(1);

    let phase_sig_after = phase_signature(&agent.game_mode);
    if agent.game_mode.wall != wall_before
        || agent.game_mode.active_desk_count() != seats_before
        || phase_sig_after != phase_sig_before
    {
        agent.game_mode.mark_redraw_dirty();
    }
    agent.game_mode.last_phase_sig = Some(phase_sig_after);
}

/// Signature of everything a sync *derives* that the painter reads outside the
/// desks themselves — the sync's dirty gate.
///
/// `overflow_count` is included because the +N door badge and the status strip
/// render it while the seats, phases and wall all stay put: a 7th subagent
/// arriving at a full room changed nothing else, so the badge went stale
/// (RC16 B11).
fn phase_signature(state: &GameModeState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for d in &state.desks {
        d.child_session_id.hash(&mut h);
        (d.phase as u8).hash(&mut h);
        d.failed.hash(&mut h);
    }
    (state.supervisor as u8).hash(&mut h);
    state.overflow_count.hash(&mut h);
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

    /// Office with one seated, already-settled desk (spawn walk finished).
    fn office_with_one_working_desk() -> AgentView {
        let mut agent =
            crate::app::agent_view::test_agent_view(None, std::path::PathBuf::from("."));
        agent.game_mode.open = true;
        agent.subagent_sessions.insert(
            "child-1".into(),
            crate::app::agent_view::test_fixtures::running_subagent_info("child-1"),
        );
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(agent.game_mode.active_desk_count(), 1, "agent must seat");
        // Skip the spawn walk so the room is settled (`tick_anim` would too).
        agent.game_mode.desks[0].phase = ActorPhase::AtDeskWorking;
        agent
    }

    /// Pretend a full [`sync_gate`] window has passed so the throttle is open.
    fn open_sync_gate(agent: &mut AgentView) {
        agent.game_mode.last_sync_at = agent
            .game_mode
            .last_sync_at
            .map(|t| t - sync_gate() - Duration::from_millis(1));
    }

    /// Hold the animation gate shut across the next `sync_game_mode`.
    ///
    /// Tests that assert "an unchanged room must not repaint" are otherwise
    /// racing real wall time: if the anim gate happens to open between two
    /// statements, `tick_anim` advances a sprite bucket and marks the room
    /// dirty for a reason the test never intended to exercise.
    fn freeze_anim_gate(agent: &mut AgentView) {
        agent.game_mode.last_tick = std::time::Instant::now();
    }

    /// PERF-2: an unchanged (insert-only, ever-growing) subagent map must not
    /// pay for a snapshot rebuild + sort + reseat on every tick.
    #[test]
    fn unchanged_subagent_map_skips_the_rebuild() {
        let mut agent = office_with_one_working_desk();
        assert_eq!(agent.game_mode.sync_rebuilds, 1, "first sync rebuilds");

        for _ in 0..5 {
            open_sync_gate(&mut agent);
            sync_game_mode(&mut agent, 100, 20);
        }

        assert_eq!(
            agent.game_mode.sync_rebuilds, 1,
            "identical map + settled room must skip the rebuild"
        );
        assert_eq!(agent.game_mode.desks[0].phase, ActorPhase::AtDeskWorking);
        assert_eq!(agent.game_mode.active_desk_count(), 1);
    }

    /// The Supervisor hover card's data is refreshed *outside* the PERF-2
    /// rebuild skip (a settled room says nothing about the model or the branch)
    /// and is overlay-only: refreshing it must never mark the room dirty, or a
    /// frozen office could never park (PERF-1).
    #[test]
    fn supervisor_snapshot_refreshes_without_dirtying_the_room() {
        let mut agent =
            crate::app::agent_view::test_agent_view(None, std::path::PathBuf::from("."));
        agent.game_mode.open = true;
        agent.current_branch = Some("rc16-game-mode".to_string());
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(agent.game_mode.sync_rebuilds, 1, "first sync rebuilds");
        assert_eq!(
            agent.game_mode.supervisor_info.branch.as_deref(),
            Some("rc16-game-mode")
        );
        assert_eq!(
            agent.game_mode.supervisor_info.turn_elapsed, None,
            "an idle supervisor must not show a running turn timer"
        );

        agent.game_mode.take_redraw_dirty();
        agent.current_branch = Some("dev".to_string());
        open_sync_gate(&mut agent);
        freeze_anim_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);

        assert_eq!(
            agent.game_mode.sync_rebuilds, 1,
            "empty settled room must still skip the rebuild"
        );
        assert_eq!(
            agent.game_mode.supervisor_info.branch.as_deref(),
            Some("dev"),
            "the card must not go stale behind a skipped rebuild"
        );
        assert!(
            !agent.game_mode.take_redraw_dirty(),
            "overlay-only tooltip data must not wake the room"
        );
    }

    /// PERF-2: the skip is signature-driven, so a real change is still picked
    /// up on the very next sync — including the finish that starts a celebrate.
    #[test]
    fn changed_subagent_map_is_picked_up_next_sync() {
        let mut agent = office_with_one_working_desk();
        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(agent.game_mode.sync_rebuilds, 1, "no change, no rebuild");

        let info = agent.subagent_sessions.get_mut("child-1").unwrap();
        info.finished = true;
        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);

        assert_eq!(
            agent.game_mode.sync_rebuilds, 2,
            "a map change must rebuild"
        );
        assert_eq!(
            agent.game_mode.desks[0].phase,
            ActorPhase::Celebrate,
            "finished subagent must still hand off"
        );
    }

    /// PERF-3: permission prompts / question views push the app to Fast
    /// (~30 Hz) — precisely the "WAITING ON YOU" state — so the sync must be
    /// throttled to the Slow cadence by itself, whatever the caller's rate.
    #[test]
    fn sync_is_throttled_to_slow_cadence_at_fast_tick_rate() {
        let mut agent = office_with_one_working_desk();
        // Change the map every call: only the throttle can hold the rebuild.
        for i in 0..10u32 {
            let info = agent.subagent_sessions.get_mut("child-1").unwrap();
            info.tool_call_count = Some(i);
            // Pin the window open-end to *now* so a slow machine cannot let the
            // gate elapse mid-loop and turn this into a flake.
            agent.game_mode.last_sync_at = Some(std::time::Instant::now());
            sync_game_mode(&mut agent, 100, 20);
        }
        assert_eq!(
            agent.game_mode.sync_rebuilds, 1,
            "back-to-back Fast ticks must not re-sync inside one gate window"
        );

        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(
            agent.game_mode.sync_rebuilds, 2,
            "the sync must resume once the gate window has passed"
        );
    }

    /// PERF-2 skip must not swallow the room's own state machine: `tick_anim`
    /// clears the desk when an exit walk ends, and only a sync re-derives the
    /// wall + supervisor from the emptied room — with the subagent map itself
    /// completely unchanged across both ticks.
    #[test]
    fn walk_completing_in_tick_anim_forces_the_next_sync() {
        use std::time::Instant;

        let mut agent = office_with_one_working_desk();
        {
            let info = agent.subagent_sessions.get_mut("child-1").unwrap();
            info.finished = true;
            // Frozen elapsed: only the room, never the map, changes below.
            info.duration_ms = Some(1234);
        }
        {
            let gm = &mut agent.game_mode;
            gm.desks[0].phase = ActorPhase::ExitDoor;
            gm.desks[0].finish_started = true;
            gm.desks[0].phase_started = Instant::now() - Duration::from_secs(1);
            gm.last_tick = Instant::now() - anim_tick_gate() - Duration::from_millis(1);
        }

        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(
            agent.game_mode.active_desk_count(),
            0,
            "the exit walk must have retired the desk"
        );
        assert_eq!(agent.game_mode.sync_rebuilds, 2);
        assert_eq!(
            agent.game_mode.supervisor,
            SupervisorPhase::Waiting,
            "supervisor was derived while the desk was still walking out"
        );

        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(
            agent.game_mode.sync_rebuilds, 3,
            "clearing a desk in tick_anim must invalidate the skip"
        );
        assert_eq!(
            agent.game_mode.supervisor,
            SupervisorPhase::Idle,
            "the emptied room must be re-derived"
        );

        // ...and the now-settled empty room goes back to skipping.
        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(agent.game_mode.sync_rebuilds, 3);
    }

    /// PERF-3 latency bound: the throttle may never delay a freshly spawned
    /// subagent by more than one Slow tick — and a never-synced room (toggle)
    /// always seats immediately.
    #[test]
    fn new_subagent_seats_within_one_slow_tick() {
        assert!(
            sync_gate() <= SLOW_TICK_INTERVAL,
            "sync gate {:?} must not exceed SLOW_TICK_INTERVAL {SLOW_TICK_INTERVAL:?}",
            sync_gate()
        );

        let mut agent = office_with_one_working_desk();
        agent.subagent_sessions.insert(
            "child-2".into(),
            crate::app::agent_view::test_fixtures::running_subagent_info("child-2"),
        );
        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);

        assert_eq!(
            agent.game_mode.active_desk_count(),
            2,
            "a new running subagent must seat on the next gated sync"
        );
    }

    /// B9: the room's failure vocabulary must be the dashboard's. `"error"` is
    /// a failure there, so it must run the fail beat here — never a celebrate
    /// + handoff walk.
    #[test]
    fn error_status_runs_the_fail_beat() {
        for status in ["failed", "cancelled", "error"] {
            let mut agent = office_with_one_working_desk();
            {
                let info = agent.subagent_sessions.get_mut("child-1").unwrap();
                info.finished = true;
                info.status = Some(status.into());
            }
            open_sync_gate(&mut agent);
            sync_game_mode(&mut agent, 100, 20);

            assert_eq!(
                agent.game_mode.desks[0].phase,
                ActorPhase::FailBeat,
                "{status:?} must read as a failure"
            );
        }

        // ...and a completed one still celebrates.
        let mut agent = office_with_one_working_desk();
        {
            let info = agent.subagent_sessions.get_mut("child-1").unwrap();
            info.finished = true;
            info.status = Some("completed".into());
        }
        open_sync_gate(&mut agent);
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(agent.game_mode.desks[0].phase, ActorPhase::Celebrate);
    }

    /// B10: a finished subagent's desk must show the authoritative final tool
    /// count, not the live progress count that can lag behind it.
    #[test]
    fn finished_desk_prefers_the_final_tool_call_count() {
        let mut sessions = std::collections::HashMap::new();
        let mut info = crate::app::agent_view::test_fixtures::running_subagent_info("child-1");
        info.tool_call_count = Some(3); // live progress, lagging
        info.tool_calls = Some(7); // terminal notification
        sessions.insert("child-1".to_string(), info);

        let running = snapshots_from_subagents(&sessions);
        assert_eq!(running[0].tool_calls, 3, "live count wins while running");

        sessions.get_mut("child-1").unwrap().finished = true;
        let finished = snapshots_from_subagents(&sessions);
        assert_eq!(finished[0].tool_calls, 7, "final count wins once finished");
    }

    /// B11: a 7th subagent arriving at a full room only changes `+N` — same
    /// seats, same phases, same wall — so the sync must still mark redraw
    /// dirty or the door badge and status strip sit stale.
    #[test]
    fn overflow_change_marks_redraw_dirty() {
        let mut agent =
            crate::app::agent_view::test_agent_view(None, std::path::PathBuf::from("."));
        agent.game_mode.open = true;
        for i in 0..DESK_COUNT {
            let sid = format!("child-{i}");
            agent.subagent_sessions.insert(
                sid.clone(),
                crate::app::agent_view::test_fixtures::running_subagent_info(&sid),
            );
        }
        sync_game_mode(&mut agent, 100, 20);
        assert_eq!(agent.game_mode.active_desk_count(), DESK_COUNT);
        // The seated desks are left in their spawn walk and no tick advances
        // them, so the room re-syncs (never skips) with a *stable* phase
        // signature — the control below proves nothing else moves, which is
        // what makes the final assertion about `+N` and nothing else.
        open_sync_gate(&mut agent);
        freeze_anim_gate(&mut agent);
        assert!(
            !sync_game_mode(&mut agent, 100, 20),
            "control: an unchanged room must not repaint"
        );

        agent.subagent_sessions.insert(
            "child-overflow".into(),
            crate::app::agent_view::test_fixtures::running_subagent_info("child-overflow"),
        );
        open_sync_gate(&mut agent);
        freeze_anim_gate(&mut agent);
        let dirty = sync_game_mode(&mut agent, 100, 20);

        assert_eq!(agent.game_mode.overflow_count, 1, "must queue at the door");
        assert_eq!(
            agent.game_mode.active_desk_count(),
            DESK_COUNT,
            "seats are unchanged — only +N moved"
        );
        assert!(dirty, "the +N door badge / status strip must repaint");
    }
}
