# Game Mode Fix Plan (dual audit)

Source: [`RC13_GAME_MODE_PERF_DOUBLE_AUDIT.md`](./RC13_GAME_MODE_PERF_DOUBLE_AUDIT.md)

## Goals
1. Remove lag when office is open with workers (P0 paint + tick).
2. Advance animation on Slow ticks (no freeze).
3. Fix correctness bugs (SpawnWalk, attention, WaitingOnYou, help, playground, focus).

## Implementation

| ID | Fix |
|----|-----|
| GM-P0-1 | Cache **terminal-res** `pixel_paint` buffer; halfblock uses `use_direct` |
| GM-P0-2 | Game Mode open → prefer **Slow**; exclude hidden Tasks Fast demand |
| GM-P0-3 | `AppView::tick` advances Game Mode anim and requests redraw |
| GM-P1-1 | Hover returns dirty only if desk/popup changed |
| GM-P1-2 | Sprite cache as `Arc<RgbaImage>` |
| GM-P1-3 | Reuse compose canvas (copy_from BG) |
| GM-P1-4 | Attention on **new** failed id only |
| GM-P1-5 | Pixel SpawnWalk uses `anim_t` + walk sprite |
| GM-P1-6 | `WaitingOnYou` when permission pending / waiting supervisor |
| GM-P1-7 | Keyboard focus separate from mouse hover |
| GM-P1-8 | Ctrl+G help strings; fix stale tests if any |
| GM-P1-9 | Fix playground binary |

Status: **implemented** in-tree (RC13 dual-audit wave).

## Verification

```powershell
cargo test -p xai-grok-pager --lib views::game_mode -- --test-threads=4
cargo test -p xai-grok-pager --lib actions:: -- --test-threads=4
cargo check -p xai-grok-pager --bin game-mode-playground
```
