//! Game Mode runtime state: desk slots, handoff queue, wall flags.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use image::RgbaImage;

use super::layout::GameTier;
use super::wall::WallMode;

pub const DESK_COUNT: usize = 6;

/// Ticks between forced HUD repaints in tiers that paint per-desk monitor text.
///
/// ~1 s at the ~12 Hz Slow tick. The HUD shows whole-second data (`mm:ss`,
/// tokens, tool calls), and the sync signature buckets `elapsed` to whole
/// seconds too (RC16 PERF-2), so a finer cadence would repaint identical text.
const HUD_REFRESH_TICKS: u64 = 12;

/// How long one MCP rack LED burst stays lit after a tool call (RC16 §4 #5).
///
/// Long enough for the chase to read as motion — the LEDs step on the
/// `(tick / 4)` bucket, ~333 ms at the Slow cadence, so 1.2 s is ~4 steps — and
/// short enough that the worst-case wakeup tail it can add to an otherwise
/// frozen room stays inside ~14 Slow ticks (see
/// [`GameModeState::rack_burst_active`]).
const RACK_BURST: Duration = Duration::from_millis(1200);

/// Minimum wall-clock gap between ambient animation steps (RC16 §4 #7 / #12).
///
/// The ambient step is what drives the two animations that only exist in a room
/// nothing else is animating: the thinking developer's coffee sip (with the
/// thinking bubble that rides the same sprite key) and the idle Supervisor's
/// coffee steam. It also re-reads the wall clock for [`GameModeState::clock_hm`].
///
/// Deliberately **shorter** than [`crate::app::app_view::AMBIENT_TICK_INTERVAL`],
/// the cadence the event loop wakes a parked office at. If the two were equal,
/// scheduler jitter would leave `elapsed` a hair under the gate on most wakes
/// and the animation would run at half the intended rate — exactly the 90 ms
/// gate vs 83 ms tick failure RC16 BUG-2 documented.
const AMBIENT_PERIOD: Duration = Duration::from_millis(2500);

/// Quantization of the success wave's sweep position (RC16 §4 #8).
///
/// The composed crest is derived from this bucket and nothing finer (see
/// [`GameModeState::success_wave_t`]), so the whole one-shot costs exactly
/// [`SUCCESS_WAVE_BUCKETS`] recomposes — a wave position the fingerprint cannot
/// distinguish cannot exist.
const SUCCESS_WAVE_BUCKET_MS: u64 = 150;

/// Number of [`SUCCESS_WAVE_BUCKET_MS`] steps the crest takes to cross the room.
const SUCCESS_WAVE_BUCKETS: u64 = 10;

/// How long one office-wide success wave runs after WORK FINISHED (RC16 §4 #8).
///
/// ~1.5 s: long enough to read as a sweep at the ~12 Hz Slow tick, short enough
/// that the wakeup tail it holds on a room that is otherwise parking stays
/// inside ~18 Slow ticks, once per success event (see
/// [`GameModeState::success_wave_t`]).
const SUCCESS_WAVE: Duration =
    Duration::from_millis(SUCCESS_WAVE_BUCKET_MS * SUCCESS_WAVE_BUCKETS);

/// Minimum wall time between two token-throughput samples on one desk (§4 #9).
///
/// Wall time, **not** sync count: [`super::sync_game_mode`] is throttled
/// (RC16 PERF-3) and skips entirely for unchanged subagent maps (PERF-2), so a
/// delta measured "per sync" would read a different rate depending on how busy
/// the rest of the app was. A running desk re-signs at least once a second
/// (`display_elapsed().as_secs()` is in `subagent_signature`), so a desk that
/// stops streaming still gets sampled and still decays back to Calm.
const BUSY_SAMPLE_PERIOD: Duration = Duration::from_millis(750);

/// Tokens/sec bucket boundaries for [`BusyLevel`] (RC16 §4 #9).
///
/// Two-sided on purpose — the enter thresholds sit well above the exit ones, so
/// a stream hovering at a boundary cannot flip a desk's typing cadence on every
/// sample. Without that, a desk at ~45 tok/s would visibly stutter between two
/// animation rates, which reads as a bug rather than as throughput.
const BUSY_HOT_ENTER: f32 = 45.0;
const BUSY_HOT_EXIT: f32 = 28.0;
const BUSY_NORMAL_ENTER: f32 = 12.0;
const BUSY_NORMAL_EXIT: f32 = 5.0;

/// Snapshot of one subagent for the room (decoupled from pager types).
#[derive(Debug, Clone)]
pub struct DeskAgentSnapshot {
    pub child_session_id: String,
    pub label: String,
    pub subagent_type: String,
    pub running: bool,
    pub failed: bool,
    pub elapsed: Duration,
    pub tokens: u64,
    pub tool_calls: u32,
    pub activity: String,
}

/// Visual / animation phase for a seated (or walking) agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorPhase {
    /// Sitting at desk, working.
    AtDeskWorking,
    /// Sitting, thinking.
    AtDeskThinking,
    /// Walking from door to desk (spawn).
    SpawnWalk,
    /// Celebrate at desk after success.
    Celebrate,
    /// Walking to supervisor.
    WalkToBoss,
    /// Handoff at rug.
    Handoff,
    /// Walking out the door after handoff.
    ExitDoor,
    /// Fail beat at desk.
    FailBeat,
}

/// How hard a seated desk is streaming, from its token throughput (RC16 §4 #9).
///
/// Drives the desk's typing cadence in
/// [`super::compose::desk_typing_frame`] and nothing else — a desk that is not
/// [`ActorPhase::AtDeskWorking`] composes no keyboard, which is why
/// [`GameModeState::visual_fingerprint`] hashes the level only for that phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyLevel {
    /// Barely streaming — long thinking pauses, waiting on a tool.
    Calm,
    /// Ordinary streaming; the cadence every desk had before RC16 §4 #9.
    Normal,
    /// Flat out.
    Hot,
}

impl BusyLevel {
    /// Tick divisor for this desk's typing frame.
    ///
    /// FINGERPRINT INVARIANT: the divisors are **powers of two**, and
    /// [`GameModeState::frame_bucket_divisor`] hashes `tick / d` for the finest
    /// one any desk is using. With 2/4/8 every coarser bucket's edges are a
    /// strict subset of the finer one's, so the single hashed value determines
    /// every desk's frame. A divisor like 6 would put a frame change at tick 6,
    /// where `tick / 4` does not move, and the fingerprint would miss it.
    pub(crate) fn frame_divisor(self) -> u64 {
        match self {
            Self::Calm => 8,
            Self::Normal => 4,
            Self::Hot => 2,
        }
    }
}

/// Next [`BusyLevel`] for a desk currently at `cur` measuring `rate` tokens/sec.
///
/// Hysteresis: entering a level needs a clearly higher rate than staying in it
/// (see the `BUSY_*` constants), so a noisy delta cannot oscillate the cadence.
fn next_busy_level(cur: BusyLevel, rate: f32) -> BusyLevel {
    let hot_gate = if matches!(cur, BusyLevel::Hot) {
        BUSY_HOT_EXIT
    } else {
        BUSY_HOT_ENTER
    };
    let normal_gate = if matches!(cur, BusyLevel::Calm) {
        BUSY_NORMAL_ENTER
    } else {
        BUSY_NORMAL_EXIT
    };
    if rate >= hot_gate {
        BusyLevel::Hot
    } else if rate >= normal_gate {
        BusyLevel::Normal
    } else {
        BusyLevel::Calm
    }
}

#[derive(Debug, Clone)]
pub struct DeskSlot {
    pub child_session_id: Option<String>,
    pub label: String,
    pub subagent_type: String,
    pub phase: ActorPhase,
    pub elapsed: Duration,
    pub tokens: u64,
    pub tool_calls: u32,
    pub activity: String,
    pub failed: bool,
    /// Typing cadence bucket, re-derived from token throughput at sync time.
    pub(crate) busy: BusyLevel,
    /// `tokens` as of the last throughput sample (**not** the last sync).
    pub(crate) prev_tokens: u64,
    /// When [`Self::prev_tokens`] was taken — the denominator of the rate.
    pub(crate) tokens_at: Instant,
    /// Palette index 0..5 for sprite color.
    pub skin: u8,
    /// Animation progress 0.0..1.0 for current phase.
    pub anim_t: f32,
    /// Phase started at.
    pub phase_started: Instant,
    /// One-shot: success finish / handoff sequence already started for this seating.
    /// Prevents double celebrate→handoff when snapshots re-fire "not running".
    pub finish_started: bool,
}

impl Default for DeskSlot {
    fn default() -> Self {
        Self {
            child_session_id: None,
            label: String::new(),
            subagent_type: String::new(),
            phase: ActorPhase::AtDeskWorking,
            elapsed: Duration::ZERO,
            tokens: 0,
            tool_calls: 0,
            activity: String::new(),
            failed: false,
            busy: BusyLevel::Normal,
            prev_tokens: 0,
            tokens_at: Instant::now(),
            skin: 0,
            anim_t: 0.0,
            phase_started: Instant::now(),
            finish_started: false,
        }
    }
}

impl DeskSlot {
    pub fn is_empty(&self) -> bool {
        self.child_session_id.is_none()
    }

    pub fn is_occupied(&self) -> bool {
        self.child_session_id.is_some()
    }

    /// Fold this sync's token count into the desk's throughput bucket (§4 #9).
    ///
    /// A no-op until [`BUSY_SAMPLE_PERIOD`] of **wall time** has passed since
    /// the last sample, so the measured rate is independent of how often the
    /// sync actually ran. Called before `tokens` is overwritten, but it reads
    /// [`Self::prev_tokens`], not `tokens` — the window spans several syncs.
    fn sample_throughput(&mut self, tokens: u64) {
        let dt = self.tokens_at.elapsed();
        if dt < BUSY_SAMPLE_PERIOD {
            return;
        }
        let rate = tokens.saturating_sub(self.prev_tokens) as f32 / dt.as_secs_f32();
        self.busy = next_busy_level(self.busy, rate);
        self.prev_tokens = tokens;
        self.tokens_at = Instant::now();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorPhase {
    Idle,
    Working,
    Reviewing,
    Waiting,
}

/// What the pointer (or Tab focus) is currently over.
///
/// Hit-tested against the rects captured at paint time (`last_desks`,
/// `last_supervisor`, `last_mcp_rack`). Purely an *overlay* concern: the tooltip
/// and focus ring are painted after the halfblock blit, so no variant may ever
/// reach [`GameModeState::visual_fingerprint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverTarget {
    /// Seated subagent desk, by index into `desks`.
    Desk(usize),
    /// The Supervisor (main agent) at the rug.
    Supervisor,
    /// The MCP server rack on the right wall. Pixel office only — the Unicode
    /// and Compact tiers compose no rack, and `last_mcp_rack` stays a zero-size
    /// `Rect` there, which never hit-tests.
    McpRack,
}

/// Live Supervisor facts behind the Supervisor hover card — **overlay data**.
///
/// Filled in [`super::sync_game_mode`], which is the one Game Mode entry point
/// holding `&mut AgentView`. Never hashed into
/// [`GameModeState::visual_fingerprint`] and never marks redraw dirty: the card
/// is a ratatui overlay painted after the blit, and dirtying on a ticking timer
/// would un-park the event loop that RC16 PERF-1 exists to park.
///
/// **Bounded staleness.** The sync is throttled (`sync_gate`, ~75 ms) and can
/// skip its *snapshot rebuild* entirely — the refresh here deliberately sits
/// outside that skip, so the worst case is one sync gate, and the paint path
/// re-syncs anything older than 40 ms ([`GameModeState::needs_paint_sync`]).
/// `turn_elapsed` is `Some` only while the Supervisor is actually working, and
/// a working Supervisor keeps [`GameModeState::needs_animation_tick`] true, so
/// a running timer can never be frozen by loop parking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupervisorSnapshot {
    /// Display name of the active model (`models.current_model_name()`).
    pub model: Option<String>,
    /// Pause-corrected elapsed time of the running turn; `None` when idle.
    pub turn_elapsed: Option<Duration>,
    /// Context window used / total tokens, and the resolved usage percent.
    pub context_used: u64,
    pub context_total: u64,
    pub context_pct: u8,
    /// A permission prompt or question is blocking on the human.
    pub waiting_on_user: bool,
    /// Current git branch of this agent's cwd, when known.
    pub branch: Option<String>,
}

/// Live MCP fleet facts behind the rack hover card — **overlay data**.
///
/// Mirrors `AgentView::mcp_status_cache` into the room because the painter only
/// ever sees a `GameModeState`. Same contract as [`SupervisorSnapshot`]: never
/// hashed into [`GameModeState::visual_fingerprint`], never marks redraw dirty.
///
/// The rows are re-cloned only when `AgentView::mcp_status_gen` moves, so a
/// steady ~12 Hz sync costs one `u64` compare — see
/// [`super::refresh_mcp_snapshot`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpRackSnapshot {
    /// Distilled per-server rows; empty until the first `mcp/list` lands.
    pub servers: Vec<crate::views::mcps_modal::McpStatusRow>,
    /// `x.ai/mcp/init_progress` counts — the startup fallback the card shows
    /// while `servers` is still empty.
    pub init_connected: u32,
    pub init_total: u32,
    /// Whether the shell is still connecting servers (`mcp_init_progress` is
    /// live and visible).
    pub init_active: bool,
    /// `AgentView::mcp_status_gen` that `servers` was cloned at.
    pub(crate) rows_gen: u64,
}

/// Full Game Mode UI state owned by `AgentView`.
#[derive(Debug)]
pub struct GameModeState {
    pub open: bool,
    pub desks: [DeskSlot; DESK_COUNT],
    /// child_session_id → desk index
    pub seat_map: HashMap<String, usize>,
    /// Waiting for a free desk (overflow).
    pub door_queue: VecDeque<String>,
    /// FIFO of desks waiting to hand off (indices).
    pub handoff_queue: VecDeque<usize>,
    pub supervisor: SupervisorPhase,
    pub wall: WallMode,
    /// At least one successful completion while mode was open this session.
    pub had_success: bool,
    pub tick: u64,
    pub last_tick: Instant,
    /// Overflow count for status strip.
    pub overflow_count: usize,
    /// Sticky NEEDS ATTENTION until this instant (spec: brief, not forever).
    pub attention_until: Option<Instant>,
    /// MCP rack LED burst armed until this instant (RC16 §4 #5).
    ///
    /// Armed by [`Self::sync_from_snapshots`] whenever a seated desk's
    /// `tool_calls` counter *increases*, i.e. by real work, and consumed on the
    /// expiry edge in [`Self::tick_anim`]. See [`Self::rack_burst_active`] for
    /// the fingerprint and wakeup contract.
    pub(crate) rack_active_until: Option<Instant>,
    /// Office-wide success wave armed until this instant (RC16 §4 #8).
    ///
    /// Armed by [`Self::sync_from_snapshots`] on the *edge* into
    /// [`WallMode::WorkFinished`] — never on the level, because `had_success`
    /// is sticky and the wall then sits on WorkFinished for the rest of the
    /// session. Consumed on the expiry edge in [`Self::tick_anim`]. See
    /// [`Self::success_wave_t`] for the fingerprint and wakeup contract.
    pub(crate) success_fx_until: Option<Instant>,
    /// Slow ambient animation step (RC16 §4 #7).
    ///
    /// Wall-clock driven, **not** derived from `tick`: an office the event loop
    /// has parked only wakes at [`crate::app::app_view::AMBIENT_TICK_INTERVAL`],
    /// so `tick` no longer advances at a fixed rate there and a `tick / N`
    /// bucket would drift with the room's own business. Advanced by
    /// [`Self::tick_anim`] once per [`AMBIENT_PERIOD`]; read by
    /// [`Self::ambient_frame`].
    pub(crate) ambient_step: u64,
    /// When [`Self::ambient_step`] last advanced.
    pub(crate) ambient_at: Instant,
    /// Patrol step of the floor robot (RC16 §4 #11).
    ///
    /// ZERO-COST CONTRACT — the whole point of this animation, do not relax it:
    /// advanced by [`Self::tick_anim`] **only on a `tick / 4` bucket edge and
    /// only while [`Self::pixel_needs_tick_frame`] is already true**, i.e. only
    /// while the room is animating (and therefore repainting) for its own
    /// reasons. That gives three properties at once:
    /// 1. **No new wakeups.** [`Self::needs_animation_tick`] is untouched; a
    ///    frozen room still parks at `TickDemand::None` and the robot parks with
    ///    it, wherever it had got to.
    /// 2. **No new recomposes.** Every tick this advances on already marks
    ///    redraw dirty via the `frame_edge` gate below (the bucket divisor is 4
    ///    or finer, so a `tick / 4` edge is always also a frame edge).
    /// 3. **Quantized by construction.** The composed position is
    ///    [`super::compose::roomba_position`] of this counter and nothing finer,
    ///    so a position [`Self::visual_fingerprint`] cannot distinguish cannot
    ///    exist.
    ///
    /// Making it wander while the room is idle would trip both the RC13
    /// idle-freeze (Rule 2) and the RC16 PERF-1 parking (Rule 3) at once.
    pub(crate) roomba_step: u64,
    /// Local wall clock, quantized to `(hour 0..24, ten-minute 0..6)`.
    ///
    /// The quantization *is* the fingerprint contract (RC16 §4 #12): the composed
    /// hands are derived from this pair and nothing finer, so at most 6 clock
    /// recomposes per hour are possible. Re-read on the ambient step, so it is at
    /// most [`AMBIENT_PERIOD`] stale.
    pub(crate) clock_hm: (u8, u8),
    /// Skin assignment counter.
    next_skin: u8,
    /// Full-res mockup (decoded once).
    pub(crate) pixel_bg_full: Option<RgbaImage>,
    /// Background scaled to last paint size (`cell_w × cell_h*2` × pixel_scale).
    ///
    /// PERF: only rebuild when `pixel_cell_w/h` or `pixel_bg_scale` change — never
    /// on tick, hover, status strip, or sprite phase alone.
    pub(crate) pixel_bg_scaled: Option<RgbaImage>,
    pub(crate) pixel_cell_w: u16,
    pub(crate) pixel_cell_h: u16,
    /// `pixel_scale()` captured when `pixel_bg_scaled` was last built.
    pub(crate) pixel_bg_scale: u32,
    /// Hour tint band baked into `pixel_bg_scaled` (RC16 §4 #12).
    ///
    /// PERF: the day/night tint is applied **once, to the cached background**,
    /// not per compose — a full-canvas blend on every fingerprint miss would
    /// have cost an extra O(canvas) pass during every walk. Sprites are blitted
    /// after, so they stay lit by their own monitors while the room's daylight
    /// changes, and the floor stamps sample the tinted BG so they match.
    /// A band change rebuilds the scaled BG exactly like a resize does.
    pub(crate) pixel_bg_tint: u8,
    /// High-res composited frame (`cell * PIXEL_SCALE`).
    pub(crate) pixel_frame: Option<RgbaImage>,
    /// Terminal halfblock source (`cell_w × cell_h*2`) — downsampled once per
    /// fingerprint so every paint can `use_direct` (RC13 dual-audit P0).
    pub(crate) pixel_paint: Option<RgbaImage>,
    /// Precomputed halfblock cell colors for the current fingerprint (skip
    /// per-paint image sampling — triple-scan P1).
    pub(crate) pixel_halfblock: Option<xai_grok_pager_render::render::image_overlay::HalfblockCellCache>,
    /// Reused high-res compose canvas (avoids full-frame alloc on every miss).
    pub(crate) pixel_compose_scratch: Option<RgbaImage>,
    /// Visual fingerprint for the cached composited frame (see [`Self::visual_fingerprint`]).
    pub(crate) pixel_frame_fp: u64,
    /// Prefer pixel office (mockup + sprites). False falls back to Unicode.
    pub pixel_mode: bool,
    /// Target under the mouse cursor (popup placement), if any.
    pub hover: Option<HoverTarget>,
    /// Keyboard focus desk (Tab cycle); independent of mouse hover (dual-audit).
    ///
    /// Desks only — Tab deliberately does **not** reach the Supervisor: it
    /// cycles the *seats*, and everything the Supervisor card shows (model,
    /// turn timer, context) is already reachable from the status bar without a
    /// pointer.
    pub keyboard_focus: Option<usize>,
    /// Last mouse position in screen coords (for popup placement).
    pub hover_screen: Option<(u16, u16)>,
    /// Last painted stage area (for hover hit-testing).
    pub last_stage: Option<ratatui::layout::Rect>,
    /// Last desk rects from layout (for hover hit-testing).
    pub last_desks: [ratatui::layout::Rect; 6],
    /// Last painted supervisor rect (for hover hit-testing) — derived from the
    /// compose anchors in the pixel office, from `layout.supervisor` in unicode.
    pub last_supervisor: ratatui::layout::Rect,
    /// Last painted MCP rack rect (for hover hit-testing). Zero-size whenever
    /// the pixel office did not paint — the rack exists only there.
    pub last_mcp_rack: ratatui::layout::Rect,
    /// Whether the last paint actually drew the **pixel** office.
    ///
    /// Set by `render::render_game_mode` beside `last_mcp_rack`. The ambient
    /// animations (sip, steam, wall clock) are pixel-office art only, so this is
    /// what keeps [`Self::needs_ambient_tick`] from waking a Compact card grid
    /// or a Unicode fallback for animations it does not draw.
    pub(crate) last_pixel_painted: bool,
    /// Supervisor tooltip data, refreshed by the sync path (overlay-only).
    pub supervisor_info: SupervisorSnapshot,
    /// MCP rack tooltip data, refreshed by the sync path (overlay-only).
    pub mcp_info: McpRackSnapshot,
    /// Whether this Game Mode session already asked the shell for `mcp/list`.
    ///
    /// One request per open, so a shell that never answers (or answers with an
    /// error) cannot be re-asked ~12×/second for as long as the office stays
    /// up. Cleared by [`Self::toggle`] on the way in.
    pub(crate) mcp_fetch_dispatched: bool,
    /// Failed child IDs already used to arm `attention_until` (transition-only).
    attention_armed_ids: std::collections::HashSet<String>,
    /// Set when UI needs a redraw (tick/sync/hover); consumed by AppView::tick.
    redraw_dirty: bool,
    /// Last full snapshot+sync time (paint skips if recent — single sync owner).
    pub(crate) last_sync_at: Option<Instant>,
    /// Signature of the subagent map + sync inputs behind the last rebuild.
    ///
    /// PERF (RC16 PERF-2): `None` forces the next sync to rebuild (open/toggle).
    pub(crate) last_sync_sig: Option<u64>,
    /// Desk phase signature as of the end of the last sync — reused as the
    /// "before" value next sync so it is hashed once per sync, not twice.
    pub(crate) last_phase_sig: Option<u64>,
    /// Snapshot rebuilds actually performed (skips do not count).
    ///
    /// Observability for the RC16 PERF-2/PERF-3 gates; asserted by tests.
    pub(crate) sync_rebuilds: u64,
}

