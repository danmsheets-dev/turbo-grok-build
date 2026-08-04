# Design: Game Mode (RC11)

| Item | Content |
|------|---------|
| Status | **Implementing** — pixel path integrated into pager (2026-08-02) |
| Release | RC11 |
| Visual tier | **Primary:** mockup PNG + procedural 8-bit Rust sprites → halfblock paint. **Fallback:** Unicode office / Compact cards |
| Interaction | Spectator + composer to Supervisor; no desk open/kill/spawn |
| Toggle | `Ctrl+G` — Normal View ↔ Game Mode (tasks pane: `Ctrl+Shift+G`) |
| Mockup | `game_mode_mockup.png` (inspiration; not pixel-identical) |

---

## 1. Goals

1. **Entertaining spectator view** of the main agent (Supervisor) and up to six subagents as an office full of sprites.
2. **Keep chatting**: Game Mode retains the normal composer; input always goes to the Supervisor (main Turbo agent).
3. **Informative at a glance**: wall display + per-desk monitors + status strip so the user knows if work is active, stuck, or finished.
4. **Fun beats**: spawn walk-in, work animation, finish → walk to boss → handoff → Supervisor reviewing, fail beat, ambient motion.
5. **Resize-safe**: usable across common terminal sizes via breakpoints + letterboxing (see §7).

## 2. Non-goals (RC11)

- Opening/killing/spawning subagents from desks.
- Image-protocol / Kitty / Sixel room backdrop (optional later).
- Pixel-perfect match to `game_mode_mockup.png`.
- True continuous zoom (terminals are cell grids; we scale by **layout breakpoints + spacing**, not bilinear image scale).
- Dashboard replacement (dashboard remains the dense operational list).

---

## 3. Mode & chrome

| Item | Behavior |
|------|----------|
| Toggle | `ActionId::ToggleGameMode`, default chord **`Ctrl+G`**. Toggles Normal ↔ Game for the active agent session. |
| Composer | Always visible at bottom; same prompt path as Normal View. Sends to Supervisor only. |
| Exit | `Ctrl+G` again. Prefer not stealing `Esc` from global overlays unless Game Mode is top-most and no modal is open. |
| Scrollback | Hidden while Game Mode is open (room replaces conversation chrome above the composer). |
| Focus | Spectator: no desk selection required. Optional highlight for polish is out of scope unless free. |

### 3.1 Keybind notes

- **Ctrl+G** opens Game Mode (RC11+). Tasks pane moved to **`Ctrl+Shift+G`**.
- Minimal mode keeps `Ctrl+G` for external editor; Game Mode is not the Ctrl+G owner there.
- Register in shortcuts help under Panels / View.

---

## 4. Data model (existing sources)

No new agent protocol for RC11. Derive room state each frame (or on tick) from:

| Source | Use |
|--------|-----|
| Main `AgentView` turn / streaming | Supervisor `Working` vs idle |
| `AgentView::subagent_sessions` → `SubagentInfo` | Desk occupancy, monitors, finish events |
| `SubagentInfo::is_running`, `finished`, `status` | Sprite state machine |
| `display_elapsed`, `tokens_used` / usage, `tool_call_count` / `tool_calls` | Monitor HUD |
| `activity_label` | Monitor marquee + thinking vs tool |
| Finish notifications | Enqueue handoff animation |

### 4.1 Desk slots

- **Six fixed slots** (`0..5`), layout 2×3 under the Supervisor zone.
- **Stable assignment**: first free slot on spawn; keep `child_session_id → slot` until handoff sequence completes and slot is released.
- **>6 concurrent running**: extras wait “by the door” as `+N` badge (no seventh desk sprite). When a slot frees, promote from queue (spawn walk-in into freed desk).
- **Empty slot**: chair + dark monitor + dim `IDLE`.

---

## 5. Room layout (logical stage)

Logical regions (top → bottom), then composer outside the stage:

