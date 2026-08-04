# Game Mode — Double Independent Audit (Performance + Bugs)

| Item | Content |
|------|---------|
| Date | 2026-08-03 |
| Focus | Lag after RC12 visual densify + RC13 fingerprint work |
| Auditors | Grok 4.5 · GPT-5.6 Terra (Terra Max) |
| Mode | Independent explore, read-only |

---

## 1. Scorecard

| Auditor | Perf score | One-line |
|---------|------------|----------|
| **Grok 4.5** | **4 / 10** | Compose fingerprint is fine; lag is **every-paint downsample + Fast tick while working** |
| **Terra Max** | **3 / 10** | Same dominant path; also **Slow ticks may not redraw**, freeze animation when idle |
| **Synthesis** | **~3.5 / 10** for busy office | RC13 fixed recompose; **did not fix paint cost or tick ownership** |

---

## 2. Root cause of “lag came back” (consensus)

Both auditors agree:

1. **RC13 fingerprint correctly skips high-res recompose** on idle/hover and freezes idle poses.
2. **Every paint still pays the expensive halfblock path**:
   - Cached frame is at `PIXEL_SCALE` (default **3×**) resolution.
   - `paint_halfblock_rgba` only skips resize when `src == target` exactly → with scale 3 it **always** `imageops::resize` + rewrite all cells.
3. **Game Mode does not stay on `TickDemand::Slow` when agents work**:
   - `tick_demand()` returns **Fast** first if `!session.is_idle()` **or** (Terra) hidden **Tasks pane** `needs_tick()` while subagents run.
   - Slow branch (`game_mode.open → Slow`) is never reached in the common “office with workers” case.
4. **Mouse move always returns `Changed`** even if hover desk unchanged → full office repaint flood.

So: denser RC12 art + scale-3 compose made each paint expensive; Fast demand multiplies paints; fingerprint only stops *compose*, not *paint*.

---

## 3. Hot path (what still runs every paint)

1. `snapshots_from_subagents` (clone/sort strings)  
2. `sync_from_snapshots` (seat map / phases)  
3. `ensure_pixel_frame` — often **hit** (good)  
4. **`paint_halfblock_rgba` — always**: allocate/resize scale-3 → cell res, fill every `▀`  
5. Overlays: focus ring, status strip, hover popup  

Compose miss (working desks / supervisor typing): full `bg_scaled.clone()` + sprite blits (~3×/s via `tick/4`).

---

## 4. Performance findings (merged)

| Sev | Issue | Grok | Terra | Evidence |
|-----|--------|------|-------|----------|
| **P0** | Full **resize + cell paint every draw** (scale≠1) | yes | yes | `halfblock.rs` `use_direct`; `compose` scale 3 |
| **P0** | Open Game Mode still gets **Fast** when turn/tasks need tick | yes | yes | `app_view.rs` Fast before Slow |
| **P0** | (Terra) **Slow scheduled tick may not draw** → animation freeze if nothing else dirties | — | yes | `event_loop` / `tick` vs paint-side `sync_game_mode` |
| **P1** | Mouse move always `Changed` | yes | yes | `input.rs` |
| **P1** | `bg_scaled.clone()` + sprite **clone** on compose miss | yes | — | `compose.rs` |
| **P2** | Snapshot rebuild every paint | — | yes | `mod.rs` / `state.rs` |
| **P2** | Default PIXEL_SCALE=3 heavy; cache clear at 128 | yes | — | `sprites_pixel` / `compose` |

---

## 5. Correctness findings (merged)

| Sev | Issue | Grok | Terra |
|-----|--------|------|-------|
| **P0** | Animation only advances on paint; Slow-only can freeze | — | yes |
| **P1** | Ctrl+G tests/help still say **Tasks** | yes | — |
| **P1** | `WallMode::WaitingOnYou` never set | yes | yes |
| **P1** | Pixel **SpawnWalk** has no motion (Unicode does) | yes | yes |
| **P1** | Failure `attention_until` re-armed every sync while failed desk exists | — | yes |
| **P1** | Playground bin stale (`mcp_active` / API) | — | yes |
| **P2** | Keyboard focus cleared by mouse move (shared hover) | — | yes |
| **P2** | Thinking = activity contains `"think"` | yes | — |

---

## 6. Concrete fix plan (ordered)

### Must-do for lag (P0)

1. **Cache terminal-resolution frame** next to high-res composite  
   - On fingerprint miss: compose high-res → **downsample once** → store paint buffer.  
   - On paint: pass paint buffer so `use_direct` is true (no `imageops::resize` every frame).  
   - Optional: cache halfblock cell colors and skip full cell rewrite when fp + hover overlays unchanged.

2. **Own Game Mode cadence**  
   - When Game Mode open: do **not** inherit Fast solely from hidden Tasks / streaming unless another **visible** surface needs Fast.  
   - Cap office paints to ~10–12 Hz (or skip paint if &lt; Slow interval and only office animation).  
   - Advance `tick_anim` / state on **tick**, not only as side-effect of paint (fixes Terra freeze).

3. **Hover dirty only when desk/popup changes**  
   - Compare previous `hover_desk` / popup cell before `InputOutcome::Changed`.

### Should-do (P1)

4. Avoid full `bg_scaled.clone()` — copy_from / dirty rects / double-buffer.  
5. Sprite cache as `Arc<RgbaImage>` (no clone per blit).  
6. Failure attention on **transition**, not every snapshot sync.  
7. Implement or delete pixel SpawnWalk; fix Ctrl+G help/tests; wire or remove WaitingOnYou.  
8. Fix/remove stale playground binary.

### Nice (P2)

9. Default `PIXEL_SCALE=2` on large stages / Windows, keep 3 via env.  
10. Split keyboard focus from mouse hover.  
11. Don’t rebuild full subagent snapshots every paint if map unchanged.

---

## 7. Suggested measurements / tests

| Check | Pass criteria |
|-------|----------------|
| Paint hit (fp unchanged) | &lt; ~1–2 ms @ ~160×40 after paint-buffer cache |
| Compose miss | Budget separately; no resize every paint |
| Demand with 6 workers + Game Mode | Prefer Slow for office unless visible chrome needs Fast |
| Slow tick alone | Advances celebrate/walk/fail without input |
| Hover flood | No full paint if desk focus unchanged |
| Failed desk | `attention_until` expires after 12s once |
| Ctrl+G | Help/tests = ToggleGameMode |

Manual: `game-mode-playground` Idle vs Full; `GROK_GAME_PIXEL_SCALE=2` vs `3`.

---

## 8. What RC13 already got right (do not regress)

- Fingerprint excludes pure hover / idle seated `anim_t` noise.  
- Hover ring/popup as buffer overlays (not full recompose).  
- BG rescale only on size/scale change.  
- Idle supervisor frozen at frame 0.  
- Unit tests for idle/hover fingerprint stability.

---

## 9. Bottom line

**Lag is not “fingerprint broken.”** It is **(A)** every-paint high-res→cell **resize+fill**, and **(B)** **Fast** tick rate whenever the session/tasks are busy, multiplied by denser scale-3 art. Fix paint cache + tick ownership first; then hover dirty-guard and clone costs.

---

_End of double audit._