impl Default for GameModeState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameModeState {
    pub fn new() -> Self {
        Self {
            open: false,
            desks: std::array::from_fn(|_| DeskSlot::default()),
            seat_map: HashMap::new(),
            door_queue: VecDeque::new(),
            handoff_queue: VecDeque::new(),
            supervisor: SupervisorPhase::Idle,
            wall: WallMode::Standby,
            had_success: false,
            tick: 0,
            last_tick: Instant::now(),
            overflow_count: 0,
            attention_until: None,
            rack_active_until: None,
            success_fx_until: None,
            ambient_step: 0,
            ambient_at: Instant::now(),
            roomba_step: 0,
            clock_hm: local_clock_bucket(),
            next_skin: 0,
            pixel_bg_full: None,
            pixel_bg_scaled: None,
            pixel_cell_w: 0,
            pixel_cell_h: 0,
            pixel_bg_scale: 0,
            pixel_bg_tint: 0,
            pixel_frame: None,
            pixel_paint: None,
            pixel_halfblock: None,
            pixel_compose_scratch: None,
            pixel_frame_fp: 0,
            pixel_mode: true,
            hover: None,
            keyboard_focus: None,
            hover_screen: None,
            last_stage: None,
            last_desks: [ratatui::layout::Rect::default(); 6],
            last_supervisor: ratatui::layout::Rect::default(),
            last_mcp_rack: ratatui::layout::Rect::default(),
            last_pixel_painted: false,
            supervisor_info: SupervisorSnapshot::default(),
            mcp_info: McpRackSnapshot::default(),
            mcp_fetch_dispatched: false,
            attention_armed_ids: std::collections::HashSet::new(),
            redraw_dirty: false,
            last_sync_at: None,
            last_sync_sig: None,
            last_phase_sig: None,
            sync_rebuilds: 0,
        }
    }

    /// Target shown in the popup: keyboard focus wins over mouse hover.
    pub fn focus_target(&self) -> Option<HoverTarget> {
        self.keyboard_focus.map(HoverTarget::Desk).or(self.hover)
    }

    /// Desk shown in popup / focus ring, when the focused target *is* a desk.
    pub fn focus_desk(&self) -> Option<usize> {
        match self.focus_target() {
            Some(HoverTarget::Desk(i)) => Some(i),
            _ => None,
        }
    }

    /// Mark that the next Slow tick / paint cycle should redraw.
    pub fn mark_redraw_dirty(&mut self) {
        self.redraw_dirty = true;
    }

    /// Consume redraw dirty flag (AppView::tick).
    pub fn take_redraw_dirty(&mut self) -> bool {
        let d = self.redraw_dirty;
        self.redraw_dirty = false;
        d
    }

    /// Update hover from terminal mouse coordinates using last paint layout.
    ///
    /// Returns `true` only when the **hovered target** changes (not every mouse
    /// cell). Popup anchors to entry cell; micro-moves on the same target do not
    /// repaint (triple-scan hover throttle) — `agent_view::input` relies on that
    /// to drop the flood of `Moved` events.
    ///
    /// Seats win over the Supervisor when rects overlap, so the desk-only
    /// behaviour is preserved exactly wherever a desk is hit.
    pub fn update_hover(&mut self, col: u16, row: u16) -> bool {
        let prev = self.hover;
        let new_target = self.hit_test(col, row);
        if new_target == prev {
            return false;
        }
        self.hover = new_target;
        // Anchor popup once per target enter (or clear).
        self.hover_screen = new_target.map(|_| (col, row));
        // Mouse landing on a target takes the card over from Tab focus (which
        // otherwise wins in `focus_target`); empty floor keeps Tab focus.
        if new_target.is_some() && new_target != self.keyboard_focus.map(HoverTarget::Desk) {
            self.keyboard_focus = None;
        }
        self.mark_redraw_dirty();
        true
    }

