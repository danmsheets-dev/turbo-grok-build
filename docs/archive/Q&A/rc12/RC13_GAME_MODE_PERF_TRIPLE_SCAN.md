# Game Mode — Triple Performance Rating Scan (post dual-audit fixes)

| Item | Content |
|------|---------|
| Date | 2026-08-03 |
| Focus | Lag after RC13 dual-audit implementation |
| Auditors | **Grok 4.5** · **Luna High** (`openai-codex/gpt-5.6-luna`) · **Nemotron Ultra 550B** |
| Mode | Independent explore, read-only |
| Prior baseline | Dual-audit synthesis **~3.5 / 10** (`RC13_GAME_MODE_PERF_DOUBLE_AUDIT.md`) |

---

## 1. Scorecard

| Auditor | Score | One-line |
|---------|-------|----------|
| **Grok 4.5** | **7 / 10** | Dual-audit P0s are in code; residual lag is full halfblock cell rewrite @ Slow + compose clone on miss |
| **Luna High** | **8 / 10** | Dominant resize + Fast amplification removed; steady cost is O(cells) raster + periodic recompose |
| **Nemotron Ultra 550B** | **4 / 10** | Argues paint cell-loop + Fast inheritance still dominate; **over-penalizes** (see §3 corrections) |
| **Synthesis** | **~7 / 10** busy office · **~8.5 / 10** idle room | **+3.5 vs dual-audit baseline** for the common “office + workers” path |

### Score movement

| Scenario | Dual-audit (pre-fix) | This scan (post-fix) |
|----------|----------------------|------------------------|
| Idle office | Fingerprint OK; paint still resized | Stable fp + direct paint + Slow (~12 Hz) |
| Busy office (6 workers) | Fast ~30 Hz × scale-3 resize every paint | Slow ~12 Hz × direct terminal-res paint; recompose ~tick/4 |
| Overall | **~3.5 / 10** | **~7 / 10** |

---

## 2. Consensus: what is actually fixed

All three auditors (with Nemotron partially disagreeing on labels) align on this **verified** set:

| Prior dual-audit P0/P1 | Status | Evidence (synthesis) |
|------------------------|--------|----------------------|
| Terminal-res `pixel_paint` / no scale-3 resize every paint | **Fixed** | `state.rs` `ensure_pixel_frame` downsamples once; `paint_halfblock_rgba` `use_direct` when dims match |
| Game Mode Slow when workers / turn running | **Fixed for Tasks + turn + scrollback** | `tick_demand`: `(!game_open && tasks/scrollback/!idle)`; open → `Slow` |
| Slow tick freezes animation | **Fixed** | `AppView::tick` → `sync_game_mode` + `needs_redraw = true` |
| Hover always `Changed` | **Fixed** | `input.rs`: `update_hover` → `Changed` only if dirty, else `Unchanged` |
| Sprite clone per blit | **Fixed** | `compose.rs` `Arc<RgbaImage>` cache |
| Fingerprint idle/hover freeze | **Still solid** | Tests + `visual_fingerprint` exclusions |

**Nemotron corrections (supervisor re-read):**

1. **Hover dirty guard is fixed** — Nemotron said “not fully read input”; code at `input.rs:1106–1110` returns `Unchanged` when `update_hover` is false.
2. **Fast from Tasks/turn/scrollback is fixed when Game Mode open** — gated with `!game_open`. Nemotron’s “Fast still always” overstates residual chrome (permissions, toast, todo badge, etc.), which is **by design** for visible UI, not the dual-audit P0 “hidden Tasks / streaming Fast storm.”
3. **Score 4/10 is too low** for post-fix code; structural path matches Grok/Luna ~7–8.

---

## 3. Hot path (agreed model)

### Paint HIT (fingerprint stable)

1. `sync_game_mode` (snapshots + sync; may tick_anim)
2. `ensure_pixel_frame` → early return
3. `paint_halfblock_rgba` **direct** — still **O(W×H cells)** full rewrite of `▀` + styles
4. Overlays: focus ring, status strip, hover popup

### Paint MISS (working desks / walk / celebrate)

1. Optional BG rescale (size/scale only)
2. `compose_cell_frame`: **`bg_scaled.clone()`** + sprite blits (Arc, no sprite clone)
3. One downsample → `pixel_paint`
4. Same full halfblock cell loop

### Tick

- Prefer **Slow** (~12 Hz) while Game Mode open and no visible Fast chrome
- Unconditional `needs_redraw = true` every Slow tick while open (even frozen rooms)

---

## 4. Remaining issues (merged, severity rebased)

