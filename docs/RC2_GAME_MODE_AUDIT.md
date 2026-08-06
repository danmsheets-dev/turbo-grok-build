# RC2 Game Mode Audit — Bugs, Performance, Hover Info & Sprite Animations

> **STATUS: IMPLEMENTED** on `rc2-game-mode` (18 commits). Every bug and perf finding below is fixed; both hover
> tooltips and 11 of the 12 animation proposals shipped. Animation #10 (supervisor pacing) was **skipped** — see
> "Deferred" at the bottom. Game Mode tests went 25 → 132. Do not read this document as a list of open issues;
> it is the design record for work that has landed. Shipped behavior is summarized in `KNOWN_ISSUES.md`.

**Date:** 2026-08-05 · **Branch:** `dev` @ `28038242d` · **Method:** 19-agent workflow (4 bug/perf finders, 2 feature assessors, 12 adversarial verifiers, 1 completeness critic), static analysis only — nothing compiled or profiled.

**Scope:** `crates/codegen/xai-grok-pager/src/views/game_mode/` (mod, state, layout, compose, render, sprites, sprites_pixel, monitor, wall) plus integration (`app_view.rs` tick/demand, `agent_view/{render,input}.rs`, `event_loop.rs`, halfblock overlay). GBoom is a separate mini-game and was excluded.

**Verdict legend:** ✅ = adversarially verified against the code (verifier tried to refute, failed). ⚠️ = reported with evidence but not independently verified (verification capped at 12).

---

## 1. Confirmed bugs

### BUG-1 ✅ HIGH — Tick-path sync double-peels the status strip (tier off-by-one vs paint)
`views/game_mode/mod.rs:98`, `layout.rs:89-105`, `render.rs:33-35`, `app_view.rs:5554-5563`

`compute_layout` peels a 1-row status strip before computing the tier. The paint path stores `last_stage = layout.stage` — the *already-peeled* rect — and `AppView::tick` feeds those dims back into `sync_game_mode`, which calls `compute_layout` again and **peels a second time**. The tick-side tier is therefore always evaluated on a stage one row shorter than what is painted.

- **Catastrophic case:** paint area exactly 19 rows → paint tier Normal (stage 18 = `MIN_STAGE_H`), tick tier Compact (17). Every ~83 ms tick executes the compact snap-complete branch: every desk in SpawnWalk/Celebrate/WalkToBoss/Handoff/ExitDoor is instantly cleared and `handoff_queue` is wiped (`state.rs:533-548`), while the paint layer still renders full office art. All walk/celebrate animations are permanently suppressed at that height.
- At the other tier boundaries (28, 36) the same off-by-one causes only a sprite-set mismatch. Width is unaffected.
- The comment at `app_view.rs:5552-5557` claims this exact pathway "avoids Normal↔Compact thrash that snap-clears walks/handoffs" — it does the opposite at the boundary. Independently re-confirmed by the completeness critic at exact lines.
- **Fix:** make `last_stage` carry the pre-peel area, or have `sync_game_mode` accept an already-peeled stage and call `game_tier` directly. Tick-path tier must equal paint-path tier by construction.

### BUG-2 ✅ MED — Animation tick gate (90 ms) beats the Slow tick interval (83 ms): everything animates at ~6 Hz
`mod.rs:122`, `app_view.rs:266` (`SLOW_TICK_INTERVAL`), `event_loop.rs:3136-3138`

`tick_anim` only fires when `last_tick.elapsed() >= 90ms`, but the driving cadence is 83 ms. The first tick after an anim step measures ~83-85 ms < 90 → skipped; anim advances every ~166 ms. Documented intent is "~10–12 Hz" (`mod.rs:120`, `state.rs:784`).

- Sprite frame buckets (`tick/4`) advance every ~664 ms; a 900 ms WalkToBoss shows ~5-6 samples; focus pulse halves; the unicode wall clock (`render.rs:398-404`, `secs = tick/12`) runs at **half real-time**.
- Animation rate is also **demand-dependent**: when unrelated chrome forces Fast (33 ms), the gate passes ~every 99 ms → ~10 Hz. Office speed silently changes with UI state.
- Reported independently by three finders; verified twice. `docs/KNOWN_ISSUES.md:20` (RC13 entry) documents the shipped anim cadence as fixed — the doc is now wrong and should be updated with whatever fix lands.
- **Fix:** derive the gate from `SLOW_TICK_INTERVAL` minus a jitter margin (e.g. ≥80 ms), or drop the gate and let the event-loop cadence own the rate.