```
┌─────────────────────────────────────────────────────────────┐
│  WALL DISPLAY (title status)              session clock     │
│  props · SUPERVISOR DESK + sprite · whiteboard · cooler     │
│                     handoff / rug zone                        │
│   Desk0        Desk1        Desk2                             │
│   Desk3        Desk4        Desk5              door           │
├─────────────────────────────────────────────────────────────┤
│  status strip                                                 │
├─────────────────────────────────────────────────────────────┤
│  composer (Supervisor)                                        │
└─────────────────────────────────────────────────────────────┘
```

### 5.1 Sprites

**Supervisor** — distinct boss palette (horns / gold accent). States:

- `Idle` / `Waiting`
- `Working` (typing when main turn streams)
- `Reviewing` (after handoff or while processing post-child work)

**Developers** — up to six palette variants; optional tint by `subagent_type`. States:

- `AtDeskWorking`, `AtDeskThinking`
- `WalkToBoss`, `Handoff`, `WalkBack` or `ExitDoor`
- (absent) for empty desk

**Props** — floor tile, rug, plants, bookshelf, whiteboard frame, water cooler, door. Ambient: slow plant sway, clock, cooler bubble.

Sprites are **preauthored** multi-cell frames (`&'static` or small owned `Text`/`Line` tables), not generative each frame.

---

## 6. Interactions & wall display

### 6.1 Handoff (success complete)

On successful `SubagentFinished`:

1. Short desk celebrate (~0.4s).
2. Pathfind (coarse grid / fixed waypoints) to rug handoff zone.
3. Handoff anim (packet → Supervisor).
4. Supervisor → `Reviewing`.
5. Agent clears slot (return-then-despawn **or** exit door — prefer **exit door** after handoff for “job done” readability; empty desk left behind).
6. Multiple finishers: **FIFO queue** on rug; one handoff at a time.

### 6.2 Other RC11 beats

| Beat | Trigger | Visual |
|------|---------|--------|
| Spawn entrance | New running subagent, free slot | Walk in door → sit desk |
| Typing | Running + tool-ish activity | Keyboard cycle, monitor code scroll |
| Thinking | Activity “Thinking” / no tool | `…` bubble |
| Tool flash | Tool call count / activity change | Monitor brief flash |
| Supervisor typing | Main turn streaming | Boss typing frames |
| Fail | finished + failed/cancelled | Red monitor, short head-desk; no success handoff |
| Ambient | Always (low rate) | Clock, plants, cooler |

### 6.3 Wall display

Large high-contrast banner, center-top (scales with width — §7).

| Condition | Text |
|-----------|------|
| Any running subagent or main turn | `WORKING` + `n/6` desks + short activity |
| Main only | `SUPERVISOR BUSY` |
| No running children, no main turn, never completed this session open | `STANDBY` / `WAITING FOR ORDERS` |
| No running children, no main turn, ≥1 successful completion this Game Mode session | **`WORK FINISHED`** (green pulse) |
| Any failed/cancelled recent child (sticky briefly) | `NEEDS ATTENTION` |
| Permission / needs-user if already visible in pager state | `WAITING ON YOU` |

### 6.4 Monitors & status strip

**Occupied monitor:** type/role · elapsed · tokens · tools · activity marquee.

**Status strip (always):** occupancy dots · active count · session-ish token/tool summary · Supervisor chip · `Ctrl+G` legend.

---

## 7. Window scaling (resize support)

### 7.1 Reality check

Ratatui draws on a **character cell grid**. We cannot continuously “zoom” sprites like a GPU game. What we *can* do well:

1. **Responsive layout** — recompute regions every frame from `Frame::area()` (already how dashboard/agent layouts work).
2. **Breakpoints** — pick art density and desk geometry from width/height.
3. **Letterbox / pillarbox** — keep a designed aspect stage centered; fill margins with extended walls/floor (not empty black if avoidable).
4. **Minimum size gate** — below a floor, show a compact fallback instead of a broken office.
5. **Text truncation** — monitors and wall display use ellipsis / priority fields as width shrinks.

Resize events already force redraw in the pager; Game Mode must **not cache pixel rects across frames** without invalidation. On resize mid-walk, **clamp actor positions** into the new grid (snap to nearest legal cell / repath next tick).

### 7.2 Vertical chrome budget

Always reserve:

