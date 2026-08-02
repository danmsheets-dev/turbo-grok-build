# Game Mode RC11 — Turbo Harness Process Report

**Date:** 2026-08-02  
**Workstream:** Game Mode (terminal-native office + sprites)  
**Artifacts:** `docs/design-game-mode-rc11.md`, `docs/superpowers/plans/2026-08-02-game-mode-rc11.md`, `views/game_mode/*`

---

## What shipped (code)

| Area | Status |
|------|--------|
| Design + plan docs | Done |
| `views/game_mode` (layout, state, sprites, wall, monitor, render) | Done |
| Resize tiers + letterbox + Compact fallback | Done + unit tests |
| Slot map, handoff/spawn/fail beats, wall display | Done |
| `Shift+G` / `ActionId::ToggleGameMode` | Done (before pane routing) |
| Composer retained (scrollback region only replaced) | Done |
| `TickDemand::Slow` while Game Mode open | Done (P0 fix) |
| Shared layout tier for sync+render | Done (P1 fix) |
| Clear stale hit targets / skip timeline under Game Mode | Done |
| Attention TTL + door-queue prune | Done |
| Playground bin `game-mode-playground` | Done |
| Unit tests `views::game_mode::*` | 11 passing (post-fix re-run) |
| Full interactive polish (pathfinding, permission WAITING ON YOU) | Partial / later |

---

## How the Turbo harness helped

1. **Ratatui already in-tree** — No greenfield TUI stack; `xai-ratatui-inline` / pager patterns made Game Mode a *view*, not a rewrite.
2. **Clear action registry pattern** — `ActionId` + `defaults.rs` + `When` is the right place for Shift+G; matches ToggleTasks/Todos.
3. **`SubagentInfo` is rich enough** — elapsed, tokens, tools, activity, status already exist; no new protocol for RC11 spectator.
4. **Playground bin convention** — `todo-pane-playground` etc. gave a proven way to iterate art without full agent boot.
5. **Code-reviewer subagent** — Found real P0 (tick freeze) and P1s (tier off-by-one, hit targets, attention sticky) that would have been painful in manual play only.
6. **`developer_log` tool** — Structured product friction capture (Shift+G vs vim GotoBottom; tick_demand docs gap) without polluting chat-only feedback.
7. **Workspace monorepo** — One `cargo test -p xai-grok-pager --lib views::game_mode` path once the compile graph warms.

---

## Friction / what could improve

### 1. Cold compile of `xai-grok-pager` is very heavy (~5–7+ min)

Implementing a pure UI module still forces shell/tools/voice/workspace rebuilds when the cache is cold or locks wait.  
**Improve:** Document “warm incremental” expectations; optional feature-gated playground crate that depends only on `game_mode` + ratatui for art iteration; package-cache lock wait messaging in the harness.

### 2. PowerShell wraps cargo stderr as `NativeCommandError` (exit 1)

Even when tests all pass, agent tooling can report **failed** because cargo writes warnings to stderr.  
**Improve:** Turbo shell should treat cargo success by exit code of the process without PowerShell error-record promotion, or use `$ErrorActionPreference` / `2>&1 | Out-String` conventions in the boot card.

### 3. Animation requires dual registration (draw + `tick_demand`)

Easy to ship a beautiful view that freezes when idle.  
**Improve:** Docs next to `TickDemand`; maybe a trait `AnimatedView { fn tick_demand(&self) -> TickDemand }` scanned from agent views.

### 4. Keychord collisions are silent

`key!('G')` and `key!('G', SHIFT)` normalize to the same shortcut; vim GotoBottom and Game Mode collide. Discovered by reading normalize_case, not by a registry lint.  
**Improve:** Startup or test-time **duplicate chord detector** across contexts (or at least same-context + cross-context notes in shortcuts help).

### 5. Exhaustive `ActionId` matches are scattered

Adding one variant requires several match sites (`resolve_action`, dashboard, …). Compile helps, but discoverability is weak.  
**Improve:** Central `ActionId` → handler table, or `#[non_exhaustive]` + default arms only where safe.

### 6. Subagent isolation vs shared monorepo

`isolation=worktree` is great for parallel edits but costly when the child needs the same multi-GB target dir; we used `isolation=none` for review.  
**Improve:** Document when to use none vs worktree for “read-only review of parent tree” vs “implement in parallel”.

### 7. No lightweight visual golden path in CI for TUI

Unit tests cover layout/state; they cannot assert “looks fun”. Playground is manual.  
**Improve:** Optional ratatui `TestBackend` snapshot for wall title + desk occupancy ASCII.

### 8. Brainstorming skill vs user “just implement”

The design→plan→approve loop was valuable for scaling/handoff rules, but once the user said “write the plan and start implementing,” forcing more gates would slow delivery.  
**Improve:** Explicit “execute mode” after approval that skips re-asking execution style.

### 9. Git / commit workflow not used in this session

Workspace was treated as a working tree without a formal commit gate; multi-file features benefit from checkpoint commits the plan skill expects.  
**Improve:** Auto-suggest commits after green unit tests for the feature package.

---

## Product issues filed (`developer_log`)

1. **Shift+G steals vim GotoBottom** — `feature_gap` / p2  
2. **tick_demand docs for animated views** — `docs_gap` / p3  

Review: `turbo issues list` / `turbo issues export`.

---

## Recommended next steps (post-RC11 MVP)

1. Wire permission queue → `WAITING ON YOU` wall mode.  
2. Zero timeline column width while Game Mode open (free width).  
3. Theme-aware palette tokens.  
4. PTY e2e: Shift+G toggles, composer still sends.  
5. Reduce-motion setting.  
6. Desk select / Enter open subagent (spectator → control room).

---

## Bottom line

Turbo’s **existing Ratatui pager architecture, subagent telemetry, action registry, and review subagents** made Game Mode feasible as a focused feature rather than a fork. The main harness costs were **compile weight**, **animation tick registration**, and **silent keybind collisions**. Addressing those three would make the next “fun TUI surface” much faster and safer.