### BUG-3 ✅ MED — ExitDoor walk teleports backward and walks into the boss instead of exiting through the door
`compose.rs:388-394` (ExitDoor arm; finder originally miscited render.rs), `state.rs:864-867`

All walk phases interpolate along the desk→`SUPERVISOR_ANCHOR` line. Handoff renders at `t=1.0` (on the supervisor); the Handoff→ExitDoor transition resets `anim_t` to 0 and ExitDoor maps to `t = 0.55 + anim_t*0.45`. So the actor visibly teleports 45% of the path backward, then walks *into the supervisor again* and vanishes there. The door (x ≈ 0.06·w, used by SpawnWalk) is never approached; "Walking out the door after handoff" (`state.rs:43-44`) never happens.

- **Fix:** for ExitDoor interpolate supervisor→door: `x = sup_x + (door_x − sup_x)·anim_t`, exiting stage left to match SpawnWalk's entry.

### BUG-4 ⚠️ MED — Thinking-only rooms freeze the Compact/unicode HUD (elapsed, tokens, marquee)
`state.rs:341-361` (`pixel_needs_tick_frame` excludes AtDeskThinking), `state.rs:788-801`, `monitor.rs:87-121`, `render.rs:508-546`

The idle-freeze contract is correct for the pixel office (a thinking sprite is static), but Compact/unicode tiers paint per-desk HUDs with time-varying data (elapsed timer, tokens, tick-scrolled marquee). Desk data *is* refreshed each sync, but nothing marks redraw dirty, so the painted `01:23` timer can sit frozen for minutes until any other repaint happens. **Fix:** when the active tier doesn't use office art, mark redraw dirty at a low cadence while any desk is occupied (or hash a coarse elapsed bucket into the sync dirty signature for non-pixel tiers).

### Low-severity bugs (⚠️ unverified, evidence-backed)

| # | Where | Issue |
|---|-------|-------|
| B5 | `render.rs:213` | Hover popup + drop shadow can paint outside the game area into adjacent UI rows |
| B6 | `sprites_pixel.rs:452` | Typing developer's arm rect (2px tall) is fully consumed by its own outline — no interior pixels |
| B7 | `sprites.rs:160` | Unicode Small empty-desk: IDLE row one column wider than its box borders |
| B8 | `sprites.rs:81` | Unicode Medium supervisor: desk-front row 2-3 cols narrower than the SUPER header |
| B9 | `mod.rs:47` | Failed-status match (`"failed"`/`"cancelled"`) is narrower than the dashboard's classifier — a future `"error"` status would celebrate instead of FailBeat |
| B10 | `mod.rs:54` | Finished subagents show stale live tool-call count (precedence of `tool_call_count` vs `tool_calls` inverted relative to tasks pane) |
| B11 | `mod.rs:111` | Overflow (+N) changes never mark redraw dirty → stale door badge/status strip |
| B12 | `state.rs:544` | Dead write: compact snap-complete sets `SupervisorPhase::Reviewing`, unconditionally overwritten by `update_supervisor` |
| B13 | `monitor.rs:44` | `fmt_tokens` boundary rounding renders `1000.0k`/`1000.0M` instead of promoting units |

---

## 2. Confirmed performance issues

Verifiers agreed with every mechanism below but consistently noted absolute costs are microseconds-scale — this is idle-churn hygiene and battery/wakeup discipline, not user-visible jank. The two HIGHs are "high" for their unbounded/always-on shape, not magnitude.

### PERF-1 ✅ HIGH — Open Game Mode pins the event loop at ~12 Hz forever, even fully idle
`app_view.rs:5968` (tick_demand), `app_view.rs:266`

`tick_demand()` returns `Slow` unconditionally while `game_mode.open` — no check that anything can animate. A frozen room (no desks, supervisor Idle, fingerprint stable) still wakes the loop every 83 ms and runs the full `AppView::tick` body + snapshot rebuild. Without Game Mode an idle agent parks at `TickDemand::None` — zero wakeups, asserted by the test at `app_view.rs:6895`. Leave the office open overnight → ~12 wakeups/sec producing zero visual change.
**Fix:** return `Slow` only when the room can animate or needs sync (desk occupied, supervisor Working/Reviewing, queues non-empty, redraw pending); otherwise `None`, re-armed by ACP notifications/input events.