| Region | Height policy |
|--------|----------------|
| Composer + status-ish prompt chrome | Existing agent prompt height (dynamic multiline) |
| Game status strip | 1 row (fixed) |
| Room stage | **remainder** |

If remainder height is below `MIN_STAGE_ROWS`, enter **Compact fallback** (§7.5).

### 7.3 Breakpoints

Named tiers (exact numbers tunable in playground; start here):

| Tier | Approx size | Behavior |
|------|-------------|----------|
| **Compact** | stage &lt; 72×18 **or** total terminal &lt; 80×24 usable | No free-walk office. Card grid: Supervisor banner + 6 mini desks (2×3 text cards) + wall line. Still animated (spinners, WORK FINISHED). Composer kept. |
| **Normal** | stage ≥ 72×18 and &lt; 120×28 | Full office. **Small sprites** (e.g. developer 3×3, desk ~10×5). Tighter aisles. Monitors: elapsed + activity only (tokens/tools if width allows). |
| **Comfort** | stage ≥ 120×28 and &lt; 160×36 | Mockup-like spacing. **Standard sprites** (developer ~4×4, desk ~14×6). Full monitor HUD. Handoff path readable. |
| **Wide** | stage ≥ 160×36 | Extra floor/props padding; wall display grows; optional wider rug. **Do not** invent a 4th desk row — still 6 desks. Pillarbox with plants/books rather than stretching sprites past max frame size. |

Width and height can disagree: take **the more constrained tier** (e.g. wide but short → Compact or Normal by height).

```text
fn game_tier(stage: Rect) -> GameTier {
    match (stage.width, stage.height) {
        (w, h) if w < 72 || h < 18 => Compact,
        (w, h) if w < 120 || h < 28 => Normal,
        (w, h) if w < 160 || h < 36 => Comfort,
        _ => Wide,
    }
}
```

### 7.4 Stage letterboxing

1. Compute `stage = area above status strip + composer`.
2. Choose tier from stage size.
3. For Normal/Comfort/Wide, compute **content rect**:
   - Target aspect ≈ **16:9 to 2:1** office (prefer width-driven).
   - `content_w = min(stage.width, max_stage_width_for_tier)`
   - `content_h = min(stage.height, max_stage_height_for_tier)`
   - Center: `x = stage.x + (stage.width - content_w) / 2`, same for `y`.
4. Paint **margin** with continuing wall/floor pattern (same palette) so letterbox feels like a larger building, not a floating HUD.
5. All sprite coordinates are **relative to content rect**, then offset for draw.

### 7.5 Compact fallback (small windows)

When the office would be unreadable:

- One-line **wall display** (same state machine text).
- Supervisor chip row.
- **6 desk cards** in 2×3 `Layout` with `Constraint::Ratio` / `Min`:
  - Empty: `· IDLE`
  - Busy: name/type · elapsed · spinner
- Skip walk animations; on finish, **short “→ boss” flash** or card slides to “done” then clears (optional micro-anim, 2–3 frames).
- Composer unchanged.

This guarantees Game Mode never requires a specific font size or maximized window.

### 7.6 Sprite scaling strategy

| Approach | RC11 |
|----------|------|
| Multiple sprite sets (sm / md) | **Yes** — two sets tied to Normal vs Comfort/Wide |
| Procedural double-width cells | No (looks bad with most fonts) |
| Half-block “hi-res” sprites | Optional later; not required |
| Stretch single art with spaces | **No** — use padding between desks instead |

Desk **positions** are computed by splitting the desk region:

```text
horizontal: Layout::horizontal([Fill; 3]) with min desk width
vertical:   Layout::vertical([Fill; 2])
```

Each cell of the 2×3 gets a `Rect`; desk art is **centered** inside that rect. Extra space = floor tiles (scales naturally).

### 7.7 Text & wall display scaling

- Wall display title: `WORKING` / `WORK FINISHED` uses full width up to a max; center text; pulse via style tick.
- Subtitle (activity) truncates with `…` when `width < len`.
- Monitor fields drop by priority: `activity → elapsed → tokens → tools` as desk rect shrinks.
- Unicode width via existing `unicode-width` usage in pager/textarea stack.