    /// Hover target at a terminal cell, if any.
    fn hit_test(&self, col: u16, row: u16) -> Option<HoverTarget> {
        let hit = |r: ratatui::layout::Rect| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.x.saturating_add(r.width)
                && row >= r.y
                && row < r.y.saturating_add(r.height)
        };
        if let Some(i) = self
            .last_desks
            .iter()
            .enumerate()
            .find_map(|(i, r)| (hit(*r) && self.desks[i].is_occupied()).then_some(i))
        {
            return Some(HoverTarget::Desk(i));
        }
        if hit(self.last_supervisor) {
            return Some(HoverTarget::Supervisor);
        }
        hit(self.last_mcp_rack).then_some(HoverTarget::McpRack)
    }

    pub fn clear_hover(&mut self) {
        if self.hover.is_some() || self.hover_screen.is_some() || self.keyboard_focus.is_some() {
            self.mark_redraw_dirty();
        }
        self.hover = None;
        self.hover_screen = None;
        self.keyboard_focus = None;
    }

    /// Cycle keyboard focus across occupied desks (Tab). Returns true if focus moved.
    pub fn focus_next_desk(&mut self) -> bool {
        let occupied: Vec<usize> = (0..DESK_COUNT)
            .filter(|&i| self.desks[i].is_occupied())
            .collect();
        if occupied.is_empty() {
            self.keyboard_focus = None;
            return false;
        }
        // Same precedence as `focus_desk`: Tab focus, else a hovered desk.
        let cur = self.focus_desk();
        let next = match cur.and_then(|c| occupied.iter().position(|&i| i == c)) {
            Some(pos) => occupied[(pos + 1) % occupied.len()],
            None => occupied[0],
        };
        self.keyboard_focus = Some(next);
        if let Some(r) = self.last_desks.get(next) {
            self.hover_screen = Some((r.x.saturating_add(1), r.y.saturating_add(1)));
        }
        self.mark_redraw_dirty();
        true
    }

    /// Cycle keyboard focus backwards (Shift+Tab).
    pub fn focus_prev_desk(&mut self) -> bool {
        let occupied: Vec<usize> = (0..DESK_COUNT)
            .filter(|&i| self.desks[i].is_occupied())
            .collect();
        if occupied.is_empty() {
            self.keyboard_focus = None;
            return false;
        }
        // Same precedence as `focus_desk`: Tab focus, else a hovered desk.
        let cur = self.focus_desk();
        let next = match cur.and_then(|c| occupied.iter().position(|&i| i == c)) {
            Some(pos) => occupied[(pos + occupied.len() - 1) % occupied.len()],
            None => occupied[occupied.len() - 1],
        };
        self.keyboard_focus = Some(next);
        if let Some(r) = self.last_desks.get(next) {
            self.hover_screen = Some((r.x.saturating_add(1), r.y.saturating_add(1)));
        }
        self.mark_redraw_dirty();
        true
    }

    /// Halfblock paint source: terminal-resolution buffer (preferred) or high-res.
    pub fn pixel_paint_frame(&self) -> Option<&RgbaImage> {
        self.pixel_paint.as_ref().or(self.pixel_frame.as_ref())
    }

    /// Whether pure tick advances change the **composited** pixel frame.
    ///
    /// PERF INVARIANT: idle/thinking-only rooms freeze the sprite frame bucket so
    /// `tick_anim` (~12 Hz via `TickDemand::Slow`) does **not** force recompose.
    /// Typing, walk frames, celebrate/fail FX, and a working supervisor still
    /// sample `tick / 4`. Hover focus ring is a ratatui overlay — never here.
    fn pixel_needs_tick_frame(&self) -> bool {
        if matches!(
            self.supervisor,
            SupervisorPhase::Working | SupervisorPhase::Reviewing
        ) {
            return true;
        }
        self.desks.iter().any(|d| {
            d.is_occupied()
                && matches!(
                    d.phase,
                    ActorPhase::AtDeskWorking
                        | ActorPhase::SpawnWalk
                        | ActorPhase::Celebrate
                        | ActorPhase::FailBeat
                        | ActorPhase::WalkToBoss
                        | ActorPhase::Handoff
                        | ActorPhase::ExitDoor
                )
        })
    }

    /// Finest `tick` divisor any composed sprite is reading this frame (§4 #9).
    ///
    /// 4 is the floor: the walkers, the fail beat, the supervisor and the MCP
    /// rack all ride the global `(tick / 4) % 4` bucket, so the room can never
    /// hash anything coarser. A typing desk may ask for a *finer* one
    /// ([`BusyLevel::frame_divisor`]) — that, and only that, is what makes a hot
    /// desk type faster.
    ///
    /// COST: a room with any Hot desk moves its fingerprint (and its repaints)
    /// at `tick / 2` instead of `tick / 4` — roughly double the recompose rate,
    /// ~6 Hz instead of ~3 Hz, and only while a desk is genuinely streaming at
    /// [`BUSY_HOT_ENTER`] tokens/sec or better. Calm desks are *cheaper* than
    /// the old fixed cadence but cannot lower the floor.
    pub(crate) fn frame_bucket_divisor(&self) -> u64 {
        self.desks
            .iter()
            .filter(|d| d.is_occupied() && matches!(d.phase, ActorPhase::AtDeskWorking))
            .map(|d| d.busy.frame_divisor())
            .fold(4, u64::min)
    }

    /// Fingerprint for pixel recompose — **only** inputs that change
    /// [`super::compose::compose_cell_frame`] output.
    ///
    /// PERF INVARIANTS (RC13):
    /// 1. Pure `tick_anim` while all desks are empty/thinking and the supervisor
    ///    is Idle/Waiting must keep `pixel_frame_fp` stable (no recompose)
    ///    **within one [`AMBIENT_PERIOD`]**. RC16 §4 #7 relaxes the RC13
    ///    invariant by exactly that much and no more: an idle room recomposes on
    ///    the slow ambient step edge (~0.4 Hz), never on the ~12 Hz tick, and the
    ///    `tick / 4` sprite bucket stays frozen — the ambient art rides
    ///    [`Self::ambient_step`], a separate wall-clock counter.
    /// 2. `hover` / `hover_screen` / `supervisor_info` are **excluded** — focus
    ///    ring + hover card are painted as buffer overlays after halfblock
    ///    paint, so neither the hovered target nor the live model/turn/context
    ///    text behind the Supervisor card may force a recompose.
    /// 3. Wall title, overflow, labels, tokens, elapsed, activity are **excluded**
    ///    (status strip / hover popup only).
    /// 4. `anim_t` is hashed only for phases whose composited output actually
    ///    moves with it ([`phase_anim_t_is_visible`]) — not for the seated desk
    ///    blink, which uses the tick frame bucket.
    /// 5. Scaled BG cache is independent — see [`Self::ensure_pixel_frame`].
    /// 6. `mcp_info` is excluded for the same reason as `supervisor_info` — the
    ///    rack *card* is an overlay. Only the derived
    ///    [`Self::rack_burst_active`] bool, which the composed LEDs read, is in.
    /// 7. The success wave (RC16 §4 #8) is hashed **only while it runs**: its
    ///    bucket is `None` from the instant it expires, so the room's
    ///    fingerprint returns to exactly the value it had before the wave and
    ///    re-freezes. A wave that kept hashing after expiry would recompose the
    ///    room forever — the trap, since it fires as the room goes idle.
    /// 8. Each typing desk's [`BusyLevel`] is hashed for the one phase that
    ///    composes it, together with the finest tick bucket in use
    ///    ([`Self::frame_bucket_divisor`]) — hashing the level without the
    ///    finer bucket would freeze a hot desk's extra frames.
    /// 9. The floor robot's [`Self::roomba_step`] (RC16 §4 #11) is hashed
    ///    unconditionally and still costs nothing: it can only advance on a
    ///    `tick / 4` edge in a room that is already animating, i.e. on ticks the
    ///    bucket in invariant 8 already moved the fingerprint on. In a frozen
    ///    room it is a constant, so invariant 1 is untouched.
    /// 10. The two **time-derived** inputs (the rack burst bool and the wave
    ///    bucket) are read at a caller-supplied `now`, never from their own
    ///    `Instant::now()`. Compose reads the same two facts, and both used to
    ///    take their own clock sample: a deadline or a 150 ms wave bucket
    ///    falling between the two reads composed pixels one bucket ahead of the
    ///    fingerprint they were then cached under. See
    ///    [`Self::ensure_pixel_frame`], which snapshots one `Instant` and
    ///    threads it into both.
    fn visual_fingerprint(&self, cell_w: u16, cell_h: u16, now: Instant) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        cell_w.hash(&mut h);
        cell_h.hash(&mut h);
        super::sprites_pixel::effective_pixel_scale(cell_w, cell_h).hash(&mut h);
        (self.supervisor as u8).hash(&mut h);
        // One bool: the rack's idle art vs its lit art. Quantized by
        // construction — a deadline compared against `now` can only ever be
        // on or off, so this cannot force a recompose per tick.
        self.rack_burst_active(now).hash(&mut h);
        // One-shot golden sweep: `None` once it expires, so the fingerprint
        // returns to its pre-wave value and the room parks (see invariant 7).
        self.success_wave_bucket_at(now).hash(&mut h);
        // Wall clock hands + the day/night tint band both derive from this pair
        // and nothing finer, so the clock costs at most 6 recomposes/hour.
        self.clock_hm.hash(&mut h);
        // Slow ambient bucket (~ one step per AMBIENT_PERIOD) only when a
        // composed sprite reads it — see [`Self::ambient_is_visible`].
        if self.ambient_is_visible() {
            self.ambient_step.hash(&mut h);
        } else {
            0u64.hash(&mut h);
        }
        // Sprite frame bucket (~ tick÷4, finer while a desk is Hot) only when
        // compose samples it.
        if self.pixel_needs_tick_frame() {
            (self.tick / self.frame_bucket_divisor()).hash(&mut h);
        } else {
            0u64.hash(&mut h);
        }
        // Floor robot: hashed unconditionally, and free anyway — the counter can
        // only move on a tick the branch above already moved on (see invariant
        // 9 and the doc on `roomba_step`). Hashing it only while the room
        // animates would leave the *parked* position resting on the fact that
        // `pixel_frame_fp` holds one value rather than a set, which is true but
        // far too subtle to build a freeze invariant on.
        self.roomba_step.hash(&mut h);
        for d in &self.desks {
            d.child_session_id.hash(&mut h);
            (d.phase as u8).hash(&mut h);
            d.skin.hash(&mut h);
            // Typing cadence — only the phase that actually composes a keyboard
            // reads it, so a thinking desk's throughput cannot dirty an idle
            // room (the RC13 freeze, invariant 1).
            if matches!(d.phase, ActorPhase::AtDeskWorking) {
                (d.busy as u8).hash(&mut h);
            }
            // Walk path smoothness — SpawnWalk / WalkToBoss / ExitDoor slides.
            if phase_anim_t_is_visible(d.phase) {
                ((d.anim_t * 20.0) as u8).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Ensure a cell-resolution RGBA frame is ready for halfblock paint.
    ///
    /// Returns true when `pixel_frame` / cell cache can be painted. Never PNG-encodes.
    ///
    /// PERF: skips recompose when [`Self::visual_fingerprint`] matches; rescales
    /// the office BG only when cell size or `pixel_scale()` changes (never on
    /// hover-only or pure-idle tick frames). Builds halfblock cell cache once
    /// per fingerprint so HIT paints skip image sampling.
    ///
    /// COHERENCE: takes **one** clock sample for the whole frame and threads it
    /// into both the fingerprint and the compose pass (fingerprint invariant
    /// 10). Both derive the rack-burst bool and the success-wave bucket from
    /// time; sampling the clock separately in each let a deadline or a 150 ms
    /// bucket land between them and cache the composed pixels under a
    /// fingerprint describing the *other* side of the edge.
    pub fn ensure_pixel_frame(&mut self, cell_w: u16, cell_h: u16) -> bool {
        if !self.pixel_mode || cell_w == 0 || cell_h == 0 {
            return false;
        }
        let now = Instant::now();
        let fp = self.visual_fingerprint(cell_w, cell_h, now);
        // Hit: terminal paint + cell cache present (high-res scratch optional).
        if self.pixel_frame_fp == fp
            && self.pixel_paint.is_some()
            && self.pixel_halfblock.is_some()
        {
            return true;
        }

        // Decode full BG once.
        if self.pixel_bg_full.is_none() {
            match super::compose::load_office_background() {
                Ok(bg) => self.pixel_bg_full = Some(bg),
                Err(e) => {
                    tracing::warn!(error = %e, "game mode: failed to load office background");
                    self.pixel_mode = false;
                    return false;
                }
            }
        }

        // Rescale BG only when terminal cell size, pixel_scale asset factor, or
        // the day/night tint band changes (RC16 §4 #12 — the tint is baked into
        // the cached BG, see `pixel_bg_tint`).
        let scale = super::sprites_pixel::effective_pixel_scale(cell_w, cell_h).max(1);
        let tint = super::compose::hour_tint_band(self.clock_hm.0);
        if self.pixel_cell_w != cell_w
            || self.pixel_cell_h != cell_h
            || self.pixel_bg_scale != scale
            || self.pixel_bg_tint != tint
            || self.pixel_bg_scaled.is_none()
        {
            let Some(full) = self.pixel_bg_full.as_ref() else {
                return false;
            };
            // Temporarily pin scale for this compose via thread-local? scale_bg uses
            // pixel_scale() — ensure effective scale is applied by sprites_pixel helper.
            let mut scaled =
                super::compose::scale_bg_to_cells_with_scale(full, cell_w, cell_h, scale);
            super::compose::apply_hour_tint(&mut scaled, tint);
            self.pixel_bg_scaled = Some(scaled);
            self.pixel_cell_w = cell_w;
            self.pixel_cell_h = cell_h;
            self.pixel_bg_scale = scale;
            self.pixel_bg_tint = tint;
            self.pixel_paint = None;
            self.pixel_halfblock = None;
        }

        // Borrow-and-restore so we never drop the scaled BG on a compose path.
        let Some(bg) = self.pixel_bg_scaled.take() else {
            return false;
        };
        // Frozen tick when nothing samples the frame bucket (idle/thinking room).
        let tick = if self.pixel_needs_tick_frame() {
            self.tick
        } else {
            0
        };
        // Reuse compose scratch canvas (triple-scan P1 — no full alloc every miss).
        let mut scratch = self
            .pixel_compose_scratch
            .take()
            .unwrap_or_else(|| RgbaImage::new(bg.width(), bg.height()));
        super::compose::compose_cell_frame_into_at(&mut scratch, &bg, self, tick, now);
        // Terminal-res paint buffer for halfblock (use_direct — no per-paint resize).
        //
        // PERF (RC16 PERF-5): both the paint buffer and the cell cache are
        // retained and overwritten in place. At a stable terminal size a
        // fingerprint miss now allocates nothing; only a size change reallocates.
        let paint_w = u32::from(cell_w).max(1);
        let paint_h = u32::from(cell_h).saturating_mul(2).max(1);
        let mut paint = self
            .pixel_paint
            .take()
            .filter(|p| p.width() == paint_w && p.height() == paint_h)
            .unwrap_or_else(|| RgbaImage::new(paint_w, paint_h));
        resample_nearest_into(&mut paint, &scratch);
        let mut halfblock = self.pixel_halfblock.take().unwrap_or_default();
        halfblock.fill_from_rgba(&paint, cell_w, cell_h);
        self.pixel_bg_scaled = Some(bg);
        // High-res only kept as reusable scratch (no second full-frame clone).
        self.pixel_frame = None;
        self.pixel_compose_scratch = Some(scratch);
        self.pixel_paint = Some(paint);
        self.pixel_halfblock = Some(halfblock);
        self.pixel_frame_fp = fp;
        true
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.last_tick = Instant::now();
            self.last_sync_at = None;
            // Nothing was synced while hidden: force a full rebuild on reopen.
            self.last_sync_sig = None;
            self.last_phase_sig = None;
            // Re-arm the one-shot `mcp/list` request (RC16 §3 step 3): reopening
            // is the user asking to look at the room again, and the cheapest
            // retry for a fetch that previously failed.
            self.mcp_fetch_dispatched = false;
            // The office was not ticking while hidden, so the wall clock is as
            // stale as the time it spent closed — re-read it before the first
            // paint rather than showing the old hands for one AMBIENT_PERIOD.
            self.clock_hm = local_clock_bucket();
            self.ambient_at = Instant::now();
            self.mark_redraw_dirty();
        } else {
            // PERF (RC16 PERF-7): a hidden office must impose no standing cost,
            // and a hidden office is not painting the pixel art the ambient wake
            // exists for (see [`Self::needs_ambient_tick`]).
            self.last_pixel_painted = false;
            self.release_pixel_memory();
        }
    }

    /// Whether a paint-side sync should run (tick already synced recently).
    ///
    /// A lower bound only: the rebuild itself is additionally gated by
    /// `sync_gate` (RC16 PERF-3), so an early paint-side call may find nothing
    /// to do and return without touching the room.
    pub fn needs_paint_sync(&self) -> bool {
        match self.last_sync_at {
            None => true,
            Some(t) => t.elapsed() >= Duration::from_millis(40),
        }
    }

    /// Whether the room still has something to animate or reconcile.
    ///
    /// PERF INVARIANT (RC16 PERF-1): an open Game Mode used to hold the event
    /// loop at [`crate::app::app_view::TickDemand::Slow`] (~12 Hz) forever. A
    /// frozen room — no seated desks, Idle/Waiting supervisor, empty queues, no
    /// armed attention window, nothing dirty — produces zero visual change per
    /// Slow tick, so it must contribute **no** `Slow` demand.
    ///
    /// What the app parks at then depends on the tier, because the ambient
    /// batch (RC16 §4 #7 / #12) gave the pixel office animations that exist
    /// *only* in a room nothing else is animating:
    /// - **Pixel office on screen:** [`Self::needs_ambient_tick`] takes over and
    ///   the app settles at [`crate::app::app_view::TickDemand::Ambient`]
    ///   (~0.33 wakeups/sec) — the standing cost of the idle coffee sip, the
    ///   Supervisor's steam and the composed wall-clock hands.
    /// - **Compact / Unicode fallback:** neither draws that art, so the ambient
    ///   wake is gated off too and the app really does park at
    ///   `TickDemand::None`.
    ///
    /// **Synced room state only.** Live agent liveness (a turn that just
    /// started, a background subagent still running) is not visible here and is
    /// checked by the caller (`AppView::tick_demand`) — parking on state that
    /// has not been synced into the room yet would strand the office.
    pub fn needs_animation_tick(&self) -> bool {
        if self.redraw_dirty || self.last_sync_at.is_none() {
            return true;
        }
        if matches!(
            self.supervisor,
            SupervisorPhase::Working | SupervisorPhase::Reviewing
        ) {
            return true;
        }
        if !self.handoff_queue.is_empty() || !self.door_queue.is_empty() {
            return true;
        }
        // Armed attention window: the wall must flip back when it expires, so
        // stay awake until a sync has *consumed* the expiry (see
        // `sync_from_snapshots`) — not merely until the deadline passes.
        if self.attention_until.is_some() {
            return true;
        }
        // Same shape for the MCP rack burst: `is_some()`, not `> now`, so
        // exactly one `tick_anim` observes the expiry, repaints the darkened
        // rack and only then lets the room park (see [`Self::rack_burst_active`]).
        if self.rack_active_until.is_some() {
            return true;
        }
        // ...and for the success wave, which is armed *precisely* as the room
        // goes idle: without this the loop would park on the same tick the wave
        // was armed on and the sweep would never take a step.
        if self.success_fx_until.is_some() {
            return true;
        }
        // Seated desks animate (typing, walks, celebrate/fail beats) and own the
        // hover/focus ring — an empty room has neither.
        self.desks.iter().any(|d| d.is_occupied())
    }

    /// Fingerprint bucket of the success wave as observed at `at` (RC16 §4 #8).
    ///
    /// `None` when no wave is armed **or** the armed one has already elapsed —
    /// which is what makes the wave provably one-shot: the hashed value returns
    /// to `None` at expiry whether or not [`Self::tick_anim`] has consumed the
    /// deadline yet, so the recomposed frame is byte-identical to the pre-wave
    /// one and the room re-freezes.
    fn success_wave_bucket_at(&self, at: Instant) -> Option<u8> {
        let until = self.success_fx_until?;
        let remaining = until.saturating_duration_since(at);
        if remaining.is_zero() {
            return None;
        }
        let elapsed_ms = SUCCESS_WAVE.saturating_sub(remaining).as_millis() as u64;
        Some((elapsed_ms / SUCCESS_WAVE_BUCKET_MS).min(SUCCESS_WAVE_BUCKETS - 1) as u8)
    }

    /// Sweep position `0.0..=1.0` of the live success wave, or `None` (§4 #8).
    ///
    /// Derived from [`Self::success_wave_bucket_at`], i.e. from exactly the
    /// value [`Self::visual_fingerprint`] hashes and nothing finer — the same
    /// contract the wall clock's hands are drawn under. Read by
    /// [`super::compose::paint_fx_success_wave`].
    ///
    /// WAKEUP BUDGET: **~18 Slow ticks, once per success event, and nothing
    /// standing.** The wave is armed on the edge into
    /// [`WallMode::WorkFinished`], which is by definition the moment the room
    /// stops animating for its own reasons, so its whole 1.5 s is a tail the
    /// office would otherwise have spent parked. It buys 10 recomposes (one per
    /// [`SUCCESS_WAVE_BUCKET_MS`]) and then the room parks — pinned by
    /// `expired_success_wave_is_consumed_and_lets_the_room_park`.
    ///
    /// Takes the observation instant rather than sampling its own clock, for
    /// the same reason as [`Self::rack_burst_active`]: a 150 ms bucket boundary
    /// crossed between the fingerprint and the compose pass would put the
    /// composed crest one step ahead of its own cache key (invariant 10).
    pub(crate) fn success_wave_t(&self, at: Instant) -> Option<f32> {
        let bucket = self.success_wave_bucket_at(at)?;
        Some(f32::from(bucket) / (SUCCESS_WAVE_BUCKETS - 1) as f32)
    }

    /// Whether the MCP rack's LEDs are lit this frame (RC16 §4 #5).
    ///
    /// FINGERPRINT: this bool **is** hashed by [`Self::visual_fingerprint`] —
    /// unlike the phase-derived predicate it replaced, `rack_active_until` is
    /// state of its own, so the idle↔lit edge would otherwise never recompose.
    /// It costs exactly two extra recomposes per burst (on, then off): the
    /// deadline is compared against `now`, so the hashed value is a single bit
    /// that flips twice and can never split a tick.
    ///
    /// WAKEUP BUDGET: **~0 in the normal case, ≤ 15 wakeups per burst tail in
    /// the worst case.** Tool calls come from a *running* subagent, and a
    /// seated working desk already holds [`Self::needs_animation_tick`] true
    /// and already recomposes every `tick / 4` bucket — so a burst armed while
    /// work is happening rides ticks the office was going to spend anyway. The
    /// only incremental cost is the tail: if every desk retires inside the
    /// [`RACK_BURST`] window, the room stays on the ~12 Hz Slow tick for the
    /// remainder of it instead of parking — at most `RACK_BURST` ÷
    /// `SLOW_TICK_INTERVAL` ≈ 14 extra wakeups, once, per burst. That tail is
    /// the price of the expiry being observable at all; see
    /// [`Self::tick_anim`], which consumes it on the edge.
    ///
    /// IDLE-FREEZE: deliberately **not** wired into
    /// [`Self::pixel_needs_tick_frame`]. Unfreezing the sprite bucket for a
    /// burst would also unfreeze the idle supervisor's two-frame pose and the
    /// thinking bubble — the exact relaxation RC13 forbids. So a burst in an
    /// otherwise frozen room shows a *lit but still* rack; the chase only runs
    /// while the room is animating for its own reasons, which is when tool
    /// calls actually happen.
    ///
    /// Takes the observation instant rather than sampling its own clock:
    /// compose reads the same fact to pick the rack art and must not land on
    /// the other side of the deadline from the fingerprint it is cached under
    /// (fingerprint invariant 10).
    pub(crate) fn rack_burst_active(&self, at: Instant) -> bool {
        self.rack_active_until.is_some_and(|t| t > at)
    }

    /// Slow ambient sprite frame: flips 0↔1 once per [`AMBIENT_PERIOD`].
    ///
    /// Fed to the two sprites the office pins to frame 0 today — the idle/waiting
    /// Supervisor (which reads `frame % 2` for its coffee steam) and the
    /// non-typing developer (which reads `frame % 4 < 2` for the thinking bubble
    /// and, now, the coffee sip). Compose doubles it for the developer so it
    /// lands on the two canonical keys [`super::sprites_pixel::dev_at_desk_frame_key`]
    /// already declares — **zero new cache keys**.
    pub(crate) fn ambient_frame(&self) -> u8 {
        (self.ambient_step % 2) as u8
    }

    /// Sprite frame of the floor robot: flips 0↔1 with every patrol step.
    ///
    /// Inside [`super::sprites_pixel::roomba_frame_key`]'s declared period of 2,
    /// so the robot's blinking lamp and swapping brush cost **two** cache keys
    /// and no more.
    pub(crate) fn roomba_frame(&self) -> u8 {
        (self.roomba_step % 2) as u8
    }

    /// Whether the floor robot is travelling rather than parked (RC16 §4 #11).
    ///
    /// Exactly [`Self::pixel_needs_tick_frame`] — the predicate that gates the
    /// step counter — exposed to `compose` so the dust trail is painted only
    /// while the robot is actually moving. Safe as a compose input for the same
    /// reason the predicate itself is: every value it reads (the supervisor
    /// phase, each desk's occupancy and phase) is hashed individually by
    /// [`Self::visual_fingerprint`], so two rooms that disagree about it also
    /// disagree about the fingerprint.
    pub(crate) fn roomba_is_moving(&self) -> bool {
        self.pixel_needs_tick_frame()
    }

    /// Whether any composed sprite actually reads [`Self::ambient_frame`].
    ///
    /// Mirrors [`Self::pixel_needs_tick_frame`]'s job for the slow bucket: the
    /// step is hashed into [`Self::visual_fingerprint`] only when it is visible,
    /// so a fully busy room (Working supervisor, every desk typing) does not pay
    /// an extra recompose every [`AMBIENT_PERIOD`] for pixels nothing draws.
    fn ambient_is_visible(&self) -> bool {
        if matches!(
            self.supervisor,
            SupervisorPhase::Idle | SupervisorPhase::Waiting
        ) {
            return true;
        }
        self.desks
            .iter()
            .any(|d| d.is_occupied() && matches!(d.phase, ActorPhase::AtDeskThinking))
    }

    /// Whether a room that has nothing else to animate still needs slow ticks.
    ///
    /// RULE 3 (RC16 §4 #7 / #12): [`Self::needs_animation_tick`] parks the event
    /// loop, so an ambient animation that only satisfied the fingerprint would
    /// still freeze — the ticks that advance it would never happen. This is the
    /// wake path, and `AppView::tick_demand` maps it to
    /// [`crate::app::app_view::TickDemand::Ambient`], **not** `Slow`.
    ///
    /// WAKEUP BUDGET: **~0.33 wakeups/sec** (one per
    /// [`crate::app::app_view::AMBIENT_TICK_INTERVAL`] = 3 s) for as long as the
    /// pixel office is on screen. That is the whole standing cost of relaxing the
    /// RC13 idle-freeze: 36× cheaper than the ~12/sec an open office cost before
    /// RC16 PERF-1, and it buys three animations that were shipped-but-dead (the
    /// Supervisor's idle coffee steam, the thinking bubble blink) or impossible
    /// (a real wall clock). Each wake costs one `AppView::tick` body, whose
    /// Game Mode sync is itself skip-gated (RC16 PERF-2/3), plus one recompose
    /// when the ambient step is visible.
    ///
    /// Gated on the last paint really having drawn the pixel office: Compact is a
    /// card grid and the Unicode fallback draws none of this art, so waking
    /// either of them would buy nothing. A never-painted office also parks —
    /// opening one marks redraw dirty, which is the wake that produces the first
    /// paint.
    ///
    /// The corollary: in those two tiers [`Self::clock_hm`] is never refreshed,
    /// because nothing ever calls [`Self::tick_anim`]. Nothing may read it
    /// there. The Unicode office's wall-strip clock therefore samples
    /// [`local_clock_bucket`] live at paint time instead — it is a ratatui
    /// overlay, so a live read reaches no fingerprint and needs no wake (see
    /// [`super::render::paint_wall_display`]).
    pub fn needs_ambient_tick(&self) -> bool {
        self.pixel_mode && self.last_pixel_painted
    }

    /// Whether the room can be left alone when the subagent data is unchanged.
    ///
    /// PERF (RC16 PERF-2): [`Self::sync_from_snapshots`] is a no-op for
    /// byte-identical snapshots *only* while nothing in the room moves on its
    /// own. In-flight sequences do move: `tick_anim` promotes walks and clears
    /// desks between syncs, which frees seats for the door queue, retires the
    /// handoff queue and re-derives the supervisor + wall. An armed attention
    /// window likewise flips the wall back when it expires. Any of those means
    /// the next sync must actually run.
    pub(crate) fn room_is_settled(&self) -> bool {
        if !self.handoff_queue.is_empty() || !self.door_queue.is_empty() {
            return false;
        }
        if self.attention_until.is_some() {
            return false;
        }
        self.desks.iter().all(|d| {
            d.is_empty()
                || matches!(
                    d.phase,
                    ActorPhase::AtDeskWorking | ActorPhase::AtDeskThinking
                )
        })
    }

    /// Sync seats from current subagent snapshots + whether main agent is streaming.
    ///
    /// `waiting_on_user`: permission queue / question UI needs human input
    /// (drives [`WallMode::WaitingOnYou`] when nothing is running).
    pub fn sync_from_snapshots(
        &mut self,
        agents: &[DeskAgentSnapshot],
        supervisor_working: bool,
        tier: GameTier,
        waiting_on_user: bool,
    ) {
        // Consume an expired attention window on the *edge*, not by level test.
        //
        // RC16: `room_is_settled` and `needs_animation_tick` both key off
        // `attention_until`. If both merely tested `> now` they would flip in the
        // same instant: this sync would be skipped as settled (leaving `wall` on
        // its last `NeedsAttention` value, since the wall is only re-derived at
        // the end of this fn) while the loop simultaneously parked — stranding
        // NEEDS ATTENTION on screen forever. Clearing it here, with both
        // predicates testing `is_some()`, guarantees exactly one sync observes
        // the expiry, re-derives the wall, and only then lets the room park.
        if self.attention_until.is_some_and(|t| t <= Instant::now()) {
            self.attention_until = None;
        }

        // Compact mid-walk: snap-complete handoffs (spec §7.8).
        //
        // No supervisor phase is set here: [`Self::update_supervisor`] runs
        // unconditionally at the end of this sync and derives it from the
        // (now cleared) desks + handoff queue, so an assignment in this loop
        // was dead by construction (RC16 B12).
        if !tier.uses_office_art() {
            for i in 0..DESK_COUNT {
                if matches!(
                    self.desks[i].phase,
                    ActorPhase::WalkToBoss
                        | ActorPhase::Handoff
                        | ActorPhase::ExitDoor
                        | ActorPhase::Celebrate
                        | ActorPhase::SpawnWalk
                ) {
                    self.clear_desk(i);
                }
            }
            self.handoff_queue.clear();
        }

        let running_ids: std::collections::HashSet<&str> = agents
            .iter()
            .filter(|a| a.running)
            .map(|a| a.child_session_id.as_str())
            .collect();

        // Drop finished overflow IDs so +N stays accurate.
        self.door_queue
            .retain(|id| agents.iter().any(|a| a.child_session_id == *id && a.running));

        // Arm brief attention only on **new** failed child IDs (not every sync).
        //
        // PERF (RC16 P11): probe the armed set by `&str` and only allocate the
        // owned id on the arming transition. A failed child stays in the map
        // until the turn ends, so the old `insert(id.to_string())` allocated a
        // String per failed child on *every* sync just to throw it away.
        let mut new_fail = false;
        for id in agents
            .iter()
            .filter(|a| a.failed && !a.running)
            .map(|a| a.child_session_id.as_str())
        {
            if !self.attention_armed_ids.contains(id) {
                self.attention_armed_ids.insert(id.to_string());
                new_fail = true;
            }
        }
        // Drop armed ids that are gone from the map entirely.
        self.attention_armed_ids
            .retain(|id| agents.iter().any(|a| a.child_session_id == *id));
        if new_fail {
            self.attention_until = Some(Instant::now() + Duration::from_secs(12));
        }

        // Update existing seats / detect finishes.
        let mut tool_call_seen = false;
        for i in 0..DESK_COUNT {
            let Some(sid) = self.desks[i].child_session_id.clone() else {
                continue;
            };
            if let Some(snap) = agents.iter().find(|a| a.child_session_id == sid) {
                // PERF (RC16 PERF-2): compare before assigning — these three are
                // byte-identical on almost every sync, and `clone_from` on a
                // mismatch reuses the existing buffer instead of allocating.
                if self.desks[i].label != snap.label {
                    self.desks[i].label.clone_from(&snap.label);
                }
                if self.desks[i].subagent_type != snap.subagent_type {
                    self.desks[i].subagent_type.clone_from(&snap.subagent_type);
                }
                if self.desks[i].activity != snap.activity {
                    self.desks[i].activity.clone_from(&snap.activity);
                }
                self.desks[i].elapsed = snap.elapsed;
                // Typing cadence from real throughput (RC16 §4 #9). Measured
                // against wall time inside the desk, so a throttled or skipped
                // sync cannot change the rate it reads.
                self.desks[i].sample_throughput(snap.tokens);
                self.desks[i].tokens = snap.tokens;
                // Real work lights the MCP rack (RC16 §4 #5). The desk's own
                // `tool_calls` *is* the previous sync's value until the line
                // below overwrites it, so the increment edge needs no extra
                // field — and it is an edge, not a level, so a desk parked at
                // 12 calls does not hold the LEDs on forever.
                if snap.tool_calls > self.desks[i].tool_calls {
                    tool_call_seen = true;
                }
                self.desks[i].tool_calls = snap.tool_calls;
                self.desks[i].failed = snap.failed;

                if snap.running {
                    // Don't clobber walk animations.
                    if matches!(
                        self.desks[i].phase,
                        ActorPhase::AtDeskWorking
                            | ActorPhase::AtDeskThinking
                            | ActorPhase::SpawnWalk
                    ) {
                        let thinking = activity_is_thinking(&snap.activity);
                        if !matches!(self.desks[i].phase, ActorPhase::SpawnWalk) {
                            self.desks[i].phase = if thinking {
                                ActorPhase::AtDeskThinking
                            } else {
                                ActorPhase::AtDeskWorking
                            };
                        }
                    }
                } else if !snap.failed
                    && !self.desks[i].finish_started
                    && matches!(
                        self.desks[i].phase,
                        ActorPhase::AtDeskWorking
                            | ActorPhase::AtDeskThinking
                            | ActorPhase::SpawnWalk
                    )
                {
                    // Success finish → celebrate then handoff (one-shot per seating).
                    self.begin_success_finish(i, tier);
                } else if snap.failed
                    && !self.desks[i].finish_started
                    && matches!(
                        self.desks[i].phase,
                        ActorPhase::AtDeskWorking
                            | ActorPhase::AtDeskThinking
                            | ActorPhase::SpawnWalk
                    )
                {
                    self.desks[i].finish_started = true;
                    self.desks[i].phase = ActorPhase::FailBeat;
                    self.desks[i].phase_started = Instant::now();
                    self.desks[i].anim_t = 0.0;
                }
            } else if !running_ids.contains(sid.as_str())
                && matches!(
                    self.desks[i].phase,
                    ActorPhase::AtDeskWorking | ActorPhase::AtDeskThinking | ActorPhase::SpawnWalk
                )
            {
                // Disappeared without snapshot — treat as clear.
                self.clear_desk(i);
            }
        }

        // Seat new running agents.
        for snap in agents.iter().filter(|a| a.running) {
            if self.seat_map.contains_key(&snap.child_session_id) {
                continue;
            }
            if self.door_queue.contains(&snap.child_session_id) {
                continue;
            }
            match self.find_free_desk() {
                Some(idx) => self.seat_agent(idx, snap, tier),
                None => {
                    self.door_queue.push_back(snap.child_session_id.clone());
                }
            }
        }

        // Promote from door queue.
        while let Some(idx) = self.find_free_desk() {
            let Some(sid) = self.door_queue.pop_front() else {
                break;
            };
            if let Some(snap) = agents.iter().find(|a| a.child_session_id == sid) {
                if snap.running {
                    self.seat_agent(idx, snap, tier);
                }
            }
        }

        // Arm / re-arm the rack burst. Marking dirty only on the dark→lit edge
        // is enough: a re-arm inside a live burst changes no pixels (the chase
        // rides the `tick / 4` bucket the working room already repaints on),
        // while the edge itself flips the composed rack art.
        if tool_call_seen {
            if !self.rack_burst_active(Instant::now()) {
                self.mark_redraw_dirty();
            }
            self.rack_active_until = Some(Instant::now() + RACK_BURST);
        }

        self.overflow_count = self.door_queue.len();
        self.update_supervisor(supervisor_working);
        let attention_active = self
            .attention_until
            .is_some_and(|t| t > Instant::now());
        let wall_before = self.wall;
        self.wall = super::wall::compute_wall_mode(
            agents,
            supervisor_working,
            self.had_success,
            self.desks.iter().any(|d| {
                matches!(
                    d.phase,
                    ActorPhase::Celebrate
                        | ActorPhase::WalkToBoss
                        | ActorPhase::Handoff
                        | ActorPhase::ExitDoor
                )
            }),
            attention_active,
            waiting_on_user,
        );

        // Office-wide success wave (RC16 §4 #8), armed on the **edge** into
        // WorkFinished. A level test (`self.wall == WorkFinished`) would re-arm
        // it on every sync for the rest of the session: `had_success` is sticky,
        // so once the last subagent lands the wall never leaves WorkFinished
        // until new work starts — and a permanently re-armed wave is a room that
        // can never park. `finish_started` is the same one-shot discipline one
        // level down, per desk.
        if self.wall == WallMode::WorkFinished && wall_before != WallMode::WorkFinished {
            self.success_fx_until = Some(Instant::now() + SUCCESS_WAVE);
            self.mark_redraw_dirty();
        }
    }

    /// Drop **every** image buffer Game Mode owns, including the decoded
    /// full-res office background.
    ///
    /// PERF (RC16 PERF-7): called when Game Mode is toggled closed. Hidden, the
    /// office used to keep ~8-10 MB resident for the rest of the process
    /// (`pixel_bg_full` alone is 1448×1086 RGBA ≈ 6.3 MB). `pixel_bg_full` is
    /// dropped too: reopening re-decodes the embedded PNG, which is expected to
    /// be small next to the background rescale that same path already performs
    /// for the new stage — but neither has been benchmarked, so treat the
    /// trade-off as reasoned, not measured.
    pub fn release_pixel_memory(&mut self) {
        self.invalidate_pixel_cache();
        self.pixel_bg_full = None;
    }

    /// Drop composited pixel caches (e.g. terminal resize). Scaled BG rebuilds
    /// on the next [`Self::ensure_pixel_frame`]; the decoded full-res BG is
    /// kept (see [`Self::release_pixel_memory`] to drop that too).
    pub fn invalidate_pixel_cache(&mut self) {
        self.pixel_frame = None;
        self.pixel_paint = None;
        self.pixel_halfblock = None;
        self.pixel_compose_scratch = None;
        self.pixel_frame_fp = 0;
        self.pixel_bg_scaled = None;
        self.pixel_cell_w = 0;
        self.pixel_cell_h = 0;
        self.pixel_bg_scale = 0;
        self.pixel_bg_tint = 0;
    }

    fn begin_success_finish(&mut self, desk: usize, tier: GameTier) {
        if self.desks[desk].finish_started {
            return;
        }
        self.desks[desk].finish_started = true;
        self.had_success = true;
        self.desks[desk].phase = ActorPhase::Celebrate;
        self.desks[desk].phase_started = Instant::now();
        self.desks[desk].anim_t = 0.0;
        let _ = tier; // office vs compact share celebrate path; compact clears faster in tick
    }

    fn seat_agent(&mut self, idx: usize, snap: &DeskAgentSnapshot, tier: GameTier) {
        let skin = self.next_skin % DESK_COUNT as u8;
        self.next_skin = self.next_skin.wrapping_add(1);
        let phase = if tier.uses_office_art() {
            ActorPhase::SpawnWalk
        } else {
            ActorPhase::AtDeskWorking
        };
        self.desks[idx] = DeskSlot {
            child_session_id: Some(snap.child_session_id.clone()),
            label: snap.label.clone(),
            subagent_type: snap.subagent_type.clone(),
            phase,
            elapsed: snap.elapsed,
            tokens: snap.tokens,
            tool_calls: snap.tool_calls,
            activity: snap.activity.clone(),
            failed: snap.failed,
            // A fresh seat starts at the cadence every desk used before RC16
            // §4 #9 and measures its first rate one BUSY_SAMPLE_PERIOD later —
            // seeding `prev_tokens` from the snapshot means an agent that was
            // already streaming before it got a desk does not read as a burst.
            busy: BusyLevel::Normal,
            prev_tokens: snap.tokens,
            tokens_at: Instant::now(),
            skin,
            anim_t: 0.0,
            phase_started: Instant::now(),
            finish_started: false,
        };
        self.seat_map.insert(snap.child_session_id.clone(), idx);
    }

    fn find_free_desk(&self) -> Option<usize> {
        self.desks.iter().position(|d| d.is_empty())
    }

    fn clear_desk(&mut self, idx: usize) {
        if let Some(sid) = self.desks[idx].child_session_id.take() {
            self.seat_map.remove(&sid);
        }
        self.desks[idx] = DeskSlot {
            skin: self.desks[idx].skin,
            phase_started: Instant::now(),
            ..DeskSlot::default()
        };
        self.desks[idx].child_session_id = None;
    }

    fn update_supervisor(&mut self, supervisor_working: bool) {
        let reviewing = self.desks.iter().any(|d| {
            matches!(
                d.phase,
                ActorPhase::Handoff | ActorPhase::WalkToBoss | ActorPhase::Celebrate
            )
        }) || !self.handoff_queue.is_empty();

        self.supervisor = if supervisor_working {
            SupervisorPhase::Working
        } else if reviewing {
            SupervisorPhase::Reviewing
        } else if self.desks.iter().any(|d| d.is_occupied() && !d.failed) {
            SupervisorPhase::Waiting
        } else {
            SupervisorPhase::Idle
        };
    }

    /// Advance animations. Call once per `SLOW_TICK_INTERVAL` (~12 Hz) while the
    /// room animates ([`Self::needs_animation_tick`]), or once per
    /// `AMBIENT_TICK_INTERVAL` (~3 s) while a parked pixel office still wants
    /// ambient steps ([`Self::needs_ambient_tick`]).
    ///
    /// Nothing user-visible may depend on this being called at all: a frozen
    /// Compact/Unicode room gets neither cadence (RC16 PERF-1). That is why the
    /// wall-strip clock samples [`local_clock_bucket`] at paint time rather
    /// than reading the [`Self::clock_hm`] this refreshes — `clock_hm` exists
    /// for the *pixel* office, whose composed hands need a fingerprint input.
    ///
    /// Marks redraw dirty when visual output may change (working desks, walks,
    /// focus pulse edge, per-desk HUD text) — see the dirty gate below for the
    /// per-path cadences.
    pub fn tick_anim(&mut self, tier: GameTier) {
        // Structural changes below (phase transitions, desk clears, handoff
        // promotion) are inputs to `sync_from_snapshots`: they free seats, drain
        // the handoff queue and re-derive the supervisor + wall. Invalidate the
        // cached sync signatures so the next sync cannot be skipped (RC16
        // PERF-2). A settled room only advances `anim_t`, which no sync input
        // reads — and every structural branch here needs a phase or a queue
        // entry that [`Self::room_is_settled`] already rejects.
        if !self.room_is_settled() {
            self.last_sync_sig = None;
            self.last_phase_sig = None;
        }
        // Consume an expired rack burst on the *edge*, exactly like
        // `attention_until` (see `sync_from_snapshots`) but here, because
        // nothing a *sync* derives depends on it — the rack art is read
        // straight off `rack_burst_active` at compose time. `tick_anim` runs on
        // every Slow tick the room is awake for, and `needs_animation_tick`
        // tests `is_some()`, so exactly one tick observes the expiry, repaints
        // the darkened rack, and only then lets the room park (RC16 PERF-1).
        if self.rack_active_until.is_some_and(|t| t <= Instant::now()) {
            self.rack_active_until = None;
            self.mark_redraw_dirty();
        }
        // Same edge-consuming shape for the success wave (RC16 §4 #8), and for
        // a sharper reason: it is armed exactly as the room goes idle, so
        // nothing else is left to repaint the sweep or to notice it ending.
        // `self.last_tick` is still the *previous* tick here (it is refreshed
        // below), so the bucket compare marks dirty once per composed step —
        // ~10 repaints across the wave rather than one per Slow tick.
        if let Some(until) = self.success_fx_until {
            let now = Instant::now();
            if until <= now {
                self.success_fx_until = None;
                self.mark_redraw_dirty();
            } else if self.success_wave_bucket_at(now)
                != self.success_wave_bucket_at(self.last_tick)
            {
                self.mark_redraw_dirty();
            }
        }
        // Slow ambient step (RC16 §4 #7 / #12). Wall-clock gated, so it runs at
        // the same rate whether the room is awake at ~12 Hz for its own reasons
        // or parked on the ambient wake — and it re-reads the clock at the only
        // cadence [`Self::clock_hm`]'s quantization can distinguish anyway.
        //
        // Dirty is marked only when composed pixels can actually differ: the
        // sip / steam frame when something draws it, the clock when its bucket
        // moved. A fully busy office therefore pays nothing for this.
        if self.ambient_at.elapsed() >= AMBIENT_PERIOD {
            self.ambient_at = Instant::now();
            self.ambient_step = self.ambient_step.wrapping_add(1);
            let clock = local_clock_bucket();
            let clock_moved = clock != self.clock_hm;
            self.clock_hm = clock;
            if self.ambient_is_visible() || clock_moved {
                self.mark_redraw_dirty();
            }
        }
        let tick_before = self.tick;
        let needs_frames = self.pixel_needs_tick_frame();
        let had_focus = self.focus_desk().is_some();
        self.tick = self.tick.wrapping_add(1);
        self.last_tick = Instant::now();
        // Focus ring pulse flips every 4 ticks (a ratatui overlay, so it keeps
        // the fixed cadence whatever the desks are doing).
        let bucket_edge = (tick_before / 4) != (self.tick / 4);
        // Sprite frame edge — finer than the focus pulse while a desk is Hot
        // (RC16 §4 #9). Must be the same divisor `visual_fingerprint` hashes,
        // or a hot desk's extra frames would be composed and never painted.
        let frame_div = self.frame_bucket_divisor();
        let frame_edge = (tick_before / frame_div) != (self.tick / frame_div);
        if had_focus && bucket_edge {
            self.mark_redraw_dirty();
        }
        // Floor robot (RC16 §4 #11): one patrol step per `tick / 4` bucket, and
        // **only** while the room already samples that bucket. Deliberately no
        // `mark_redraw_dirty` of its own — `frame_div` is 4 or finer, so every
        // edge this fires on is already a `frame_edge` below, and marking here
        // would double-count on the very path RC16 PERF-6 exists to thin out.
        // When the room freezes the robot simply stops where it is; sending it
        // home would mean animating a parked office.
        if needs_frames && bucket_edge {
            self.roomba_step = self.roomba_step.wrapping_add(1);
        }
        // Which office the paint path will draw (`render::render_game_mode`):
        // the pixel stage needs ≥40×8 cells, which every office tier already
        // guarantees (`layout::MIN_STAGE_W/H` are 72×18).
        let pixel_office = self.pixel_mode && tier.uses_office_art();
        if needs_frames {
            // PERF (RC16 PERF-6): the pixel office samples only the sprite
            // frame bucket (`tick / 4`, or `tick / 2` while a desk is Hot), so
            // a room of seated desks composes a pixel-identical frame on 3 of
            // every 4 ticks. Mark those on the bucket edge only.
            // Walks still mark every tick (their `anim_t` is fingerprint-visible),
            // and the Unicode office must keep per-tick marks — it animates at
            // `tick%2` / `tick%4` / `tick%6` with a `tick/2` activity marquee.
            let moves_between_buckets = self
                .desks
                .iter()
                .any(|d| d.is_occupied() && phase_anim_t_is_visible(d.phase));
            if !pixel_office || moves_between_buckets || frame_edge {
                self.mark_redraw_dirty();
            }
        } else if !pixel_office
            && (self.tick / HUD_REFRESH_TICKS) != (tick_before / HUD_REFRESH_TICKS)
            && self.desks.iter().any(|d| d.is_occupied())
        {
            // RC16 BUG-4: the pixel idle-freeze is right for a static sprite, but
            // Compact / Unicode paint per-desk monitor HUDs (elapsed timer, token
            // counts, scrolled activity) whose data the sync refreshes every
            // second. Nothing else marks those dirty, so an on-screen `01:23`
            // could sit frozen for minutes. Refresh at [`HUD_REFRESH_TICKS`].
            self.mark_redraw_dirty();
        }

        let compact = !tier.uses_office_art();
        let mut clear_after: Vec<usize> = Vec::new();

        for i in 0..DESK_COUNT {
            if self.desks[i].is_empty() {
                continue;
            }
            let elapsed = self.desks[i].phase_started.elapsed();
            match self.desks[i].phase {
                ActorPhase::SpawnWalk => {
                    let dur = Duration::from_millis(800);
                    self.desks[i].anim_t = (elapsed.as_secs_f32() / dur.as_secs_f32()).min(1.0);
                    if elapsed >= dur {
                        self.desks[i].phase = ActorPhase::AtDeskWorking;
                        self.desks[i].phase_started = Instant::now();
                        self.desks[i].anim_t = 0.0;
                    }
                }
                ActorPhase::Celebrate => {
                    let dur = if compact {
                        Duration::from_millis(350)
                    } else {
                        Duration::from_millis(400)
                    };
                    self.desks[i].anim_t = (elapsed.as_secs_f32() / dur.as_secs_f32()).min(1.0);
                    if elapsed >= dur {
                        if compact {
                            clear_after.push(i);
                        } else if matches!(self.desks[i].phase, ActorPhase::Celebrate) {
                            // Transition exactly once out of Celebrate.
                            if !self.handoff_queue.contains(&i)
                                && !self.desks.iter().any(|d| {
                                    matches!(
                                        d.phase,
                                        ActorPhase::WalkToBoss | ActorPhase::Handoff
                                    )
                                })
                            {
                                self.desks[i].phase = ActorPhase::WalkToBoss;
                                self.desks[i].phase_started = Instant::now();
                                self.desks[i].anim_t = 0.0;
                            } else if !self.handoff_queue.contains(&i) {
                                // Park in a waiting-to-walk phase without re-queue spam:
                                // stay Celebrate but already finish_started; only enqueue once.
                                self.handoff_queue.push_back(i);
                            }
                        }
                    }
                }
                ActorPhase::WalkToBoss => {
                    let dur = Duration::from_millis(900);
                    self.desks[i].anim_t = (elapsed.as_secs_f32() / dur.as_secs_f32()).min(1.0);
                    if elapsed >= dur {
                        self.desks[i].phase = ActorPhase::Handoff;
                        self.desks[i].phase_started = Instant::now();
                        self.desks[i].anim_t = 0.0;
                    }
                }
                ActorPhase::Handoff => {
                    let dur = Duration::from_millis(500);
                    self.desks[i].anim_t = (elapsed.as_secs_f32() / dur.as_secs_f32()).min(1.0);
                    if elapsed >= dur {
                        self.desks[i].phase = ActorPhase::ExitDoor;
                        self.desks[i].phase_started = Instant::now();
                        self.desks[i].anim_t = 0.0;
                    }
                }
                ActorPhase::ExitDoor => {
                    let dur = Duration::from_millis(700);
                    self.desks[i].anim_t = (elapsed.as_secs_f32() / dur.as_secs_f32()).min(1.0);
                    if elapsed >= dur {
                        clear_after.push(i);
                    }
                }
                ActorPhase::FailBeat => {
                    let dur = Duration::from_millis(900);
                    self.desks[i].anim_t = (elapsed.as_secs_f32() / dur.as_secs_f32()).min(1.0);
                    if elapsed >= dur {
                        clear_after.push(i);
                    }
                }
                ActorPhase::AtDeskWorking | ActorPhase::AtDeskThinking => {
                    self.desks[i].anim_t = (self.tick % 8) as f32 / 8.0;
                }
            }
        }

        for i in clear_after {
            self.clear_desk(i);
        }

        // Start next queued handoff if free.
        if !self
            .desks
            .iter()
            .any(|d| matches!(d.phase, ActorPhase::WalkToBoss | ActorPhase::Handoff))
        {
            while let Some(idx) = self.handoff_queue.pop_front() {
                if self.desks[idx].is_occupied()
                    && matches!(self.desks[idx].phase, ActorPhase::Celebrate)
                {
                    self.desks[idx].phase = ActorPhase::WalkToBoss;
                    self.desks[idx].phase_started = Instant::now();
                    self.desks[idx].anim_t = 0.0;
                    break;
                }
                if self.desks[idx].is_occupied()
                    && matches!(
                        self.desks[idx].phase,
                        ActorPhase::AtDeskWorking | ActorPhase::AtDeskThinking
                    )
                {
                    // Stale queue entry; skip.
                    continue;
                }
            }
        }
    }

    pub fn active_desk_count(&self) -> usize {
        self.desks.iter().filter(|d| d.is_occupied()).count()
    }
}