| Sev | Issue | Grok | Luna | Nemo | Synthesis |
|-----|--------|------|------|------|-----------|
| **P1** | Full halfblock cell rewrite every paint (HIT still pays O(cells)) | yes | yes (called P0) | yes | **New dominant cost** — not dual-audit P0 “resize” |
| **P1** | Unconditional redraw every Slow tick while open | yes | — | — | Idle room still paints ~12 Hz full office |
| **P1** | Compose miss: `bg_scaled.clone()` + full recompose | yes | yes | yes | Working desks recompose ~tick/4 |
| **P1** | Double `sync_game_mode` (tick + paint) | yes | — | — | Snapshot clone/sort twice per cycle |
| **P2** | Fast still if toast/permissions/todo badge/etc. | yes | yes | yes (overweighted) | Correct UX; rare Fast storm |
| **P2** | Snapshot rebuild every sync | yes | yes | yes | Modest at 6 agents |
| **P2** | Hover dirties on every mouse **cell** move (popup follows cursor) | yes | notes intentional | wrong “unfixed” | Throttle optional |
| **P2** | Memory: full + scaled + high-res + paint buffers | — | yes | — | Acceptable |
| **P2** | Default `PIXEL_SCALE=3` | — | — | yes | Nice-to-have downshift |

**No remaining dual-audit “must-do P0” for resize/Fast-from-tasks/freeze/hover-flood.**  
Next wave is **paint dirty-skipping** and **compose alloc reuse**.

---

## 5. Ranked next fixes (synthesis)

1. **Skip halfblock rewrite when `pixel_frame_fp` HIT and only overlays change**  
   Cache last painted cell styles / dirty-rect overlays (focus ring pulse, popup). Highest residual win.

2. **Dirty-gate Slow redraw**  
   Don’t force `needs_redraw = true` every tick on frozen fingerprint; redraw on fp change, wall/status change, focus pulse edge, hover dirty.

3. **Compose canvas reuse**  
   Double-buffer / `copy_from` instead of full `bg_scaled.clone()` on miss (finish GM-P1-3).

4. **Single sync owner**  
   Tick-only or paint-only `sync_game_mode` per frame; skip double snapshot work.

5. **Optional:** `PIXEL_SCALE` default 2 on large stages; snapshot generation cache; hover popup throttle.

---

## 6. Cost model (Luna-style, consensus)

| Path | Cadence (typical busy) | Dominant work |
|------|------------------------|---------------|
| Tick demand | **Slow ~12 Hz** (not Fast from turn/tasks) | `tick_anim` O(6 desks) |
| Compose miss | ~`tick/4` when workers typing (~3 Hz) | High-res clone + blits + 1 downsample |
| Paint always | Every redraw | **O(cells)** halfblock write |
| Idle room | Slow tick + stable fp | Halfblock only (still full write if redraw forced) |

---

## 7. Auditor disagreement summary

| Topic | Who said what | Verdict |
|-------|---------------|---------|
| Overall score | 7 · 8 · 4 | **~7** (discount Nemotron P0 mislabels) |
| Resize every paint | Fixed / Fixed / “No” (conflated with cell loop) | **Resize fixed**; cell loop remains |
| Fast with workers | Fixed for dual-audit scope | Fixed; residual Fast = visible chrome |
| Hover flood | Fixed | **Fixed** (cell-level dirty remains intentional) |
| Next bottleneck | Halfblock O(cells) | **Unanimous residual #1** |

---

## 8. Bottom line

**Performance recovered substantially** after dual-audit implementation:

- Dual-audit root causes (**scale-3 resize every paint**, **Fast from hidden tasks/turn**, **Slow freeze**, **hover always dirty**) are **fixed in tree**.
- New rating: **~7 / 10** for a busy office (was **~3.5**).
- Idle room is closer to **8–9 / 10** structurally; forced Slow redraw keeps a little waste.
- Next lag is **not** “fingerprint broken” — it is **(A)** full truecolor halfblock rewrite every paint, **(B)** forced Slow redraw, **(C)** full-frame clone on compose miss.

**Confidence:** High on control flow (three independent reads + supervisor spot-check of disputed lines). Medium on absolute ms/FPS (no runtime profiler in this scan).

---

## 9. Implementation status (post-scan wave)

| Ranked fix | Status |
|------------|--------|
| 1. Skip halfblock rewrite / cell-color cache on HIT | **Done** — `HalfblockCellCache` + `paint_halfblock_cells` |
| 2. Dirty-gate Slow redraw | **Done** — `mark_redraw_dirty` / `take_redraw_dirty`; tick no longer forces paint |
| 3. Compose canvas reuse | **Done** — `compose_cell_frame_into` + `pixel_compose_scratch` |
| 4. Single sync owner | **Done** — tick owns sync; paint only if `needs_paint_sync` |
| 5. Adaptive scale-2 on large stages + hover desk throttle | **Done** — `effective_pixel_scale`; hover desk-only dirty |

_End of triple performance scan._