### 7.8 Resize during animation

| Situation | Policy |
|-----------|--------|
| Tier change mid-handoff | Snap actor to equivalent slot / handoff cell in new layout; continue state machine without restarting handoff |
| Compact entered mid-walk | Cancel walk; complete handoff as instant desk-clear + Supervisor `Reviewing` beat |
| Expand from Compact | Reseat running agents at stable slots; no retroactive walk-in |

### 7.9 Testing resize

- Unit: pure `layout::compute(stage, chrome) -> GameLayout` for widths `{60, 80, 100, 120, 160, 200}` and heights `{20, 24, 30, 40, 50}`.
- Assert: non-overlapping desk rects; composer not overlapped; wall display height ≥ 1; Compact when below mins.
- Playground: resize terminal manually; optional `game_mode_playground` with forced tier flag.

---

## 8. Animation / performance

- While Game Mode open: request redraw on a **~12–15 Hz** tick (reuse pager tick patterns).
- Idle ambient at lower effective rate (frame counter modulo).
- Preauthored frames only; pathfinding on coarse grid (desk → aisle → rug).
- No per-frame heap art generation; reuse buffers where the rest of the pager does.

---

## 9. Module layout (implementation sketch)

```text
xai-grok-pager/src/views/game_mode/
  mod.rs          // public render + toggle surface
  state.rs        // GameModeState, slots, handoff queue, session flags
  layout.rs       // tiers, letterbox, desk rects (pure)
  sprites.rs      // frames for boss / dev / props (sm + md)
  anim.rs         // walk, handoff, ambient tick
  wall.rs         // wall display state machine
  monitor.rs      // per-desk HUD text
  render.rs       // paint Buffer / Frame
```

Integration points:

- `ActionId::ToggleGameMode` + defaults + shortcuts help
- Agent render path: if game mode open, draw room + strip above composer (composer path unchanged)
- Finish/spawn hooks: feed `GameModeState` events from existing subagent notification handling
- Playground: `src/bin/game_mode_playground.rs` for art/tier iteration

---

## 10. RC11 acceptance criteria

1. `Ctrl+G` toggles Normal ↔ Game; composer works in Game Mode → Supervisor.
2. Up to 6 desks show running subagents; empty = IDLE desk.
3. Monitors show live elapsed / tokens / tools / activity when space allows.
4. Wall display shows WORKING / STANDBY / **WORK FINISHED** / attention states correctly.
5. Success finish plays handoff (or Compact equivalent); Supervisor enters Reviewing.
6. Spawn can walk in from door when not Compact.
7. **Resize:** Comfort → Normal → Compact does not panic; layout remains readable; no overlapping critical HUD.
8. Below minimum size, Compact fallback remains informative (not a clipped mess).
9. Failures do not use success handoff.
10. Shortcuts help documents `Ctrl+G` (Game Mode) and `Ctrl+Shift+G` (tasks).

---

## 11. Post-RC11 (out of scope but planned)

- Desk select + Enter to open subagent.
- Image-protocol poster room with Ratatui overlays.
- Water cooler / coffee / trophy shelf.
- Half-block hi-res sprite set for Comfort+.
- User preference: default open Game Mode, animation speed, reduce motion.

**Reduce motion:** if we already have an a11y/animation preference, honor it (skip walks; instant seat/clear). If not, document as follow-up; default full motion for RC11.

---

## 12. Open tunables (playground, not product blockers)

- Exact breakpoint numbers (72×18 / 120×28 / 160×36).
- Handoff: exit door vs return-to-empty-desk (default: **exit door**).
- WORK FINISHED sticky duration after new work starts (immediate clear on new spawn/turn).
- Palette tokens aligned with Turbo theme vs fixed “game” palette (prefer theme-aware base + fixed accent for sprites).

---

## 13. Spec self-review

- [x] No TBD blockers for RC11 scope
- [x] Scaling is explicit (breakpoints + letterbox + Compact), not “somehow scale”
- [x] Interaction model matches spectator + composer
- [x] Data from existing `SubagentInfo` only
- [x] Acceptance criteria testable