/// Nearest-neighbour resample of `src` into the already-allocated `dst`.
///
/// PERF (RC16 PERF-5): replaces `image::imageops::resize(.., Nearest)` on the
/// compose path so the terminal-res paint buffer is written in place instead of
/// reallocated on every fingerprint miss. Picks the same source pixel
/// `imageops` would — `floor((out + 0.5) * src/dst)` per axis — so the painted
/// frame is unchanged. `dst` dimensions define the target size; the Game Mode
/// caller always passes an exact integer downscale (`src = dst * pixel_scale`).
fn resample_nearest_into(dst: &mut RgbaImage, src: &RgbaImage) {
    let (dw, dh) = dst.dimensions();
    let (sw, sh) = src.dimensions();
    if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
        return;
    }
    let ratio_x = sw as f32 / dw as f32;
    let ratio_y = sh as f32 / dh as f32;
    for y in 0..dh {
        let sy = (((y as f32 + 0.5) * ratio_y) as u32).min(sh - 1);
        for x in 0..dw {
            let sx = (((x as f32 + 0.5) * ratio_x) as u32).min(sw - 1);
            let p = *src.get_pixel(sx, sy);
            dst.put_pixel(x, y, p);
        }
    }
}

/// Whether `phase` renders at a position driven by `anim_t`, i.e. whether the
/// composited office can change **between** `tick / 4` sprite bucket edges.
///
/// Single source of truth for two gates that must not drift apart:
/// [`GameModeState::visual_fingerprint`] (what forces a recompose) and
/// [`GameModeState::tick_anim`] (what forces a repaint). FailBeat samples the
/// frame bucket, not `anim_t`, so it stays out: its 900 ms span already covers
/// ~3 `tick / 4` edges, which is enough to alternate a 2-frame pose for free.
///
/// Celebrate is in because 400 ms is **not** enough — it covers barely one
/// bucket edge, so both its pose and its confetti would have shown a single
/// frame (RC16 §4 #2). Both now read `anim_t`
/// ([`super::compose::celebrate_pose_frame`],
/// [`super::compose::paint_fx_confetti`]), which costs ~5 recomposes per
/// subagent success — one per Slow tick of the phase — inside a
/// Celebrate→WalkToBoss→Handoff→ExitDoor sequence whose other 2.1 s already
/// recompose every tick. Lengthening Celebrate to ~1 s so the bucket could
/// drive it was the alternative; it would have delayed every handoff by 600 ms.
///
/// Handoff was excluded by RC16 BUG-3-bonus because
/// [`super::compose::walk_position`] pins the walker on the rug, making those
/// 500 ms of recomposes pixel-identical — pure waste. It is back in because
/// [`super::compose::paint_fx_handoff_papers`] now draws a burst of paper quads
/// whose arc *is* `anim_t`: the frames differ again, so the recomposes buy
/// something. The cost is bounded and rare — ~6 extra recomposes (one per Slow
/// tick of the 500 ms Handoff) per subagent completion, inside a
/// Celebrate→WalkToBoss→Handoff→ExitDoor sequence whose other 2 s already
/// recompose every tick. Driving the papers off the `tick/4` bucket instead
/// would have been free but would only have sampled ~2 arc positions in 500 ms,
/// i.e. a flicker rather than a throw.
fn phase_anim_t_is_visible(phase: ActorPhase) -> bool {
    matches!(
        phase,
        ActorPhase::SpawnWalk
            | ActorPhase::WalkToBoss
            | ActorPhase::ExitDoor
            | ActorPhase::Handoff
            | ActorPhase::Celebrate
    )
}

