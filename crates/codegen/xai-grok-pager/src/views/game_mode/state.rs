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
    /// Background scaled to last paint size (`cell_w × cell_h*2`).
    pub(crate) pixel_bg_scaled: Option<RgbaImage>,
    pub(crate) pixel_cell_w: u16,
    pub(crate) pixel_cell_h: u16,
    /// Last composited cell-resolution frame (no PNG).
    pub(crate) pixel_frame: Option<RgbaImage>,
    /// Visual fingerprint for the cached frame.
    pub(crate) pixel_frame_fp: u64,
    /// Prefer pixel office (mockup + sprites). False falls back to Unicode.
    pub pixel_mode: bool,
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
            pixel_frame: None,
            pixel_frame_fp: 0,
            pixel_mode: true,
        }
    }

    /// Fingerprint for recompose. Uses coarse tick (÷6) so we do not rebuild
    /// every anim pulse for the wall ribbon alone more than ~2 Hz extras.
    fn visual_fingerprint(&self, cell_w: u16, cell_h: u16) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        cell_w.hash(&mut h);
        cell_h.hash(&mut h);
        (self.tick / 6).hash(&mut h);
        self.wall.title().hash(&mut h);
        (self.supervisor as u8).hash(&mut h);
        self.overflow_count.hash(&mut h);
        for d in &self.desks {
            d.child_session_id.hash(&mut h);
            (d.phase as u8).hash(&mut h);
            d.skin.hash(&mut h);
            // Coarse anim position (0..10) — enough for walk smoothness
            ((d.anim_t * 10.0) as u8).hash(&mut h);
        }
        h.finish()
    }

    /// Ensure a cell-resolution RGBA frame is ready for halfblock paint.
    ///
    /// Returns true when `pixel_frame` can be painted. Never PNG-encodes.
    pub fn ensure_pixel_frame(&mut self, cell_w: u16, cell_h: u16) -> bool {
        if !self.pixel_mode || cell_w == 0 || cell_h == 0 {
            return false;
        }
        let fp = self.visual_fingerprint(cell_w, cell_h);
        if self.pixel_frame_fp == fp && self.pixel_frame.is_some() {
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

        // Rescale BG only when terminal cell size changes.
        if self.pixel_cell_w != cell_w
            || self.pixel_cell_h != cell_h
            || self.pixel_bg_scaled.is_none()
        {
            let Some(full) = self.pixel_bg_full.as_ref() else {
                return false;
            };
            self.pixel_bg_scaled = Some(super::compose::scale_bg_to_cells(full, cell_w, cell_h));
            self.pixel_cell_w = cell_w;
            self.pixel_cell_h = cell_h;
        }

        let Some(bg) = self.pixel_bg_scaled.take() else {
            return false;
        };
        let tick = self.tick;
        let frame = super::compose::compose_cell_frame(&bg, self, tick);
        self.pixel_bg_scaled = Some(bg);
        self.pixel_frame = Some(frame);
        self.pixel_frame_fp = fp;
        true
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.last_tick = Instant::now();
        }
    }

    /// Sync seats from current subagent snapshots + whether main agent is streaming.
    pub fn sync_from_snapshots(
        &mut self,
        agents: &[DeskAgentSnapshot],
        supervisor_working: bool,
        tier: GameTier,
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

        // Arm brief attention window on new failures.
        if agents.iter().any(|a| a.failed && !a.running) {
            let until = Instant::now() + Duration::from_secs(12);
            self.attention_until = Some(match self.attention_until {
                Some(prev) if prev > Instant::now() => prev.max(until),
                _ => until,
            });
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
                    && matches!(
                        self.desks[i].phase,
                        ActorPhase::AtDeskWorking
                            | ActorPhase::AtDeskThinking
                            | ActorPhase::SpawnWalk
                    )
                {
                    // Success finish → celebrate then handoff.
                    self.begin_success_finish(i, tier);
                } else if snap.failed
                    && matches!(
                        self.desks[i].phase,
                        ActorPhase::AtDeskWorking
                            | ActorPhase::AtDeskThinking
                            | ActorPhase::SpawnWalk
                    )
                {
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
        );
    }

    fn begin_success_finish(&mut self, desk: usize, tier: GameTier) {
        self.had_success = true;
        if !tier.uses_office_art() {
            // Compact: instant clear after short celebrate flag.
            self.desks[desk].phase = ActorPhase::Celebrate;
            self.desks[desk].phase_started = Instant::now();
            self.desks[desk].anim_t = 0.0;
            return;
        }
        self.desks[desk].phase = ActorPhase::Celebrate;
        self.desks[desk].phase_started = Instant::now();
        self.desks[desk].anim_t = 0.0;
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

    /// Advance animations. Call ~12–15 Hz while open.
    pub fn tick_anim(&mut self, tier: GameTier) {
        self.tick = self.tick.wrapping_add(1);
        self.last_tick = Instant::now();

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
                        } else if !self.handoff_queue.contains(&i)
                            && !self.desks.iter().any(|d| {
                                matches!(d.phase, ActorPhase::WalkToBoss | ActorPhase::Handoff)
                            })
                        {
                            // Start walk if no other handoff in flight.
                            self.desks[i].phase = ActorPhase::WalkToBoss;
                            self.desks[i].phase_started = Instant::now();
                            self.desks[i].anim_t = 0.0;
                        } else if !self.handoff_queue.contains(&i) {
                            self.handoff_queue.push_back(i);
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
        s.sync_from_snapshots(&agents, false, GameTier::Comfort);
        assert_eq!(s.active_desk_count(), 6);
        assert_eq!(s.overflow_count, 2);
    }

    #[test]
    fn success_starts_celebrate() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("a", true)], false, GameTier::Comfort);
        assert_eq!(s.active_desk_count(), 1);
        s.sync_from_snapshots(&[snap("a", false)], false, GameTier::Comfort);
        assert!(matches!(s.desks[0].phase, ActorPhase::Celebrate));
        assert!(s.had_success);
    }

    #[test]
    fn stable_seat_map() {
        let mut s = GameModeState::new();
        s.sync_from_snapshots(&[snap("x", true), snap("y", true)], false, GameTier::Normal);
        let ix = *s.seat_map.get("x").unwrap();
        let iy = *s.seat_map.get("y").unwrap();
        s.sync_from_snapshots(&[snap("y", true), snap("x", true)], false, GameTier::Normal);
        assert_eq!(s.seat_map.get("x"), Some(&ix));
        assert_eq!(s.seat_map.get("y"), Some(&iy));
    }
}