### PERF-2 ✅ HIGH — Full snapshot rebuild + sort every tick over an insert-only, ever-growing map
`mod.rs:35-76,106`, `app_view.rs:5563`, `agent_view/render.rs:1633-1638`, `state.rs:584-601`

Every sync rebuilds `Vec<DeskAgentSnapshot>` from **all** `subagent_sessions` — verified insert-only (`subagent.rs:676,714`, `session.rs:1493`, `session_notification.rs:359`; no remove/retain/clear anywhere) — with ~3-4 String allocs per entry plus a sort, then `sync_from_snapshots` does O(desks×sessions) scans and re-clones desk label/type/activity Strings (`state.rs:585-591`) and `to_ascii_lowercase()` per running desk (`:601`). No generation counter, no change detection; `phase_signature` hashed twice per call (`mod.rs:104,113`).
Session with 60 historical subagents, office idle: ~240 String allocs + sort every 83 ms, forever.
**Fix:** generation counter on `subagent_sessions` to skip unchanged syncs; filter finished sessions out of the snapshot source; compare-before-clone on desk fields.

### PERF-3 ✅ MED — Fast-demand chrome triples the sync rate to 30 Hz — exactly when the office is "WAITING ON YOU"
`app_view.rs:5563`, `:5929-5930`, `mod.rs:95-96`

The tick path has no interval throttle (only the paint path has the 40 ms gate). Permission prompts/question views force Fast (~33 ms) — and a pending permission is precisely the state Game Mode's wall advertises. A 2-minute prompt with 40 historical subagents ≈ 3,600 rebuilds (~576k String allocs) with nothing changing. **Fix:** reuse `last_sync_at` as an ~80 ms throttle in the tick path.

### PERF-4 ✅ MED — Whole-frame recompose per animation step; no incremental compositing
`compose.rs:284` (bg replace), `:168-196` (rug per-pixel blend), `state.rs:394-402`

Any single-sprite change (one walker moving a few pixels, one 12×12 monitor scroll) pays: full background replace (up to ~84k px at Normal/scale-3) + 7 floor stamps + all blits + full Nearest downsample + full halfblock cache rebuild. During walks the quantized `anim_t` misses the fingerprint essentially every 83 ms tick; a full handoff sequence is ~2.9 s ≈ 35 full-frame recomposes where <5% of pixels differ per step. Verifier bonus: during Handoff the walker is pinned at `t=1.0` (`compose.rs:389`) while `anim_t` still churns the fingerprint — **500 ms of full recomposes of pixel-identical frames**.
**Fix (cheap):** stop hashing `anim_t` during Handoff (pinned-t phases). **Fix (real):** dirty-rect compositing — restore bg under old+new sprite rects, re-blit affected sprites, re-sample covered cells only; keep full recompose for seat/phase/size changes.

### PERF-5 ✅ MED — Fresh paint buffer + halfblock cache allocated per fingerprint miss; dead reuse branch
`state.rs:480-499`, `sprites_pixel.rs:17-55`, `halfblock.rs:47-92`

Each miss allocates a new terminal-res `RgbaImage` (via `imageops::resize`) and a new packed cache Vec (~250 KB combined at a 200×90 stage) instead of writing in place — several MB/s of short-lived allocations during every walk window. The `scratch.clone()` branch at `state.rs:480-482` is **unreachable**: `effective_pixel_scale` is always ≥2, so scratch dims never equal paint dims. `pixel_paint` is retained but never painted from in the app path (`render.rs:53-58` always uses the cell cache; only the playground bin reads it).
**Fix:** retain and overwrite `pixel_paint`/`pixel_halfblock.packed` in place (`fill_from_rgba(&mut self, …)`); delete the dead branch (playground consumers noted at `game_mode_playground.rs:205,221`).

### PERF-6 ✅ MED — Redraw dirty marked 4× more often than pixels change while desks type
`state.rs:798`, `:384-388`