/// Local wall clock as `(hour 0..24, ten-minute 0..6)` (RC16 §4 #12).
///
/// `chrono` is already a dependency of this crate (`acp::tracker` reads
/// `Local::now()`), so the real local time costs no new dependency — and `std`
/// alone cannot do it: `SystemTime` is UTC and carries no zone offset, which
/// would put the office clock and the day/night tint hours off for most users.
///
/// The ten-minute quantization is the whole perf contract: it is what the
/// fingerprint hashes and what the composed hands are drawn from, so the clock
/// can force at most 6 recomposes per hour.
///
/// Two readers, for two different reasons. The pixel office reads it *through*
/// [`GameModeState::clock_hm`], because its hands and hour tint are composed
/// pixels and must therefore be a fingerprint input. The Unicode office's wall
/// strip calls this directly at paint time
/// ([`super::render::paint_wall_display`]): that text is a ratatui overlay
/// painted after the halfblock blit, so a live read can never reach the
/// fingerprint, and it keeps the strip honest in the two tiers
/// ([`GameModeState::needs_ambient_tick`] excludes them) where nothing ever
/// refreshes `clock_hm`.
pub(super) fn local_clock_bucket() -> (u8, u8) {
    use chrono::Timelike;
    let now = chrono::Local::now();
    ((now.hour() % 24) as u8, (now.minute() / 10) as u8)
}

