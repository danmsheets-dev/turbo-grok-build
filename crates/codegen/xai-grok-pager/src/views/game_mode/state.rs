//! Game Mode runtime state: desk slots, handoff queue, wall flags.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use image::RgbaImage;

use super::layout::GameTier;
use super::wall::WallMode;

pub const DESK_COUNT: usize = 6;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorPhase {
    Idle,
    Working,
    Reviewing,
    Waiting,
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
    /// Desk under mouse cursor (popup placement), if any.
    pub hover_desk: Option<usize>,
    /// Keyboard focus desk (Tab cycle); independent of mouse hover (dual-audit).
    pub keyboard_focus: Option<usize>,
    /// Last mouse position in screen coords (for popup placement).
    pub hover_screen: Option<(u16, u16)>,
    /// Last painted stage area (for hover hit-testing).
    pub last_stage: Option<ratatui::layout::Rect>,
    /// Last desk rects from layout (for hover hit-testing).
    pub last_desks: [ratatui::layout::Rect; 6],
    /// Failed child IDs already used to arm `attention_until` (transition-only).
    attention_armed_ids: std::collections::HashSet<String>,
    /// Set when UI needs a redraw (tick/sync/hover); consumed by AppView::tick.
    redraw_dirty: bool,
    /// Last full snapshot+sync time (paint skips if recent — single sync owner).
    pub(crate) last_sync_at: Option<Instant>,
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
            next_skin: 0,
            pixel_bg_full: None,
            pixel_bg_scaled: None,
            pixel_cell_w: 0,
            pixel_cell_h: 0,
            pixel_bg_scale: 0,
            pixel_frame: None,
            pixel_paint: None,
            pixel_halfblock: None,
            pixel_compose_scratch: None,
            pixel_frame_fp: 0,
            pixel_mode: true,
            hover_desk: None,
            keyboard_focus: None,
            hover_screen: None,
            last_stage: None,
            last_desks: [ratatui::layout::Rect::default(); 6],
            attention_armed_ids: std::collections::HashSet::new(),
            redraw_dirty: false,
            last_sync_at: None,
        }
    }

    /// Desk shown in popup / focus ring: keyboard focus wins over mouse hover.
    pub fn focus_desk(&self) -> Option<usize> {
        self.keyboard_focus.or(self.hover_desk)
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
    /// Returns `true` only when the **hovered desk** changes (not every mouse
    /// cell). Popup anchors to entry cell; micro-moves on the same desk do not
    /// repaint (triple-scan hover throttle).
    pub fn update_hover(&mut self, col: u16, row: u16) -> bool {
        let prev_desk = self.hover_desk;
        let new_desk = self.last_desks.iter().enumerate().find_map(|(i, r)| {
            if r.width == 0 || r.height == 0 {
                return None;
            }
            if col >= r.x
                && col < r.x.saturating_add(r.width)
                && row >= r.y
                && row < r.y.saturating_add(r.height)
                && self.desks[i].is_occupied()
            {
                Some(i)
            } else {
                None
            }
        });
        if new_desk == prev_desk {
            return false;
        }
        self.hover_desk = new_desk;
        // Anchor popup once per desk enter (or clear).
        self.hover_screen = new_desk.map(|_| (col, row)).or(None);
        if new_desk.is_none() {
            self.hover_screen = None;
        }
        // Mouse landing on a desk clears keyboard focus; empty keeps Tab focus.
        if new_desk.is_some() && new_desk != self.keyboard_focus {
            self.keyboard_focus = None;
        }
        self.mark_redraw_dirty();
        true
    }

    pub fn clear_hover(&mut self) {
        if self.hover_desk.is_some()
            || self.hover_screen.is_some()
            || self.keyboard_focus.is_some()
        {
            self.mark_redraw_dirty();
        }
        self.hover_desk = None;
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
        let cur = self.keyboard_focus.or(self.hover_desk);
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
        let cur = self.keyboard_focus.or(self.hover_desk);
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

    /// Fingerprint for pixel recompose — **only** inputs that change
    /// [`super::compose::compose_cell_frame`] output.
    ///
    /// PERF INVARIANTS (RC13):
    /// 1. Pure `tick_anim` while all desks are empty/thinking and the supervisor
    ///    is Idle/Waiting must keep `pixel_frame_fp` stable (no recompose).
    /// 2. `hover_desk` / `hover_screen` are **excluded** — focus ring + popup are
    ///    painted as buffer overlays after halfblock paint.
    /// 3. Wall title, overflow, labels, tokens, elapsed, activity are **excluded**
    ///    (status strip / hover popup only).
    /// 4. `anim_t` is hashed only for walk path positions (handoff/exit), not for
    ///    seated desk blink (which uses the tick frame bucket).
    /// 5. Scaled BG cache is independent — see [`Self::ensure_pixel_frame`].
    fn visual_fingerprint(&self, cell_w: u16, cell_h: u16) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        cell_w.hash(&mut h);
        cell_h.hash(&mut h);
        super::sprites_pixel::effective_pixel_scale(cell_w, cell_h).hash(&mut h);
        (self.supervisor as u8).hash(&mut h);
        // Coarse sprite frame bucket (~ tick÷4) only when compose samples it.
        if self.pixel_needs_tick_frame() {
            (self.tick / 4).hash(&mut h);
        } else {
            0u64.hash(&mut h);
        }
        for d in &self.desks {
            d.child_session_id.hash(&mut h);
            (d.phase as u8).hash(&mut h);
            d.skin.hash(&mut h);
            // Walk path smoothness — include SpawnWalk slide.
            if matches!(
                d.phase,
                ActorPhase::WalkToBoss
                    | ActorPhase::Handoff
                    | ActorPhase::ExitDoor
                    | ActorPhase::SpawnWalk
            ) {
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
    pub fn ensure_pixel_frame(&mut self, cell_w: u16, cell_h: u16) -> bool {
        if !self.pixel_mode || cell_w == 0 || cell_h == 0 {
            return false;
        }
        let fp = self.visual_fingerprint(cell_w, cell_h);
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

        // Rescale BG only when terminal cell size or pixel_scale asset factor changes.
        let scale = super::sprites_pixel::effective_pixel_scale(cell_w, cell_h).max(1);
        if self.pixel_cell_w != cell_w
            || self.pixel_cell_h != cell_h
            || self.pixel_bg_scale != scale
            || self.pixel_bg_scaled.is_none()
        {
            let Some(full) = self.pixel_bg_full.as_ref() else {
                return false;
            };
            // Temporarily pin scale for this compose via thread-local? scale_bg uses
            // pixel_scale() — ensure effective scale is applied by sprites_pixel helper.
            self.pixel_bg_scaled =
                Some(super::compose::scale_bg_to_cells_with_scale(full, cell_w, cell_h, scale));
            self.pixel_cell_w = cell_w;
            self.pixel_cell_h = cell_h;
            self.pixel_bg_scale = scale;
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
        super::compose::compose_cell_frame_into(&mut scratch, &bg, self, tick);
        // Terminal-res paint buffer for halfblock (use_direct — no per-paint resize).
        let paint_w = u32::from(cell_w).max(1);
        let paint_h = u32::from(cell_h).saturating_mul(2).max(1);
        let paint = if scratch.width() == paint_w && scratch.height() == paint_h {
            scratch.clone()
        } else {
            image::imageops::resize(
                &scratch,
                paint_w,
                paint_h,
                image::imageops::FilterType::Nearest,
            )
        };
        let halfblock =
            xai_grok_pager_render::render::image_overlay::HalfblockCellCache::from_rgba(
                &paint, cell_w, cell_h,
            );
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
            self.mark_redraw_dirty();
        }
    }

    /// Whether a paint-side sync should run (tick already synced recently).
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
    /// tick, so it must contribute **no** tick demand and let the app park at
    /// `TickDemand::None`.
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
        // Armed attention window: the wall must flip back when it expires.
        if self.attention_until.is_some_and(|t| t > Instant::now()) {
            return true;
        }
        // Seated desks animate (typing, walks, celebrate/fail beats) and own the
        // hover/focus ring — an empty room has neither.
        self.desks.iter().any(|d| d.is_occupied())
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
        // Compact mid-walk: snap-complete handoffs (spec §7.8).
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
                    self.supervisor = SupervisorPhase::Reviewing;
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
        let failed_ids: Vec<&str> = agents
            .iter()
            .filter(|a| a.failed && !a.running)
            .map(|a| a.child_session_id.as_str())
            .collect();
        let mut new_fail = false;
        for id in &failed_ids {
            if self.attention_armed_ids.insert((*id).to_string()) {
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
        for i in 0..DESK_COUNT {
            let Some(sid) = self.desks[i].child_session_id.clone() else {
                continue;
            };
            if let Some(snap) = agents.iter().find(|a| a.child_session_id == sid) {
                self.desks[i].label = snap.label.clone();
                self.desks[i].subagent_type = snap.subagent_type.clone();
                self.desks[i].elapsed = snap.elapsed;
                self.desks[i].tokens = snap.tokens;
                self.desks[i].tool_calls = snap.tool_calls;
                self.desks[i].activity = snap.activity.clone();
                self.desks[i].failed = snap.failed;

                if snap.running {
                    // Don't clobber walk animations.
                    if matches!(
                        self.desks[i].phase,
                        ActorPhase::AtDeskWorking
                            | ActorPhase::AtDeskThinking
                            | ActorPhase::SpawnWalk
                    ) {
                        let thinking = snap.activity.to_ascii_lowercase().contains("think");
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

        self.overflow_count = self.door_queue.len();
        self.update_supervisor(supervisor_working);
        let attention_active = self
            .attention_until
            .is_some_and(|t| t > Instant::now());
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
    }

    /// Drop composited pixel caches (e.g. terminal resize). Scaled BG rebuilds
    /// on the next [`Self::ensure_pixel_frame`].
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
    /// room animates ([`Self::needs_animation_tick`]) — `render.rs::format_clock`
    /// derives its decorative seconds as `tick/12`, so that clock stands still
    /// while a frozen room is parked (RC16 PERF-1).
    ///
    /// Marks redraw dirty when visual output may change (working desks, walks,
    /// focus pulse edge).
    pub fn tick_anim(&mut self, tier: GameTier) {
        let tick_before = self.tick;
        let needs_frames = self.pixel_needs_tick_frame();
        let had_focus = self.focus_desk().is_some();
        self.tick = self.tick.wrapping_add(1);
        self.last_tick = Instant::now();
        // Focus ring pulse flips every 4 ticks.
        if had_focus && (tick_before / 4) != (self.tick / 4) {
            self.mark_redraw_dirty();
        }
        if needs_frames {
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
        assert_eq!(s.hover_desk, Some(1));
        assert!(s.update_hover(0, 0), "leaving desk dirties");
        assert_eq!(s.hover_desk, None);
    }

    #[test]
    fn fingerprint_stable_on_idle_tick_and_hover() {
        let mut s = GameModeState::new();
        // Thinking desk + idle supervisor → no tick frame sampling.
        s.desks[0].child_session_id = Some("t".into());
        s.desks[0].phase = ActorPhase::AtDeskThinking;
        s.desks[0].skin = 1;
        s.supervisor = SupervisorPhase::Idle;
        let fp0 = s.visual_fingerprint(80, 24);
        s.tick = s.tick.wrapping_add(40);
        s.hover_desk = Some(0);
        s.hover_screen = Some((10, 10));
        s.overflow_count = 3;
        assert_eq!(
            s.visual_fingerprint(80, 24),
            fp0,
            "idle/thinking + hover must not dirty pixel fingerprint"
        );
    }

    /// RC16 PERF-1: a synced, frozen room must not ask for ticks — that is what
    /// lets `AppView::tick_demand` park the loop at `None` while the office is
    /// open. A fresh (never-synced) room always does.
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
        attention.attention_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(
            !attention.needs_animation_tick(),
            "expired attention window must not keep ticking"
        );

        let mut dirty = base();
        dirty.mark_redraw_dirty();
        assert!(
            dirty.needs_animation_tick(),
            "pending redraw must be flushed"
        );
    }

    #[test]
    fn fingerprint_moves_with_working_tick_bucket() {
        let mut s = GameModeState::new();
        s.desks[0].child_session_id = Some("w".into());
        s.desks[0].phase = ActorPhase::AtDeskWorking;
        s.supervisor = SupervisorPhase::Waiting;
        let fp0 = s.visual_fingerprint(80, 24);
        s.tick = 3; // still same ÷4 bucket as 0
        assert_eq!(s.visual_fingerprint(80, 24), fp0);
        s.tick = 4; // next bucket
        assert_ne!(s.visual_fingerprint(80, 24), fp0);
    }
}