`tick_anim` marks dirty every tick while any desk works, but the fingerprint samples `tick/4` — so 3 of every 4 paints re-blit an identical office (CPU-side buffer rebuild + whole-stage cell blit + full-buffer diff; terminal I/O ≈ 0 since ratatui's diff finds nothing). **Fix constraint (verifier):** gate on bucket edges *for the pixel path only* — the unicode office animates at `tick%2`/`tick%4`/`tick%6` and would visibly slow if gated globally.

### PERF-7 ⚠️ MED — Closing Game Mode leaks ~8-10 MB of image caches for process lifetime
`state.rs:504` (toggle), `:699` (`invalidate_pixel_cache`)

`toggle()` never drops `pixel_bg_full` (decoded office_bg.png, 1448×1086 RGBA ≈ 6.3 MB), `pixel_bg_scaled` (~1.4 MB), scratch, paint, or the cell cache. The critic independently confirmed via grep: **`invalidate_pixel_cache` has zero app call sites** (only the playground bin calls it). One Ctrl+G peek costs ~8-10 MB resident for the rest of the session — the only standing cost Game Mode imposes while hidden. **Fix:** call `invalidate_pixel_cache()` on toggle-off; optionally keep `pixel_bg_full` if reopen latency matters.

### Low-severity perf (⚠️)

| # | Where | Issue |
|---|-------|-------|
| P8 | `compose.rs:94-96` | Sprite cache eviction is clear-ALL at >128 entries; worst case is already ~111/scale, and walk keys 4 frames for a 2-frame sprite (24 wasted slots) — one scale change can thrash the whole cache |
| P9 | `render.rs:648` | `put_line`/`blit_lines` allocate two heap Strings per painted character |
| P10 | `render.rs:634` | Status strip + hover popup rebuild all display Strings every redraw |
| P11 | `state.rs:568` | Attention/failed-id bookkeeping allocates a String per failed subagent per sync |
| P12 | `monitor.rs:16` | `trunc()` allocates a String per character to measure width; marquee rebuilt per paint |

---

## 3. Feature: hover tooltips (Supervisor + MCP servers)

### What exists today (all verified)
- **Hit-testing exists but is desk-only:** `update_hover` (`state.rs:240-272`) rect-tests against `last_desks: [Rect;6]` captured at paint (`render.rs:34-35`); mouse routed via `agent_view/input.rs:1133-1138` with a change-only repaint throttle; Tab/Shift-Tab keyboard parity; Esc clears.
- **A tooltip painter exists:** `paint_hover_popup` (`render.rs:123-259`) — SNES-style card with border, drop shadow, edge-clamped cursor placement, drawn as a pure buffer overlay after the halfblock blit. Currently hard-coded to `DeskSlot` fields.
- **Iron rule:** hover state is deliberately excluded from `visual_fingerprint` (`state.rs:376-405`) — hover must never trigger pixel recompose (RC13 P0). All new hover work must stay overlay-only.
- **The MCP rack sprite already exists** (`sprite_mcp_server`, `sprites_pixel.rs:626-695`, LED chase animation) **but is `#[cfg(test)]`-gated and never composed** — there is currently nothing on screen to hover.

### The MCP data problem (the real work)
`McpServerInfo` (name, `McpServerDisplayStatus` {Ready/NeedsAuth/SetupRequired/Unavailable/Initializing}, tool_count, status_detail — `mcps_modal.rs:204-261`) lives **only** in `ExtensionsModalState.mcps_data`, populated only while the /mcps modal is open. Live `x.ai/mcp/server_status` pushes are **dropped when the modal is closed** (`acp_handler/mcp.rs:227-229`). Only `mcp_init_progress` {connected/total} survives outside the modal.

### Implementation plan (three steps, in order)
1. **`HoverTarget` enum + Supervisor tooltip** — *drop-in, small.* Replace desk-index hover with `HoverTarget::{Desk(usize), Supervisor, McpRack}`; capture a supervisor hit rect at paint from `SUPERVISOR_ANCHOR` (0.50, 0.28) × cover fractions (0.13w × 0.14h, `compose.rs:317-318`) for pixel mode / `layout.supervisor` for unicode; add a `SupervisorSnapshot` filled in `sync_game_mode` (model via `current_model_name()`, `turn_elapsed()`, context tokens from `agent.context_state`, waiting-on-you, active/overflow counts, branch/cwd); factor `paint_hover_popup`'s box core into `paint_popup(buf, anchor, lines)`. *Risks:* keep new fields out of the fingerprint; update Tab-cycle + hover-throttle tests.
2. **Compose the MCP rack** — *small.* Un-gate `sprite_mcp_server`, add `cached_rack` (following `cached_supervisor`), `RACK_ANCHOR` ≈ (0.92, 0.30) near the coffee prop, floor-stamp + blit in the ambient-props block. LED animation is free if `active = any desk working` (those states already sample the tick bucket) — just hash the derived bool for the idle↔active edge. Needs a small unicode fallback glyph or explicitly no rack hover in Compact.
3. **Per-agent MCP status cache + rack tooltip** — *medium, new infra.* Add `AgentView.mcp_servers_cache`; populate from `TaskResult::McpsListLoaded` (always, not just modal-open) and patch it in `handle_mcp_server_status` *before* the modal-closed early-return (pushes are on by default, `mcp.rs:11-19`); trigger an initial `Effect::FetchMcpsList` from the tick path when Game Mode is open and cache is None (reuse `agent_has_pending_mcps_fetch`); tooltip renders status glyph + name + `label()` + tool_count + `truncate_status_detail`, falling back to init-progress counts during startup. *Risks:* `handle_mcp_server_status` has a documented session-routing/no-op contract (`mcp.rs:177-193`) — the cache write must preserve child-session drop semantics; update `acp_handler/tests/mcp.rs`.

---

## 4. Feature: sprite animations — system constraints + ranked proposals

### Constraints that gate everything (verified)
- **Idle-freeze invariant:** `pixel_needs_tick_frame` + forced `tick=0` freeze all pixels when idle/thinking. Two shipped animations are already dead because of it: supervisor idle coffee steam (`sprites_pixel.rs:608-613`) and the thinking-bubble blink (`:467-473`, pinned by `compose.rs:457`). Ambient animations need a slow hash bucket (e.g. `tick/32` ≈ every ~5 s) or must stay frozen.
- **Cache cap thrash (do this first):** 128-entry clear-all cache is already at ~111 worst case, and walk/supervisor keys over-enumerate frames (4 keyed, 2-3 used). Quantize frame in the cache key to the sprite's real frame count and evict by tag/scale instead of `clear()` **before** adding any new sprite family.
- **Fix BUG-2 first:** at effective ~6 Hz, a 400 ms Celebrate shows ~1 frame; most proposals below assume the documented ~12 Hz.
- Every new animated input must be hashed into `visual_fingerprint` (quantized, per the `anim_t/20` pattern) or it freezes; over-hashing regresses RC13 perf.
- Fixed compose order (props→supervisor→desks) means free-roaming actors need a y-sorted blit pass or margin-restricted paths; FX must be ≥2 px to survive the Nearest downsample.

### Proposals, ranked by fun-per-effort
| # | Animation | Feasibility | Effort | Key mechanism / cost |
|---|-----------|-------------|--------|----------------------|
| 1 | **Fail-state debug-rage** (head-in-hands + red error monitor) | drop-in | S | New pose fn, 2 frames; FailBeat already recomposes — zero new perf cost; +12 cache keys |
| 2 | **True celebrate pose** (\o/ + confetti burst) | drop-in | S | 2 cached frames + procedural FX from `anim_t`; needs BUG-2 fix or ~1 s duration |
| 3 | **Papers-flying handoff FX** | drop-in | S | Pure canvas FX from already-hashed `anim_t`; zero cache keys |
| 4 | **Monitor glow flicker on working desks** | drop-in | S | Baked into existing 4 typing frames; zero new keys, zero fingerprint changes |
| 5 | **MCP rack activity bursts on real tool calls** | moderate | M | Un-gate existing sprite; `rack_active_until` armed when any desk's `tool_calls` increments; best "office reacts to real work" payoff |
| 6 | **Door swing on spawn/exit** | drop-in | S | 2-frame door at existing door anchor; pure function of already-hashed inputs — no fingerprint change |
| 7 | **Coffee-sip idle for thinking desks** | moderate | M | Adds the `tick/32` slow ambient bucket; revives both dead animations; deliberately relaxes idle-freeze (~0.2 Hz recompose budget, tests must change) |
| 8 | **Office-wide golden wave on WORK FINISHED** | moderate | S | 1.5 s one-shot, ~10 recomposes per success, then re-freezes |
| 9 | **Typing cadence tied to token throughput** | moderate | M | `prev_tokens` delta → busy bucket → per-desk frame divisor; doubles recompose rate only while streaming |
| 10 | **Supervisor pacing while Waiting** | new-infra | L | First free-roaming actor: hits z-order gap + relaxes idle-freeze |
| 11 | **Roomba/cat floor wanderer** | new-infra | M | Clever perf design: only advances while room already animates, docks when idle — zero extra recomposes by construction |
| 12 | **Real day/night wall clock + hour tint** | moderate | S | `(hour, minute/10)` hash ≈ ≤6 recomposes/hour idle; replaces the fake tick clock (which BUG-2 currently runs at half speed) |

Also free wins: per-desk frame offset (`frame + desk_index`) so desks don't type in lockstep — pure function of desk index, zero cache/fingerprint impact.

---

## 5. Audit coverage gaps (critic findings)

- **Not audited:** `agent_view/mod.rs` and `session.rs` game-mode surface (trivial but unstated), Windows-specific terminal behavior (conhost vs Windows Terminal halfblock rendering, resize events), dashboard `state.rs` action passthrough. Nothing was compiled or run — findings are static analysis; `cargo test -p xai-grok-pager` should accompany any fixes.
- **Doc drift:** `docs/KNOWN_ISSUES.md:20` claims the RC13 anim-cadence fix delivers ~10-12 Hz; BUG-2 shows ~6 Hz. Update alongside the fix.
- **Playground drift:** `bin/game_mode_playground.rs` hard-codes `GameTier::Comfort` (lines 137, 158) and bypasses `render_game_mode`/`compute_layout` entirely — it cannot reproduce BUG-1 or tier behavior. `examples/export_game_mode_preview.rs:71` has a dead output path + cwd-relative PNG write.
- **Cross-finder inconsistency resolved:** the 90 ms/83 ms beat was reported at three severities; critic confirmed the mechanism and it is treated as one MED bug (BUG-2) here.

## 6. Suggested fix order

1. **BUG-1** double-peel (small, kills a destructive boundary behavior) + **BUG-2** tick gate (one constant; also fixes the wall clock and makes half of PERF-6's waste disappear) + update KNOWN_ISSUES.md.
2. **PERF-1/2/3** idle discipline: tick-demand gating, generation counter, tick-path throttle. Together they take "office open + idle" from ~12-30 Hz churn to ~0.
3. **PERF-7** cache release on toggle-off (three lines), **PERF-5** buffer reuse, **BUG-3** ExitDoor path (small, very visible), **BUG-4** unicode HUD refresh.
4. Sprite-cache key quantization + eviction (prereq for all new sprites), then hover step 1 (Supervisor tooltip) and animations #1-4/#6.
5. Hover steps 2-3 (rack + MCP cache) together with animation #5 — they share the rack compositing work.
6. PERF-4 dirty-rect compositing only if profiling shows compose cost matters after the above; the Handoff pinned-t fingerprint fix is a cheap standalone slice of it.

---

## Deferred — animation #10, supervisor pacing

Skipped deliberately after measurement, not for lack of effort. The audit named z-order and event-loop wakeups as
the blockers; both turned out to be non-issues (a horizontal pace stays in the supervisor's own depth band, and
`SupervisorPhase::Waiting` implies an occupied desk, which already holds the loop on `Slow`). The real blocker is
the asset:

1. `sprite_supervisor` is not a figure — it is 34×30 px of boss **and his wide executive desk baked into one
   image** (`// Wide executive desk` is the first thing it draws). Pacing it would walk the desk around the room.
2. He does not stand on floor. Compose stamps a 0.13w × 0.14h carpet patch over the mockup's bare **wall** at
   (0.50, 0.28); the room's real floor does not start until ~0.35h.
3. He does not fit on that patch. At the two smallest office stages the sprite already overhangs it; at the most
   generous there is ±7 px of slack — under 2.5 terminal cells of travel. Widening the patch to a pace-worthy
   ±0.12w stamps carpet over the mockup's own picture frame (0.556–0.614w) and left plant (to 0.41w).

Doing #10 properly therefore means splitting the supervisor sprite into a desk prop plus a figure, re-anchoring
both, authoring gold-horned walk frames, and re-authoring `office_bg.png` to give the boss a real floor bay — an
asset change plus a re-pin of every placement test, not a code batch. What #10 wanted (a free-roaming actor that
proves the office is alive) is delivered by #11, the floor robot, on real floor and at provably zero cost.