/// Case-insensitive `contains("think")` over the live activity label.
///
/// PERF (RC16 PERF-2): runs once per running desk per sync — the previous
/// `to_ascii_lowercase().contains(..)` allocated a String each time.
fn activity_is_thinking(activity: &str) -> bool {
    activity
        .as_bytes()
        .windows(5)
        .any(|w| w.eq_ignore_ascii_case(b"think"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: &str, running: bool) -> DeskAgentSnapshot {
        DeskAgentSnapshot {
            child_session_id: id.into(),
            label: id.into(),
            subagent_type: "explore".into(),
            running,
            failed: false,
            elapsed: Duration::from_secs(12),
            tokens: 1000,
            tool_calls: 3,
            activity: "Working".into(),
        }
    }

    #[test]
    fn seats_up_to_six_and_overflows() {
        let mut s = GameModeState::new();
        let agents: Vec<_> = (0..8).map(|i| snap(&format!("c{i}"), true)).collect();
        s.sync_from_snapshots(&agents, false, GameTier::Comfort, false);
        assert_eq!(s.active_desk_count(), 6);
        assert_eq!(s.overflow_count, 2);
    }

    #[test]
    fn success_starts_celebrate() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("a", true)], false, GameTier::Comfort, false);
        assert_eq!(s.active_desk_count(), 1);
        s.sync_from_snapshots(&[snap("a", false)], false, GameTier::Comfort, false);
        assert!(matches!(s.desks[0].phase, ActorPhase::Celebrate));
        assert!(s.had_success);
        assert!(s.desks[0].finish_started);
    }

    #[test]
    fn success_finish_is_one_shot() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("a", true)], false, GameTier::Comfort, false);
        s.sync_from_snapshots(&[snap("a", false)], false, GameTier::Comfort, false);
        assert!(matches!(s.desks[0].phase, ActorPhase::Celebrate));
        // Second "not running" sync must not re-start celebrate / re-queue.
        let started = s.desks[0].phase_started;
        s.sync_from_snapshots(&[snap("a", false)], false, GameTier::Comfort, false);
        assert!(matches!(s.desks[0].phase, ActorPhase::Celebrate));
        assert_eq!(s.desks[0].phase_started, started);
        assert_eq!(s.handoff_queue.len(), 0);
    }

    /// B12: the compact snap-complete branch used to assign
    /// `SupervisorPhase::Reviewing` per cleared desk, which
    /// [`GameModeState::update_supervisor`] overwrote at the end of the same
    /// sync. The phase is derived from the room, never written mid-sync.
    #[test]
    fn compact_snap_complete_leaves_the_supervisor_derived() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("a", true)], false, GameTier::Compact, false);
        assert_eq!(s.active_desk_count(), 1);
        s.desks[0].phase = ActorPhase::Celebrate;

        // Agent gone from the map: the celebrate snap-completes, the room
        // empties, and the supervisor must read the emptied room.
        s.sync_from_snapshots(&[], false, GameTier::Compact, false);
        assert_eq!(s.active_desk_count(), 0);
        assert_eq!(s.supervisor, SupervisorPhase::Idle);

        // ...and it follows the live turn, not the snap-complete, when the
        // supervisor is actually busy.
        s.desks[0].child_session_id = Some("a".into());
        s.desks[0].phase = ActorPhase::Handoff;
        s.sync_from_snapshots(&[], true, GameTier::Compact, false);
        assert_eq!(s.supervisor, SupervisorPhase::Working);
    }

    #[test]
    fn stable_seat_map() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("x", true), snap("y", true)], false, GameTier::Normal, false);
        let ix = *s.seat_map.get("x").unwrap();
        let iy = *s.seat_map.get("y").unwrap();
        s.sync_from_snapshots(&[snap("y", true), snap("x", true)], false, GameTier::Normal, false);
        assert_eq!(s.seat_map.get("x"), Some(&ix));
        assert_eq!(s.seat_map.get("y"), Some(&iy));
    }

    #[test]
    fn attention_arms_once_per_failed_id() {
        let mut s = GameModeState::new();
        let mut fail = snap("bad", false);
        fail.failed = true;
        s.sync_from_snapshots(&[fail.clone()], false, GameTier::Comfort, false);
        let until = s.attention_until.expect("armed");
        s.sync_from_snapshots(&[fail], false, GameTier::Comfort, false);
        assert_eq!(s.attention_until, Some(until), "re-sync must not re-arm");
    }

    /// RC16 regression: the attention expiry must be consumed by exactly one
    /// sync, which re-derives the wall, *before* the room is allowed to park.
    ///
    /// The original PERF-1 + PERF-2 pairing level-tested `attention_until > now`
    /// in both `room_is_settled` and `needs_animation_tick`, so both flipped in
    /// the same instant: the sync that owed the wall a re-derive was skipped as
    /// "settled" while the loop parked, stranding NEEDS ATTENTION on screen for
    /// the rest of the session.
    #[test]
    fn expired_attention_is_consumed_before_the_room_parks() {
        let mut s = GameModeState::new();
        let mut fail = snap("bad", false);
        fail.failed = true;
        s.sync_from_snapshots(&[fail], false, GameTier::Comfort, false);
        assert_eq!(s.wall, WallMode::NeedsAttention, "failure arms the wall");
        assert!(s.attention_until.is_some(), "window armed");

        // Deadline passes. The room must NOT be considered settled or parkable
        // yet — no sync has re-derived the wall.
        s.attention_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(
            !s.room_is_settled(),
            "the owed wall re-derive must force the next sync to run"
        );
        assert!(
            s.needs_animation_tick(),
            "the loop must stay awake to deliver that sync"
        );

        // That one sync consumes the expiry and flips the wall back.
        s.sync_from_snapshots(&[], false, GameTier::Comfort, false);
        assert_eq!(s.attention_until, None, "expiry consumed exactly once");
        assert_ne!(
            s.wall,
            WallMode::NeedsAttention,
            "the wall must not strand on NEEDS ATTENTION"
        );

        // Only now may the room settle and the event loop park. (`sync_game_mode`
        // owns the redraw-dirty edge from the wall change; this layer owns the
        // settle/park predicates, so pin the sync bookkeeping it would have set.)
        assert!(s.room_is_settled(), "consumed window settles the room");
        s.last_sync_at = Some(Instant::now());
        s.take_redraw_dirty();
        assert!(!s.needs_animation_tick(), "and the loop may finally park");
    }

    /// P11: the armed set is now probed by `&str` and only allocates an owned
    /// id on the arming transition. The set semantics it carries must not
    /// drift — a distinct new failure arms, and an id that leaves the map is
    /// forgotten so its return arms again.
    #[test]
    fn attention_arms_per_distinct_failed_id() {
        let failed = |id: &str| {
            let mut a = snap(id, false);
            a.failed = true;
            a
        };
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[failed("bad")], false, GameTier::Comfort, false);
        let first = s.attention_until.expect("armed");
        assert_eq!(s.attention_armed_ids.len(), 1);

        // A second, distinct failure arms again.
        s.sync_from_snapshots(
            &[failed("bad"), failed("worse")],
            false,
            GameTier::Comfort,
            false,
        );
        let second = s.attention_until.expect("re-armed");
        assert!(second > first, "a new failed id must extend attention");
        assert_eq!(s.attention_armed_ids.len(), 2);

        // Both gone from the map: the armed ids go with them...
        s.sync_from_snapshots(&[], false, GameTier::Comfort, false);
        assert!(s.attention_armed_ids.is_empty());

        // ...so the same id failing again is a new arming transition.
        s.sync_from_snapshots(&[failed("bad")], false, GameTier::Comfort, false);
        assert!(s.attention_until.expect("re-armed") > second);
    }

    #[test]
    fn waiting_on_user_sets_wall() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[], false, GameTier::Comfort, true);
        assert_eq!(s.wall, super::super::wall::WallMode::WaitingOnYou);
    }

    #[test]
    fn update_hover_dirty_only_on_desk_change() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("a".into());
        s.desks[1].child_session_id = Some("b".into());
        s.last_desks[0] = ratatui::layout::Rect::new(10, 10, 8, 4);
        s.last_desks[1] = ratatui::layout::Rect::new(30, 10, 8, 4);
        assert!(s.update_hover(12, 11), "first desk hit dirties");
        assert!(!s.update_hover(13, 11), "same desk micro-move is clean");
        assert!(!s.update_hover(12, 12), "same desk cell move is clean");
        assert!(s.update_hover(32, 11), "other desk dirties");
        assert_eq!(s.hover, Some(HoverTarget::Desk(1)));
        assert!(s.update_hover(0, 0), "leaving desk dirties");
        assert_eq!(s.hover, None);
    }

    /// The Supervisor is a hover target of its own, and carries the same
    /// change-only throttle the desks do — `agent_view::input` returns
    /// `Unchanged` for every mouse move that does not flip the target.
    #[test]
    fn update_hover_selects_supervisor_with_the_same_throttle() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("a".into());
        s.last_desks[0] = ratatui::layout::Rect::new(10, 20, 8, 4);
        s.last_supervisor = ratatui::layout::Rect::new(40, 5, 12, 3);

        assert!(s.update_hover(44, 6), "entering the supervisor dirties");
        assert_eq!(s.hover, Some(HoverTarget::Supervisor));
        assert_eq!(s.focus_desk(), None, "supervisor is not a desk");
        assert_eq!(s.hover_screen, Some((44, 6)), "card anchors on entry");

        assert!(!s.update_hover(45, 7), "micro-move on the boss is clean");
        assert_eq!(s.hover_screen, Some((44, 6)), "anchor must not follow");

        assert!(s.update_hover(12, 21), "desk ← supervisor dirties");
        assert_eq!(s.hover, Some(HoverTarget::Desk(0)));

        assert!(s.update_hover(0, 0), "leaving dirties");
        assert_eq!(s.hover, None);
        assert_eq!(s.hover_screen, None);
    }

    /// Seats win over the boss wherever the rects overlap, so the pre-RC16
    /// desk-only behaviour is preserved exactly.
    #[test]
    fn overlapping_desk_wins_over_supervisor() {
        let mut s = GameModeState::new();
        s.desks[2].child_session_id = Some("c".into());
        s.last_desks[2] = ratatui::layout::Rect::new(10, 10, 8, 4);
        s.last_supervisor = ratatui::layout::Rect::new(8, 8, 20, 10);
        assert!(s.update_hover(12, 11));
        assert_eq!(s.hover, Some(HoverTarget::Desk(2)));
        // ...but the surrounding boss rect still answers outside the desk.
        assert!(s.update_hover(9, 9));
        assert_eq!(s.hover, Some(HoverTarget::Supervisor));
    }

    /// The MCP rack is the third hover target, and it exists only where the
    /// pixel office painted one: a zero-size `last_mcp_rack` — what
    /// `render_game_mode` publishes for the Unicode and Compact tiers — must
    /// never hit-test, at any coordinate including the origin.
    #[test]
    fn update_hover_selects_the_mcp_rack_only_when_one_was_painted() {
        let mut s = GameModeState::new();
        s.last_supervisor = ratatui::layout::Rect::new(40, 5, 12, 3);

        assert!(
            !s.update_hover(0, 0),
            "an unpainted rack must not be hoverable at the origin"
        );
        assert_eq!(s.hover, None);

        s.last_mcp_rack = ratatui::layout::Rect::new(90, 4, 10, 9);
        assert!(s.update_hover(94, 8), "entering the rack dirties");
        assert_eq!(s.hover, Some(HoverTarget::McpRack));
        assert_eq!(s.hover_screen, Some((94, 8)), "card anchors on entry");
        assert!(!s.update_hover(95, 9), "micro-move on the rack is clean");

        assert!(s.update_hover(44, 6), "supervisor ← rack dirties");
        assert_eq!(s.hover, Some(HoverTarget::Supervisor));
    }

    /// The rack card is an overlay like the Supervisor's: neither the hovered
    /// rack nor the live server rows behind it may reach the fingerprint.
    #[test]
    fn mcp_rack_hover_and_snapshot_stay_out_of_the_fingerprint() {
        use crate::views::mcps_modal::{McpServerDisplayStatus, McpStatusRow};

        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("t".into());
        s.desks[0].phase = ActorPhase::AtDeskThinking;
        let fp0 = s.visual_fingerprint(80, 24, Instant::now());

        s.last_mcp_rack = ratatui::layout::Rect::new(90, 4, 10, 9);
        assert!(s.update_hover(94, 8));
        assert_eq!(s.hover, Some(HoverTarget::McpRack));
        s.mcp_info = McpRackSnapshot {
            servers: vec![McpStatusRow {
                name: "github".into(),
                display_name: None,
                status: McpServerDisplayStatus::Unavailable,
                tool_count: 0,
                status_detail: Some("EOF while reading handshake".into()),
            }],
            init_connected: 3,
            init_total: 4,
            init_active: true,
            rows_gen: 7,
        };
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp0,
            "rack hover + tooltip snapshot must not dirty pixel fingerprint"
        );
    }

    /// Tab cycles **seats only** (documented on `keyboard_focus`): hovering the
    /// Supervisor and pressing Tab lands on a desk, and Tab focus keeps winning
    /// over the mouse until the pointer enters a target itself.
    #[test]
    fn tab_focus_stays_on_desks() {
        let mut s = GameModeState::new();
        s.desks[1].child_session_id = Some("b".into());
        s.last_desks[1] = ratatui::layout::Rect::new(10, 20, 8, 4);
        s.last_supervisor = ratatui::layout::Rect::new(40, 5, 12, 3);

        assert!(s.update_hover(44, 6));
        assert!(s.focus_next_desk(), "Tab must reach the only occupied desk");
        assert_eq!(s.focus_target(), Some(HoverTarget::Desk(1)));
        assert_eq!(
            s.hover,
            Some(HoverTarget::Supervisor),
            "Tab does not clear the mouse target, it outranks it"
        );

        // Mouse re-entering the boss takes the card back off Tab focus (leaving
        // and re-entering, since a move that does not flip the target is
        // swallowed by the throttle — as it always was for desks).
        assert!(s.update_hover(0, 0));
        assert_eq!(s.focus_target(), Some(HoverTarget::Desk(1)), "Tab holds");
        assert!(s.update_hover(45, 6));
        assert_eq!(s.keyboard_focus, None);
        assert_eq!(s.focus_target(), Some(HoverTarget::Supervisor));
    }

    #[test]
    fn fingerprint_stable_on_idle_tick_and_hover() {
        let mut s = GameModeState::new();
        // Thinking desk + idle supervisor → no tick frame sampling.
        s.desks[0].child_session_id = Some("t".into());
        s.desks[0].phase = ActorPhase::AtDeskThinking;
        s.desks[0].skin = 1;
        s.supervisor = SupervisorPhase::Idle;
        let fp0 = s.visual_fingerprint(80, 24, Instant::now());
        s.tick = s.tick.wrapping_add(40);
        s.hover = Some(HoverTarget::Desk(0));
        s.hover_screen = Some((10, 10));
        s.overflow_count = 3;
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp0,
            "idle/thinking + hover must not dirty pixel fingerprint"
        );

        // ...and neither does hovering the Supervisor, nor anything the
        // Supervisor card renders: the whole card is a buffer overlay painted
        // after the halfblock blit.
        s.last_supervisor = ratatui::layout::Rect::new(40, 5, 12, 3);
        assert!(s.update_hover(44, 6), "moved onto the boss");
        assert_eq!(s.hover, Some(HoverTarget::Supervisor));
        s.supervisor_info = SupervisorSnapshot {
            model: Some("Grok 4.5".to_string()),
            turn_elapsed: Some(Duration::from_secs(93)),
            context_used: 42_000,
            context_total: 256_000,
            context_pct: 16,
            waiting_on_user: true,
            branch: Some("rc16-game-mode".to_string()),
        };
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp0,
            "supervisor hover + tooltip snapshot must not dirty pixel fingerprint"
        );

        // RC16 §4 #7 relaxes this by exactly one input and no more: the slow
        // ambient step. The ~12 Hz tick bucket above is still frozen — 40 ticks
        // changed nothing — and only the ambient step moves the frame.
        s.ambient_step = s.ambient_step.wrapping_add(1);
        let fp_sip = s.visual_fingerprint(80, 24, Instant::now());
        assert_ne!(fp_sip, fp0, "the ambient step must reach the idle office");
        s.tick = s.tick.wrapping_add(40);
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp_sip,
            "…and within one ambient step the office must still be frozen"
        );

        // The wall clock is the other new input, at ≤6 buckets/hour.
        s.clock_hm = (s.clock_hm.0.wrapping_add(1) % 24, s.clock_hm.1);
        assert_ne!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp_sip,
            "the hour must move the hands (and the day/night tint)"
        );
    }

    /// The ambient step is the *only* relaxation, and a room whose composed art
    /// does not read it must not pay for it — a fully busy office already
    /// recomposes on every `tick / 4` bucket and gains nothing from a second
    /// clock ticking underneath it.
    #[test]
    fn busy_room_does_not_hash_the_ambient_step() {
        let mut s = GameModeState::new();
        s.supervisor = SupervisorPhase::Working;
        s.desks[0].child_session_id = Some("w".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        // Pin the clock so this cannot straddle a ten-minute bucket edge.
        s.clock_hm = (10, 3);
        let fp0 = s.visual_fingerprint(80, 24, Instant::now());
        s.ambient_step = s.ambient_step.wrapping_add(1);
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp0,
            "no composed sprite reads the ambient step here"
        );

        // ...but a single thinking desk brings it back, because that desk's
        // sprite is drawn from it.
        s.desks[1].child_session_id = Some("t".into());
        s.desks[1].phase = ActorPhase::AtDeskThinking;
        let fp1 = s.visual_fingerprint(80, 24, Instant::now());
        s.ambient_step = s.ambient_step.wrapping_add(1);
        assert_ne!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp1,
            "a thinking desk sips coffee even while the boss works"
        );
    }

    /// RC16 §4 #7 / #12: the ambient step is wall-clock gated, **not** derived
    /// from `tick`. A parked office only wakes every `AMBIENT_TICK_INTERVAL`, so
    /// a `tick / N` bucket would run at whatever rate the room happened to be
    /// ticking at. This pins both halves: ticks alone never advance it, and one
    /// elapsed period advances it exactly once.
    #[test]
    fn ambient_step_is_wall_clock_gated_not_tick_gated() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("t".into());
        s.desks[0].phase = ActorPhase::AtDeskThinking;
        let step0 = s.ambient_step;

        for _ in 0..64 {
            s.tick_anim(GameTier::Normal);
        }
        assert_eq!(
            s.ambient_step, step0,
            "64 back-to-back ticks are still inside one ambient period"
        );

        s.ambient_at = Instant::now() - AMBIENT_PERIOD;
        s.tick_anim(GameTier::Normal);
        assert_eq!(
            s.ambient_step,
            step0.wrapping_add(1),
            "an elapsed period advances the step exactly once"
        );
        s.tick_anim(GameTier::Normal);
        assert_eq!(
            s.ambient_step,
            step0.wrapping_add(1),
            "and the gate re-arms — one step per period, not per tick"
        );

        // The gate must be shorter than the loop cadence that wakes a parked
        // office, or jitter drops every other step (the RC16 BUG-2 shape).
        assert!(
            AMBIENT_PERIOD < crate::app::app_view::AMBIENT_TICK_INTERVAL,
            "ambient gate must not be able to miss its own wake"
        );
        // ...and the whole point is that this is *slow*: an idle office must
        // stay far under one recompose per second.
        assert!(
            AMBIENT_PERIOD >= Duration::from_millis(2500),
            "an idle office must not recompose faster than ~0.4 Hz"
        );
    }

    /// RC16 PERF-1: a synced, frozen room must not ask for **Slow** ticks — that
    /// is what keeps `AppView::tick_demand` off the ~12 Hz loop while the office
    /// is open. A fresh (never-synced) room always does.
    ///
    /// RC16 §4 #7 adds the second half of the contract: such a room is no longer
    /// entirely still (coffee sip, steam, wall clock), so it asks for the much
    /// slower `TickDemand::Ambient` instead — and only once the pixel office has
    /// actually painted, since none of that art exists anywhere else.
    #[test]
    fn needs_animation_tick_false_for_frozen_room() {
        let mut s = GameModeState::new();
        assert!(
            s.needs_animation_tick(),
            "never-synced room must sync at least once"
        );
        s.sync_from_snapshots(&[], false, GameTier::Comfort, false);
        s.last_sync_at = Some(Instant::now());
        s.take_redraw_dirty();
        assert!(
            !s.needs_animation_tick(),
            "empty room + idle supervisor must let the loop park"
        );

        // Never painted: nothing to animate, so not even ambient ticks.
        assert!(
            !s.needs_ambient_tick(),
            "an office that has not painted the pixel room must park outright"
        );
        s.last_pixel_painted = true;
        assert!(
            s.needs_ambient_tick(),
            "a painted, frozen pixel office animates — slowly"
        );
        assert!(
            !s.needs_animation_tick(),
            "…and must NOT be promoted back onto the ~12 Hz Slow tick for it"
        );

        // The Unicode fallback and Compact draw none of the ambient art.
        s.pixel_mode = false;
        assert!(
            !s.needs_ambient_tick(),
            "a Unicode office has no sip, no steam and no clock hands to move"
        );
        s.pixel_mode = true;
        s.open = true;
        s.toggle();
        assert!(!s.open);
        assert!(
            !s.needs_ambient_tick(),
            "closing the office must drop the ambient wake with the pixel buffers"
        );
    }

    /// Every input that can still change the office keeps the tick alive.
    #[test]
    fn needs_animation_tick_true_while_room_can_change() {
        let base = || {
            let mut s = GameModeState::new();
            s.sync_from_snapshots(&[], false, GameTier::Comfort, false);
            s.last_sync_at = Some(Instant::now());
            s.take_redraw_dirty();
            s
        };
        let mut occupied = base();
        occupied.desks[0].child_session_id = Some("a".into());
        assert!(occupied.needs_animation_tick(), "seated desk animates");

        let mut working = base();
        working.supervisor = SupervisorPhase::Working;
        assert!(
            working.needs_animation_tick(),
            "working supervisor animates"
        );

        let mut reviewing = base();
        reviewing.supervisor = SupervisorPhase::Reviewing;
        assert!(
            reviewing.needs_animation_tick(),
            "reviewing supervisor animates"
        );

        let mut queued = base();
        queued.handoff_queue.push_back(0);
        assert!(queued.needs_animation_tick(), "pending handoff animates");

        let mut overflow = base();
        overflow.door_queue.push_back("q".into());
        assert!(overflow.needs_animation_tick(), "door queue must drain");

        let mut attention = base();
        attention.attention_until = Some(Instant::now() + Duration::from_secs(5));
        assert!(
            attention.needs_animation_tick(),
            "armed attention window must expire on a tick"
        );
        // Past the deadline but not yet consumed: the room must stay awake long
        // enough for one sync to re-derive the wall, or NEEDS ATTENTION strands.
        attention.attention_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(
            attention.needs_animation_tick(),
            "an unconsumed expiry must still tick so the wall can flip back"
        );
        attention.attention_until = None;
        assert!(
            !attention.needs_animation_tick(),
            "once the expiry is consumed the room may park"
        );

        let mut dirty = base();
        dirty.mark_redraw_dirty();
        assert!(
            dirty.needs_animation_tick(),
            "pending redraw must be flushed"
        );
    }

    /// PERF-2: the allocation-free replacement for
    /// `to_ascii_lowercase().contains("think")` must match the same labels.
    #[test]
    fn activity_thinking_matches_case_insensitively() {
        assert!(activity_is_thinking("Thinking"));
        assert!(activity_is_thinking("still THINKing about it"));
        assert!(activity_is_thinking("think"));
        assert!(!activity_is_thinking("Running: cargo build"));
        assert!(!activity_is_thinking("thin"));
        assert!(!activity_is_thinking(""));
    }

    /// PERF-2: a settled room is one where re-running the sync with identical
    /// snapshots provably changes nothing — anything in flight is not settled.
    #[test]
    fn room_is_settled_only_without_work_in_flight() {
        let mut s = GameModeState::new();
        assert!(s.room_is_settled(), "empty room is settled");

        s.sync_from_snapshots(&[snap("a", true)], false, GameTier::Compact, false);
        assert!(
            s.room_is_settled(),
            "a seated desk that is only typing is settled"
        );

        s.desks[0].phase = ActorPhase::AtDeskThinking;
        assert!(s.room_is_settled(), "a thinking desk is settled");

        s.desks[0].phase = ActorPhase::WalkToBoss;
        assert!(!s.room_is_settled(), "a walk clears the desk between syncs");

        s.desks[0].phase = ActorPhase::AtDeskWorking;
        s.handoff_queue.push_back(0);
        assert!(!s.room_is_settled(), "a queued handoff still has to start");
        s.handoff_queue.clear();

        s.door_queue.push_back("q".into());
        assert!(!s.room_is_settled(), "overflow still has to be promoted");
        s.door_queue.clear();

        s.attention_until = Some(Instant::now() + Duration::from_secs(5));
        assert!(!s.room_is_settled(), "the wall flips back at expiry");
        s.attention_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(
            !s.room_is_settled(),
            "an expired-but-unconsumed window still owes the wall a re-derive"
        );
        s.attention_until = None;
        assert!(s.room_is_settled(), "a consumed window settles the room");
    }

    /// RC16 §4 #5: the MCP rack's LEDs answer to **real tool calls**, and only
    /// to those. A busy room with no tool traffic leaves the rack dark (that is
    /// the whole difference from the §3 step 2 placeholder), the armed burst is
    /// hashed so its edges recompose, and an idle tick inside the burst must
    /// still not blink anything — the idle-freeze invariant the coffee steam
    /// and the thinking bubble died to is not relaxed here.
    #[test]
    fn mcp_rack_lights_only_on_real_tool_calls() {
        use super::super::compose::rack_is_active;

        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("t".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        s.supervisor = SupervisorPhase::Working;
        assert!(
            !rack_is_active(&s, Instant::now()),
            "a busy room that has called no tools must leave the rack dark"
        );
        let fp0 = s.visual_fingerprint(80, 24, Instant::now());

        s.rack_active_until = Some(Instant::now() + RACK_BURST);
        assert!(
            rack_is_active(&s, Instant::now()),
            "an armed burst lights the rack"
        );
        let fp_lit = s.visual_fingerprint(80, 24, Instant::now());
        assert_ne!(fp_lit, fp0, "the dark→lit edge must recompose");

        s.rack_active_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(
            !rack_is_active(&s, Instant::now()),
            "an expired burst goes dark"
        );
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp0,
            "the lit→dark edge must recompose back to the idle art"
        );

        // A burst must never unfreeze a room that is otherwise frozen: the
        // rack is lit but *still*, exactly like every other sprite there.
        let mut frozen = GameModeState::new();
        frozen.desks[0].child_session_id = Some("t".into());
        frozen.desks[0].phase = ActorPhase::AtDeskThinking;
        frozen.rack_active_until = Some(Instant::now() + RACK_BURST);
        assert!(
            !frozen.pixel_needs_tick_frame(),
            "an armed burst must not unfreeze the sprite bucket"
        );
        let lit = frozen.visual_fingerprint(80, 24, Instant::now());
        frozen.tick = frozen.tick.wrapping_add(40);
        assert_eq!(
            frozen.visual_fingerprint(80, 24, Instant::now()),
            lit,
            "a pure tick inside a burst must not recompose"
        );
    }

    /// Fingerprint invariant 10: the two time-derived inputs must be read at a
    /// caller-supplied instant, not from their own `Instant::now()`.
    ///
    /// `visual_fingerprint` hashes the rack-burst bool and the success-wave
    /// bucket; compose reads the same two facts to draw the LEDs and the crest.
    /// While each sampled its own clock, a burst deadline or a 150 ms wave
    /// bucket falling between the two reads composed pixels one bucket ahead of
    /// the fingerprint they were then cached under — a stale frame at every
    /// burst edge, self-healing only on the next `ensure_pixel_frame`.
    /// `ensure_pixel_frame` now snapshots one `Instant` and threads it into
    /// both, which is only sound if every reader honours the argument.
    #[test]
    fn time_derived_fingerprint_inputs_follow_the_supplied_instant() {
        use super::super::compose::rack_is_active;

        let mut s = GameModeState::new();
        s.clock_hm = (10, 3);

        // Both deadlines sit just in the past, so the *wall clock* says dark
        // and expired while an instant from before them says lit and running.
        let deadline = Instant::now() - Duration::from_millis(10);
        let inside = deadline - Duration::from_millis(5);
        s.rack_active_until = Some(deadline);
        s.success_fx_until = Some(deadline);

        assert!(
            !s.rack_burst_active(Instant::now()),
            "wall clock: the burst has expired"
        );
        assert!(
            !rack_is_active(&s, Instant::now()),
            "...and compose agrees at the wall clock"
        );
        assert!(
            rack_is_active(&s, inside),
            "compose must answer for the instant it was handed, not `now`"
        );
        assert!(
            s.rack_burst_active(inside),
            "the fingerprint's reader must do the same"
        );
        assert!(
            s.success_wave_t(Instant::now()).is_none(),
            "wall clock: the wave is over"
        );
        assert!(
            s.success_wave_t(inside).is_some(),
            "the crest must answer for the instant it was handed"
        );

        // ...and the fingerprint moves with that argument, so a frame composed
        // at `inside` cannot be cached under a fingerprint taken at `now`.
        assert_ne!(
            s.visual_fingerprint(80, 24, inside),
            s.visual_fingerprint(80, 24, Instant::now()),
            "straddling both deadlines must change the fingerprint"
        );
        // Pure in its instant: repeated calls with the same argument agree even
        // though real time keeps moving underneath them.
        assert_eq!(
            s.visual_fingerprint(80, 24, inside),
            s.visual_fingerprint(80, 24, inside),
            "the fingerprint must not read a clock of its own"
        );
    }

    /// The wakeup contract: an armed burst holds the loop awake (so somebody
    /// observes the expiry), `tick_anim` consumes that expiry on the edge and
    /// repaints the darkened rack, and the room then parks again. A level test
    /// here — `> now` instead of `is_some()` — would let the loop park while
    /// the rack was still composed lit, which is exactly how `attention_until`
    /// stranded NEEDS ATTENTION on the wall.
    #[test]
    fn expired_rack_burst_is_consumed_and_lets_the_room_park() {
        let mut s = GameModeState::new();
        s.last_sync_at = Some(Instant::now());
        assert!(!s.needs_animation_tick(), "control: empty room parks");

        s.rack_active_until = Some(Instant::now() + RACK_BURST);
        assert!(s.needs_animation_tick(), "an armed burst holds the loop");

        // Deadline passed, nothing has consumed it yet: still awake.
        s.rack_active_until = Some(Instant::now() - Duration::from_millis(1));
        assert!(
            s.needs_animation_tick(),
            "an expired-but-unconsumed burst still owes the rack a repaint"
        );

        s.take_redraw_dirty();
        s.tick_anim(GameTier::Comfort);
        assert!(
            s.rack_active_until.is_none(),
            "tick_anim must consume the expiry"
        );
        assert!(
            s.take_redraw_dirty(),
            "the darkened rack must be repainted once"
        );
        assert!(
            !s.needs_animation_tick(),
            "a consumed burst must let the room park again (PERF-1)"
        );
    }

    /// A tool-call increment on a seated desk arms the burst; a *level* (the
    /// same count re-reported every sync) does not, or a subagent that called
    /// one tool would hold the LEDs on for its whole life.
    #[test]
    fn tool_call_increment_arms_the_rack_burst() {
        let mut s = GameModeState::new();
        let mut a = snap("a", true);
        a.tool_calls = 0;
        s.sync_from_snapshots(&[a.clone()], false, GameTier::Comfort, false);
        assert!(
            s.rack_active_until.is_none(),
            "seating alone must not arm the rack"
        );

        a.tool_calls = 1;
        s.sync_from_snapshots(&[a.clone()], false, GameTier::Comfort, false);
        assert!(
            s.rack_burst_active(Instant::now()),
            "a tool call must light the rack"
        );

        // Same count again: the arm is an edge, so let the window lapse and
        // prove an unchanged counter does not re-arm it.
        s.rack_active_until = Some(Instant::now() - Duration::from_millis(1));
        s.sync_from_snapshots(&[a], false, GameTier::Comfort, false);
        assert!(
            !s.rack_burst_active(Instant::now()),
            "an unchanged tool count must not re-arm the burst"
        );
    }

    /// RC16 §4 #8: the wave is armed on the **edge** into WorkFinished and
    /// exactly once per success event. `had_success` is sticky, so a level test
    /// would re-arm it on every sync until the session ends — a room that can
    /// never park, which is the failure this whole animation has to avoid.
    #[test]
    fn success_wave_is_armed_once_per_work_finished() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("a", true)], false, GameTier::Compact, false);
        assert_eq!(s.wall, WallMode::Working);
        assert!(
            s.success_fx_until.is_none(),
            "a running room must not sweep"
        );

        // The desk finishes and the compact tier retires the celebrate on the
        // next sync, so the wall lands on WorkFinished.
        s.sync_from_snapshots(&[snap("a", false)], false, GameTier::Compact, false);
        s.sync_from_snapshots(&[], false, GameTier::Compact, false);
        assert_eq!(s.wall, WallMode::WorkFinished);
        let armed = s.success_fx_until.expect("WORK FINISHED must sweep");
        assert!(s.success_wave_t(Instant::now()).is_some(), "and it must be live");

        // The wall stays on WorkFinished for the rest of the session: further
        // syncs must not push the deadline out.
        for _ in 0..4 {
            s.sync_from_snapshots(&[], false, GameTier::Compact, false);
        }
        assert_eq!(
            s.success_fx_until,
            Some(armed),
            "a sync on a wall that never left WorkFinished must not re-arm"
        );

        // A *new* success is a new event, and does get its own wave.
        s.success_fx_until = None;
        s.sync_from_snapshots(&[snap("b", true)], false, GameTier::Compact, false);
        assert_eq!(s.wall, WallMode::Working);
        s.sync_from_snapshots(&[snap("b", false)], false, GameTier::Compact, false);
        s.sync_from_snapshots(&[], false, GameTier::Compact, false);
        assert!(
            s.success_fx_until.is_some(),
            "the next WORK FINISHED must sweep again"
        );
    }

    /// THE TRAP (RC16 §4 #8): the wave fires exactly as the room goes idle, so
    /// if its bucket kept reaching the fingerprint after expiry the office would
    /// recompose forever and the loop would never park. This pins all three
    /// halves: the sweep moves the fingerprint while it runs, the fingerprint
    /// returns to its **pre-wave value** at expiry, and `tick_anim` consumes the
    /// deadline on the edge so `needs_animation_tick` goes false.
    #[test]
    fn expired_success_wave_is_consumed_and_lets_the_room_park() {
        let mut s = GameModeState::new();
        s.last_sync_at = Some(Instant::now());
        s.clock_hm = (10, 3);
        s.take_redraw_dirty();
        let quiet = s.visual_fingerprint(80, 24, Instant::now());
        assert!(!s.needs_animation_tick(), "control: an empty room parks");

        s.success_fx_until = Some(Instant::now() + SUCCESS_WAVE);
        assert!(s.needs_animation_tick(), "an armed wave holds the loop");
        let early = s.visual_fingerprint(80, 24, Instant::now());
        assert_ne!(early, quiet, "the sweep must recompose");
        // Half-way through: a different bucket, so a different frame.
        s.success_fx_until = Some(Instant::now() + SUCCESS_WAVE / 2);
        assert_ne!(
            s.visual_fingerprint(80, 24, Instant::now()),
            early,
            "the crest must move with the bucket"
        );

        // Deadline passed, nothing has consumed it yet.
        s.success_fx_until = Some(Instant::now() - Duration::from_millis(1));
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            quiet,
            "an elapsed wave must fingerprint exactly like the pre-wave room"
        );
        assert!(
            s.needs_animation_tick(),
            "…but the room still owes it one repaint"
        );

        s.take_redraw_dirty();
        s.tick_anim(GameTier::Normal);
        assert!(
            s.success_fx_until.is_none(),
            "tick_anim must consume the expiry"
        );
        assert!(s.take_redraw_dirty(), "the un-lit room must be repainted once");
        assert!(
            !s.needs_animation_tick(),
            "and then the room must park again (PERF-1)"
        );
        assert_eq!(s.visual_fingerprint(80, 24, Instant::now()), quiet);
    }

    /// The recompose budget, stated as a test: one full sweep costs
    /// [`SUCCESS_WAVE_BUCKETS`] repaints and no more, however many Slow ticks
    /// happen to land inside its window.
    #[test]
    fn success_wave_repaint_budget_is_one_per_bucket() {
        let mut s = GameModeState::new();
        s.clock_hm = (10, 3);
        s.success_fx_until = Some(Instant::now() + SUCCESS_WAVE);
        s.take_redraw_dirty();

        // Replay the whole window at the ~12 Hz Slow cadence without sleeping:
        // `tick_anim` marks dirty exactly when the bucket at *this* tick differs
        // from the bucket at the previous one, so stepping an `Instant` forward
        // one interval at a time counts the repaints it would have made.
        let start = Instant::now();
        let at = |step: u64| start + Duration::from_millis(step * 83);
        let mut marks = 0;
        let mut buckets = std::collections::HashSet::new();
        for step in 0..=SUCCESS_WAVE.as_millis() as u64 / 83 {
            buckets.insert(s.success_wave_bucket_at(at(step)));
            if s.success_wave_bucket_at(at(step)) != s.success_wave_bucket_at(at(step + 1)) {
                marks += 1;
            }
        }
        // `assert_eq`, not `<=`: a one-sided bound also passes at zero marks,
        // i.e. on a wave that never repaints at all. The budget is a floor as
        // much as a ceiling — nine bucket advances plus the one repaint that
        // clears the crest at expiry.
        assert_eq!(
            marks, SUCCESS_WAVE_BUCKETS,
            "{marks} repaints for {SUCCESS_WAVE_BUCKETS} composable frames"
        );
        assert_eq!(
            buckets.len() as u64,
            SUCCESS_WAVE_BUCKETS,
            "every bucket must be reachable at the Slow cadence, got {buckets:?}"
        );
    }

    /// RC16 §4 #9: the typing cadence follows real token throughput, measured
    /// against **wall time** (the sync is throttled and can skip, so a per-sync
    /// delta would read a different rate depending on how busy the app was).
    #[test]
    fn token_throughput_buckets_the_typing_cadence() {
        // One sample of `tokens` gained over `secs` seconds, from level `cur`.
        let sample = |cur: BusyLevel, gained: u64, secs: f32| {
            let mut d = DeskSlot {
                busy: cur,
                prev_tokens: 1000,
                tokens_at: Instant::now() - Duration::from_secs_f32(secs),
                ..DeskSlot::default()
            };
            d.sample_throughput(1000 + gained);
            d.busy
        };

        assert_eq!(sample(BusyLevel::Normal, 200, 2.0), BusyLevel::Hot);
        assert_eq!(sample(BusyLevel::Normal, 40, 2.0), BusyLevel::Normal);
        assert_eq!(sample(BusyLevel::Normal, 0, 2.0), BusyLevel::Calm);
        assert_eq!(sample(BusyLevel::Hot, 0, 2.0), BusyLevel::Calm);
        assert_eq!(sample(BusyLevel::Calm, 200, 2.0), BusyLevel::Hot);

        // Hysteresis: a rate sitting between the enter and exit gates must hold
        // whatever level the desk is already in, or the cadence flickers.
        let mid = ((BUSY_HOT_ENTER + BUSY_HOT_EXIT) / 2.0 * 2.0) as u64;
        assert_eq!(sample(BusyLevel::Hot, mid, 2.0), BusyLevel::Hot);
        assert_eq!(sample(BusyLevel::Normal, mid, 2.0), BusyLevel::Normal);
        let low = ((BUSY_NORMAL_ENTER + BUSY_NORMAL_EXIT) / 2.0 * 2.0) as u64;
        assert_eq!(sample(BusyLevel::Normal, low, 2.0), BusyLevel::Normal);
        assert_eq!(sample(BusyLevel::Calm, low, 2.0), BusyLevel::Calm);

        // Below the sample period nothing is measured at all — the window has
        // to be long enough for the rate to mean something.
        let mut fresh = DeskSlot {
            busy: BusyLevel::Calm,
            prev_tokens: 0,
            tokens_at: Instant::now(),
            ..DeskSlot::default()
        };
        fresh.sample_throughput(100_000);
        assert_eq!(fresh.busy, BusyLevel::Calm, "one tick is not a rate");
        assert_eq!(fresh.prev_tokens, 0, "…and the window must not restart");
    }

    /// End to end through the sync path: a streaming desk goes Hot and a silent
    /// one decays back, whatever cadence the syncs themselves arrive at.
    #[test]
    fn sync_tracks_throughput_across_a_throttled_sync_rate() {
        let mut s = GameModeState::new();
        let mut a = snap("a", true);
        a.tokens = 0;
        s.sync_from_snapshots(&[a.clone()], false, GameTier::Comfort, false);
        assert_eq!(s.desks[0].busy, BusyLevel::Normal, "a new seat starts flat");

        // Several syncs inside one sample window change nothing...
        a.tokens = 400;
        for _ in 0..5 {
            s.sync_from_snapshots(&[a.clone()], false, GameTier::Comfort, false);
        }
        assert_eq!(s.desks[0].busy, BusyLevel::Normal, "the window is still open");

        // ...then one sync after the window elapses reads the whole delta.
        s.desks[0].tokens_at = Instant::now() - Duration::from_secs(2);
        a.tokens = 800;
        s.sync_from_snapshots(&[a.clone()], false, GameTier::Comfort, false);
        assert_eq!(s.desks[0].busy, BusyLevel::Hot, "800 tokens in 2 s is hot");

        // A desk that stops streaming decays, even though its token count is
        // now identical on every sync.
        s.desks[0].tokens_at = Instant::now() - Duration::from_secs(2);
        s.sync_from_snapshots(&[a], false, GameTier::Comfort, false);
        assert_eq!(s.desks[0].busy, BusyLevel::Calm, "silence must cool down");
    }

    /// The cost of §4 #9, pinned: a Hot desk moves the fingerprint (and the
    /// repaints) at `tick / 2` instead of `tick / 4`, a Calm one at `tick / 8`,
    /// and neither can lower the floor below the global bucket the walkers, the
    /// fail beat and the rack all ride.
    #[test]
    fn busy_level_sets_the_frame_bucket_and_the_repaint_rate() {
        let seated = |busy: BusyLevel| {
            let mut s = GameModeState::new();
            s.desks[0].child_session_id = Some("w".into());
            s.desks[0].phase = ActorPhase::AtDeskWorking;
            s.desks[0].busy = busy;
            s.clock_hm = (10, 3);
            s
        };

        assert_eq!(seated(BusyLevel::Hot).frame_bucket_divisor(), 2);
        assert_eq!(seated(BusyLevel::Normal).frame_bucket_divisor(), 4);
        assert_eq!(seated(BusyLevel::Calm).frame_bucket_divisor(), 4, "floor");
        assert_eq!(
            GameModeState::new().frame_bucket_divisor(),
            4,
            "an empty room keeps the global bucket"
        );

        // A hot desk recomposes twice as often; a normal one is unchanged.
        let mut hot = seated(BusyLevel::Hot);
        let fp0 = hot.visual_fingerprint(80, 24, Instant::now());
        hot.tick = 2;
        assert_ne!(hot.visual_fingerprint(80, 24, Instant::now()), fp0, "hot moves at tick/2");
        let mut normal = seated(BusyLevel::Normal);
        let fp1 = normal.visual_fingerprint(80, 24, Instant::now());
        normal.tick = 2;
        assert_eq!(normal.visual_fingerprint(80, 24, Instant::now()), fp1, "normal at tick/4");

        // The level itself is an input: same tick, different cadence bucket.
        assert_ne!(
            seated(BusyLevel::Hot).visual_fingerprint(80, 24, Instant::now()),
            seated(BusyLevel::Normal).visual_fingerprint(80, 24, Instant::now()),
            "the composed keyboard differs, so the level must be hashed"
        );

        // ...but only for the phase that composes a keyboard: a thinking desk's
        // throughput must never dirty a frozen room (RC13 idle freeze).
        let mut think = seated(BusyLevel::Normal);
        think.desks[0].phase = ActorPhase::AtDeskThinking;
        let idle = think.visual_fingerprint(80, 24, Instant::now());
        think.desks[0].busy = BusyLevel::Hot;
        assert_eq!(
            think.visual_fingerprint(80, 24, Instant::now()),
            idle,
            "a thinking desk composes no keyboard"
        );
        assert_eq!(think.frame_bucket_divisor(), 4);

        // Repaint rate follows the same divisor, or composed frames never reach
        // the screen.
        assert_eq!(
            dirty_marks_over(&mut seated(BusyLevel::Hot), GameTier::Normal, 8),
            4,
            "a hot desk repaints on tick/2 edges"
        );
        assert_eq!(
            dirty_marks_over(&mut seated(BusyLevel::Normal), GameTier::Normal, 8),
            2,
            "…and a normal one is exactly as cheap as before RC16 §4 #9"
        );
    }

    #[test]
    fn fingerprint_moves_with_working_tick_bucket() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("w".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        s.supervisor = SupervisorPhase::Waiting;
        let fp0 = s.visual_fingerprint(80, 24, Instant::now());
        s.tick = 3; // still same ÷4 bucket as 0
        assert_eq!(s.visual_fingerprint(80, 24, Instant::now()), fp0);
        s.tick = 4; // next bucket
        assert_ne!(s.visual_fingerprint(80, 24, Instant::now()), fp0);
    }

    /// Only phases whose composited output moves with `anim_t` may hash it.
    /// Seated desks animate off the `tick/4` bucket instead, so their `anim_t`
    /// (which `tick_anim` spins every tick) must never force a recompose.
    /// Handoff is in the moving set again: the walker is still pinned on the
    /// rug, but the papers FX arcs off `anim_t` (see `phase_anim_t_is_visible`).
    /// Celebrate joined it for the same reason — its pose and confetti are both
    /// too fast for the `tick / 4` bucket (RC16 §4 #2).
    #[test]
    fn only_moving_walk_phases_hash_anim_t() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("h".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        s.desks[0].anim_t = 0.0;
        let fp0 = s.visual_fingerprint(80, 24, Instant::now());
        s.desks[0].anim_t = 1.0;
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp0,
            "a seated desk's anim_t must not dirty the pixel fingerprint"
        );

        // ...and neither does a fail beat, which rides the tick/4 bucket: its
        // 900 ms span already covers ~3 bucket edges.
        s.desks[0].phase = ActorPhase::FailBeat;
        s.desks[0].anim_t = 0.0;
        let fp_fail = s.visual_fingerprint(80, 24, Instant::now());
        s.desks[0].anim_t = 0.5;
        assert_eq!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp_fail,
            "the fail beat animates off the frame bucket, not anim_t"
        );

        for phase in [
            ActorPhase::SpawnWalk,
            ActorPhase::WalkToBoss,
            ActorPhase::ExitDoor,
            ActorPhase::Handoff,
            ActorPhase::Celebrate,
        ] {
            s.desks[0].phase = phase;
            s.desks[0].anim_t = 0.0;
            let fp = s.visual_fingerprint(80, 24, Instant::now());
            s.desks[0].anim_t = 0.5;
            assert_ne!(
                s.visual_fingerprint(80, 24, Instant::now()),
                fp,
                "{phase:?} moves with anim_t and must recompose"
            );
        }
    }

    /// Count redraw marks over `ticks` animation steps.
    fn dirty_marks_over(s: &mut GameModeState, tier: GameTier, ticks: u64) -> u64 {
        s.take_redraw_dirty();
        let mut n = 0;
        for _ in 0..ticks {
            s.tick_anim(tier);
            if s.take_redraw_dirty() {
                n += 1;
            }
        }
        n
    }

    /// BUG-4: a thinking-only room freezes the pixel office on purpose, but the
    /// Compact / Unicode tiers paint a live per-desk HUD (elapsed, tokens,
    /// marquee) that must still be refreshed — ~1 Hz, not never.
    #[test]
    fn non_pixel_tier_refreshes_hud_for_thinking_desks() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("t".into());
        s.desks[0].phase = ActorPhase::AtDeskThinking;

        assert_eq!(
            dirty_marks_over(&mut s, GameTier::Compact, HUD_REFRESH_TICKS * 2),
            2,
            "compact HUD must refresh once per HUD_REFRESH_TICKS, no faster"
        );

        // Unicode office (pixel path off) paints the same HUD.
        let mut u = GameModeState::new();
        u.pixel_mode = false;
        u.desks[0].child_session_id = Some("t".into());
        u.desks[0].phase = ActorPhase::AtDeskThinking;
        assert_eq!(
            dirty_marks_over(&mut u, GameTier::Normal, HUD_REFRESH_TICKS),
            1,
            "unicode fallback HUD must refresh too"
        );

        // ...and the pixel office keeps its idle freeze (RC13 invariant), now
        // bounded by the ambient period rather than by "forever": 24 ticks are
        // ~2 s of Slow ticks, i.e. inside one AMBIENT_PERIOD.
        let mut p = GameModeState::new();
        p.desks[0].child_session_id = Some("t".into());
        p.desks[0].phase = ActorPhase::AtDeskThinking;
        assert_eq!(
            dirty_marks_over(&mut p, GameTier::Normal, HUD_REFRESH_TICKS * 2),
            0,
            "thinking-only pixel office must not repaint within an ambient period"
        );

        // On the ambient edge it repaints exactly once — the coffee sip — and
        // then goes straight back to still.
        p.ambient_at = Instant::now() - AMBIENT_PERIOD;
        assert_eq!(
            dirty_marks_over(&mut p, GameTier::Normal, HUD_REFRESH_TICKS * 2),
            1,
            "the ambient step must buy exactly one repaint, not a cadence change"
        );
    }

    /// RC16 §4 #7 wakeup budget, stated as a test: an idle pixel office spends
    /// one repaint per [`AMBIENT_PERIOD`] and nothing else. At the
    /// `AMBIENT_TICK_INTERVAL` the loop actually wakes it at, that is ~0.33
    /// repaints/sec — versus the ~12/sec an open office cost before RC16 PERF-1.
    #[test]
    fn idle_office_repaint_budget_is_one_per_ambient_period() {
        let mut s = GameModeState::new();
        s.supervisor = SupervisorPhase::Idle;
        s.last_pixel_painted = true;
        // Pin the clock so a ten-minute boundary cannot add a repaint.
        s.clock_hm = (10, 3);
        s.take_redraw_dirty();

        let mut marks = 0;
        for _ in 0..5 {
            // One simulated ambient wake: the loop parked in between, so exactly
            // one tick_anim runs per period.
            s.ambient_at = Instant::now() - AMBIENT_PERIOD;
            s.clock_hm = (10, 3);
            s.tick_anim(GameTier::Normal);
            if s.take_redraw_dirty() {
                marks += 1;
            }
        }
        assert_eq!(marks, 5, "each ambient wake repaints the room exactly once");
        assert_eq!(
            s.ambient_step, 5,
            "…and advances the sip / steam exactly once"
        );
    }

    /// PERF-6: seated pixel desks only change on the `tick / 4` sprite bucket, so
    /// 3 of every 4 repaints re-blit an identical office. Walks and every
    /// Unicode-drawn office keep their per-tick cadence.
    #[test]
    fn pixel_steady_typing_marks_dirty_on_bucket_edges_only() {
        let seated = |pixel_mode: bool| {
            let mut s = GameModeState::new();
            s.pixel_mode = pixel_mode;
            s.desks[0].child_session_id = Some("w".into());
            s.desks[0].phase = ActorPhase::AtDeskWorking;
            s
        };

        assert_eq!(
            dirty_marks_over(&mut seated(true), GameTier::Normal, 8),
            2,
            "pixel office typing must repaint on tick/4 edges only"
        );
        assert_eq!(
            dirty_marks_over(&mut seated(false), GameTier::Normal, 8),
            8,
            "unicode office animates at tick%2/%4/%6 — must stay per-tick"
        );
        assert_eq!(
            dirty_marks_over(&mut seated(true), GameTier::Compact, 8),
            8,
            "compact cards animate + scroll the marquee — must stay per-tick"
        );

        let mut walking = seated(true);
        walking.desks[0].phase = ActorPhase::WalkToBoss;
        assert_eq!(
            dirty_marks_over(&mut walking, GameTier::Normal, 8),
            8,
            "a walk moves every tick and must repaint every tick"
        );
    }

    /// RC16 §4 #11's entire claim, in one test: the floor robot advances **only**
    /// on a `tick / 4` bucket edge in a room that is already animating. That is
    /// what makes it free — no `needs_animation_tick` change (so no wakeups), and
    /// no dirty mark of its own (the bucket edge it rides already marked one).
    #[test]
    fn roomba_advances_only_while_the_room_already_animates() {
        // Frozen: nobody home.
        let mut empty = GameModeState::new();
        assert_eq!(dirty_marks_over(&mut empty, GameTier::Normal, 24), 0);
        assert_eq!(empty.roomba_step, 0, "an empty office parks the robot");

        // Frozen: thinking-only. The RC13 invariant — and the robot with it.
        let mut thinking = GameModeState::new();
        thinking.desks[0].child_session_id = Some("t".into());
        thinking.desks[0].phase = ActorPhase::AtDeskThinking;
        assert!(!thinking.roomba_is_moving());
        assert_eq!(dirty_marks_over(&mut thinking, GameTier::Normal, 24), 0);
        assert_eq!(
            thinking.roomba_step, 0,
            "a thinking-only room must not move the robot"
        );

        // Animating: one step per bucket, and not one extra repaint for it.
        let mut working = GameModeState::new();
        working.desks[0].child_session_id = Some("w".into());
        working.desks[0].phase = ActorPhase::AtDeskWorking;
        assert!(working.roomba_is_moving());
        assert_eq!(
            dirty_marks_over(&mut working, GameTier::Normal, 8),
            2,
            "the robot must not add repaints to the PERF-6 bucket cadence"
        );
        assert_eq!(working.roomba_step, 2, "one patrol step per tick/4 bucket");

        // ...and it parks where it stands the moment the room freezes, rather
        // than driving itself home (which would mean animating a parked office).
        working.desks[0].child_session_id = None;
        working.desks[0].phase = ActorPhase::AtDeskThinking;
        assert_eq!(dirty_marks_over(&mut working, GameTier::Normal, 24), 0);
        assert_eq!(working.roomba_step, 2, "a frozen room leaves the robot put");
    }

    /// The other half of the contract: the position must be *visible* to the
    /// fingerprint (or the patrol would compose and never paint) and must be
    /// constant in a frozen room (or the RC13 freeze would be gone).
    #[test]
    fn roomba_step_is_fingerprinted_but_never_moves_an_idle_room() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("w".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        let fp = s.visual_fingerprint(80, 24, Instant::now());
        s.roomba_step += 1;
        assert_ne!(
            s.visual_fingerprint(80, 24, Instant::now()),
            fp,
            "a patrol step must recompose or the robot would never move"
        );

        // A hot desk samples `tick / 2`, so its frame edges are a superset of the
        // `tick / 4` edges the robot rides — it can never step on a tick the
        // fingerprint did not already move on.
        for level in [BusyLevel::Calm, BusyLevel::Normal, BusyLevel::Hot] {
            assert_eq!(
                4 % level.frame_divisor().min(4),
                0,
                "{level:?}: the frame bucket must divide the robot's tick/4 step"
            );
        }

        let mut idle = GameModeState::new();
        idle.desks[0].child_session_id = Some("t".into());
        idle.desks[0].phase = ActorPhase::AtDeskThinking;
        let fp = idle.visual_fingerprint(80, 24, Instant::now());
        for _ in 0..24 {
            idle.tick_anim(GameTier::Normal);
        }
        assert_eq!(
            idle.visual_fingerprint(80, 24, Instant::now()),
            fp,
            "the robot must not break the thinking-room freeze"
        );
    }

    /// PERF-7: a hidden office holds no image memory; reopening rebuilds it.
    #[test]
    fn toggle_closed_releases_image_memory_and_reopen_repaints() {
        let mut s = GameModeState::new();
        s.toggle();
        assert!(s.open);
        assert!(s.ensure_pixel_frame(40, 14), "office must paint when open");
        assert!(s.pixel_bg_full.is_some());
        assert!(s.pixel_bg_scaled.is_some());
        assert!(s.pixel_paint.is_some());
        assert!(s.pixel_halfblock.is_some());
        assert!(s.pixel_compose_scratch.is_some());

        s.toggle();
        assert!(!s.open);
        assert!(s.pixel_bg_full.is_none(), "full-res BG must be released");
        assert!(s.pixel_bg_scaled.is_none());
        assert!(s.pixel_paint.is_none());
        assert!(s.pixel_halfblock.is_none());
        assert!(s.pixel_compose_scratch.is_none());
        assert_eq!(s.pixel_cell_w, 0);
        assert_eq!(s.pixel_cell_h, 0);
        assert_eq!(s.pixel_bg_scale, 0);
        assert_eq!(
            s.pixel_frame_fp, 0,
            "a stale fingerprint must not HIT against dropped buffers"
        );

        s.toggle();
        assert!(
            s.ensure_pixel_frame(40, 14),
            "reopen must rebuild the frame"
        );
        assert_eq!(
            s.pixel_paint.as_ref().expect("paint buffer").dimensions(),
            (40, 28)
        );
        let cache = s.pixel_halfblock.as_ref().expect("cell cache");
        assert_eq!((cache.cell_w, cache.cell_h), (40, 14));
        assert_eq!(cache.packed.len(), 40 * 14);
    }

    /// PERF-5: repeated fingerprint misses at one terminal size must not
    /// reallocate the paint buffer or the packed cell cache.
    #[test]
    fn repeated_fingerprint_misses_reuse_paint_buffers() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("w".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        assert!(s.ensure_pixel_frame(40, 14));
        let paint_ptr = s.pixel_paint.as_ref().expect("paint").as_raw().as_ptr();
        let cache0 = s.pixel_halfblock.as_ref().expect("cache");
        let packed_ptr = cache0.packed.as_ptr();
        let packed_cap = cache0.packed.capacity();
        let mut fp = s.pixel_frame_fp;

        for step in 1..4u64 {
            s.tick = step * 4; // next sprite frame bucket => guaranteed miss
            assert!(s.ensure_pixel_frame(40, 14));
            assert_ne!(s.pixel_frame_fp, fp, "step {step} must be a miss");
            fp = s.pixel_frame_fp;
            assert_eq!(
                s.pixel_paint.as_ref().expect("paint").as_raw().as_ptr(),
                paint_ptr,
                "paint buffer reallocated on fingerprint miss"
            );
            let cache = s.pixel_halfblock.as_ref().expect("cache");
            assert_eq!(cache.packed.as_ptr(), packed_ptr, "cell cache reallocated");
            assert_eq!(cache.packed.capacity(), packed_cap);
        }
    }

    /// PERF-5: a stage size change still yields a correct, correctly sized buffer.
    #[test]
    fn stage_resize_rebuilds_correctly_sized_paint_buffers() {
        let mut s = GameModeState::new();
        assert!(s.ensure_pixel_frame(40, 14));
        assert_eq!(
            s.pixel_paint.as_ref().expect("paint").dimensions(),
            (40, 28)
        );

        assert!(s.ensure_pixel_frame(30, 10));
        assert_eq!(
            s.pixel_paint.as_ref().expect("paint").dimensions(),
            (30, 20)
        );
        let cache = s.pixel_halfblock.as_ref().expect("cache");
        assert_eq!((cache.cell_w, cache.cell_h), (30, 10));
        assert_eq!(cache.packed.len(), 30 * 10);
        assert!(
            cache.packed.iter().any(|p| p.iter().any(|&c| c > 0)),
            "resampled office must not be blank"
        );
    }

    /// PERF-5: the in-place resample must pick the same pixels `imageops` did.
    #[test]
    fn resample_nearest_into_matches_imageops() {
        let mut src = RgbaImage::new(12, 8);
        for (x, y, p) in src.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 20) as u8, (y * 30) as u8, (x + y) as u8, 255]);
        }
        let expect = image::imageops::resize(&src, 4, 4, image::imageops::FilterType::Nearest);
        let mut got = RgbaImage::new(4, 4);
        resample_nearest_into(&mut got, &src);
        assert_eq!(got.as_raw(), expect.as_raw());
    }
}
