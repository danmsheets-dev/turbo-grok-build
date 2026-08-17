# RC2 Plugin ↔ Turbo Harness Plan

> **For agentic workers:** implement inline in this session. Game Mode / app
> work is out of scope. This is Turbo `1.0.0-rc.2` harness completion plus
> Grok Build plugin `0.6.8` consumption of that surface.

**Goal:** Isolated plugin delegates (and in-product subagents) can run
configured Blender/Godot binaries on Windows, list the live children they
just spawned, and seed worktrees without a HEAD race — and the plugin
stops paying three CLI probes per spawn.

**Architecture:** Confine already models `blender`/`godot` by basename, but
PowerShell `& "C:\Program Files\...\blender.exe"` never reaches that
classifier (bash grammar → `shell-unparseable`). Recover those
invocations with a Windows cmdline tokenizer, fail-closed for any other
program. Plugin consumes `turbo version --json` as a single identity card.

**Tech Stack:** Rust (`xai-grok-workspace` confine, `xai-grok-pager`
subagent CLI, `xai-fast-worktree`), Node stdlib plugin bridge.

---

## Evidence (logs, not guesses)

Open **must_have** FR:

- `fr_019ffb2b609f797084ac8e5ab3ef186e` — allowlist Blender/Godot binaries
  from worktree confine (paths with spaces).

Open incidents (same bug, many fingerprints):

- `inc_019ff2a5ce7071b194bde290342dca5b` — `& "C:\Program Files\Blender Foundation\Blender 5.2\blender.exe" …` is `shell-unparseable` (still firing 2026-08-13).
- `inc_019ffb2b60a07b12bee4c4b326141fa4` (p1) and ~14 siblings — Program Files blender treated as path-outside-root / unparseable.
- `inc_019ffb174f717d20956e283b4e6a0a88` — `turbo subagent list` shows discarded agents as `STATUS=running`, omits live children.
- `inc_019ff239b4d879f3940cbcf448f2c6b8` (p1) — `git reset --hard HEAD` / `Could not parse object 'HEAD'` on concurrent worktree seed.

Shipped / out of scope for this pass:

- Keep-N / densify residual stems (`inc_019ff2b1…`) — game/app coordinator, not harness.
- Luna model slug — provider catalog, not plugin↔turbo.
- Isolation write-to-parent (`inc_019ff283…`) — rc.1 already fail-closed CWD assert; re-verify only if audit confirms still live.

## Tasks

### T1 — Confine: PowerShell / cmd engine invocations

**Files:** `crates/codegen/xai-grok-workspace/src/permission/shell_access.rs`

- [x] Add tokenizer + recovery before `Unparseable`; peel `cmd /c`.
- [x] Tests for `& "…\\blender.exe"`, single quotes, `cmd /c blender …`.

### T2 — `turbo subagent list` honesty

**Files:** `crates/codegen/xai-grok-pager/src/subagent_cmd.rs`

- [x] `--all` flag; default = newest session for this cwd, hide discarded+cleaned.
- [x] Relabel `running` + cleaned/discarded as `stale`.

### T3 — Worktree reset uses a real SHA

**Files:** `crates/codegen/xai-fast-worktree/src/git/checkout.rs`, `worktree/execute.rs`

- [x] Prefer source HEAD SHA; retry once on `Could not parse object`.

### T4 — Plugin: one identity probe

**Files:** `plugins/grok-build/scripts/lib/grok.mjs`, `grok-bridge.mjs`, tests

- [x] Cache `version --json` identity; drive compat / prefixes / confine from it.
- [x] `/check` and doctor surface product + features.
- [x] Bump plugin to 0.6.8.

### T5 — Changelog

- [x] Append RC2 harness notes on Turbo; plugin 0.6.8 notes.

### T6 — Land harness ignore + JSON union-merge

- [x] Ignore `.grok-subagent-live` / `.grok/` at land allowlist + do not copy to parent.
- [x] CLI `--json-union-by` + tool `json_union_by`; manifests always merge by name.

### T7 — temp-grok self-clean

- [x] Report + reclaim TEMP-root harness prefixes; nest plugin temps under `%TEMP%/grok`.
- [x] Post-subagent `--if-low-space` still sweeps `temp-grok` when space is OK.

### T8 — Session-configured Blender/Godot

- [x] `GROK_BLENDER` / `GROK_GODOT` (and `*_PATH`) match even when basename is not `blender`/`godot`.
