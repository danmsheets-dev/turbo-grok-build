# Hyper known issues

Living list of fork-specific gaps, fixed items, and intentional limits.
Update this file when closing an issue or shipping a release.

Last reviewed: 2026-08-03 (RC13 Game Mode perf + worktree soft-preserve docs).

## RC16 — Game Mode audit fixes

Source landed on `rc16-game-mode`. Audit: [RC16_GAME_MODE_AUDIT.md](./RC16_GAME_MODE_AUDIT.md).

| Topic | Notes |
|-------|--------|
| Tick/paint tier off-by-one (BUG-1) | `sync_game_mode` now takes the **stage** (status strip already peeled) instead of a paint area it peeled a second time. Callers derive it from `game_mode::stage_rect`, so the tick tier equals the painted tier by construction. Previously a 19-row paint area painted Normal while every tick ran the Compact snap-complete branch, permanently killing walks/celebrates/handoffs at that height. |
| Open office no longer pins the loop (PERF-1) | `tick_demand` returns `Slow` for Game Mode only when the room can actually animate — `GameModeState::needs_animation_tick()` (seated desk, Working/Reviewing supervisor, handoff/door queue, armed attention window, pending redraw, never-synced room) plus two live wake checks (turn running, any subagent still running). A frozen room now parks at `TickDemand::None` instead of waking ~12×/sec forever while the view stays open. Repaints are unaffected (the paint path re-syncs on its own); the only visible cost is the decorative unicode wall clock, which is tick-derived and freezes while the room is parked. |
| Exit walk goes to the door (BUG-3) | After a handoff the actor used to teleport 45% back down the desk→supervisor line and then walk *into* the supervisor again, because every walk phase shared that one interpolation. `ExitDoor` now mirrors the spawn entry — rug → door, dropping back to its own desk row — via `compose::walk_position`. Handoff is pinned on the rug, so its `anim_t` no longer enters the pixel fingerprint (it forced 500 ms of full recomposes of identical frames). |
| Compact/Unicode desk HUD refresh (BUG-4) | A thinking-only room deliberately freezes the pixel office (the sprite is static), but the Compact and Unicode tiers paint per-desk monitor text — elapsed timer, tokens, tool calls, scrolled activity — whose data the sync refreshes every second. Nothing marked those dirty, so an on-screen `01:23` could sit frozen for minutes. Those tiers now repaint once per ~1 s (`HUD_REFRESH_TICKS`) while any desk is occupied. The pixel idle-freeze is unchanged, and the pixel office now repaints on `tick/4` sprite-bucket edges instead of every tick while desks are only typing (PERF-6) — same frames, a quarter of the blits. |
| Anim cadence ~6 Hz → ~12 Hz (BUG-2) | The `tick_anim` gate is derived from `SLOW_TICK_INTERVAL` (minus an 8 ms jitter margin) instead of a hardcoded 90 ms that sat above the 83 ms interval and dropped every other tick. Restores the documented ~12 Hz and un-halves the unicode wall clock (`tick/12`). The ~2× demand-dependent swing is gone, but a residual difference remains by design: the fixed 75 ms gate passes every Slow tick (~12 Hz) and every third Fast tick (~10 Hz), so the office runs slightly *slower*, not faster, while a toast or modal is up. A unit test pins `gate <= SLOW_TICK_INTERVAL`. |

## RC13 — Workspace Tree inject + tombstone + Game Mode perf

Source landed for **0.2.114-r14** (rebuild/install required to take effect in the binary).
Prior line: r13 Workspace Tree inject + Game Mode perf; r14 web_fetch + workflow routing.

| Topic | Notes |
|-------|--------|
| Soft-preserve default | Completed isolation worktrees are snapshotted and **kept** by default (`GROK_SUBAGENT_SOFT_PRESERVE=0` deletes after snapshot). See user-guide `16-subagents.md`. |
| Tombstone writes/shell | Missing CWD / confine root → `cwd_missing` / `worktree_tombstone`; shell preflight; no write success when path gone. |
| Live marker prune | RUNNING trees with fresh `.grok-subagent-live` are never keep-N pruned. |
| Workspace Tree inject | Budgeted `<workspace_tree_card>` on session prompt; tools + `/tree` + `turbo tree`. Docs: `docs/workspace-tree.md`. |
| Keep-N prune | Soft-preserved peers pruned by `GROK_SUBAGENT_KEEP_N` (default 3; `0` = age-only) + free-space guard (`GROK_MIN_FREE_GB`, default 40). |
| Game Mode perf | Terminal-res `pixel_paint` cache (no per-paint scale-3 resize); Game Mode open prefers `TickDemand::Slow` (not Fast from hidden tasks/turn); anim advances on `AppView::tick`; hover dirty-if-changed; dual-audit P1 bugs fixed (SpawnWalk, WaitingOnYou, attention, focus, playground). **Correction (RC16):** the anim step landed behind a hardcoded 90 ms gate sitting *above* the 83 ms `SLOW_TICK_INTERVAL`, so RC13–RC15 actually animated at ~6 Hz (and the unicode wall clock ran at half real-time), not the documented ~10–12 Hz. Fixed in RC16 — see below. |
| Residual accepted | Shell confine is policy-level, not OS FS jail. True incremental tree freshness is still Phase 2. |

## Added: Auto Developer Log

Structured product-issue store for agents + runtime detectors. See
[AUTO_DEVELOPER_LOG.md](./AUTO_DEVELOPER_LOG.md).

| Surface | Notes |
|---------|--------|
| Tool `developer_log` | Agents file/dedup incidents under `~/.grok/developer-log/` |
| CLI `turbo issues list\|show\|export\|ack\|resolve\|path` | Maintainer review + export packs |
| Auto detectors | Worktree dispose without artifacts; isolation fallback; stall/timeout |
| Disable | `GROK_DEVELOPER_LOG=0` |

## Fixed in v0.2.114-r8 (RC8)

| Topic | Fix |
|-------|-----|
| NVIDIA stream deser `null` vs `u32` | Null-tolerant Chat Completions usage/index/tool_calls |
| Subagent hang without timeout | `timeout_ms` + budget monitor; stall on no progress |
| Worktree ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œdisappearedÃƒÂ¢Ã¢â€šÂ¬Ã‚Â | `changes.patch` + `snapshot_ref` + `worktree_state` on completion; `retain_worktree` |
| Parent cannot merge child work | `diff_subagent` / `land_subagent` tools or `turbo subagent land` |
| NVIDIA `prompt_cache_key` 400s | Platform defaults + opt-in stamp only |
| Catalog EOL / Nano token overflow | Hide EOL; clamp Nano 9B; `agent_ready` / max_parallel on compat |
| Deep multi-agent audit | `/deepaudit` + `continuous-improve` workflows |

## Open after RC8

| ID | Severity | Topic | Notes |
|----|----------|--------|--------|
| Worktree naming | low | Not always `git worktree list` | Implementation may still use clone/linked sandbox; recovery is via snapshot ref / patch |
| Ultracode free-text keyword | deferred | Auto-workflow on keyword | RC9; slash `/ultracode` / `/deepaudit` already ship |
| Fan-out `spawn_many` | done (pre-r8 residual) | `spawn_many` tool + optional wait barrier | Composes Task; coordinator max 4 FIFO queue |
| Nightly NVIDIA matrix CI | deferred | Live conformance | Unit fixtures shipped |

## Implemented (pre-r8 residual)

| ID | Topic | Notes |
|----|--------|--------|
| R1 | Progress heartbeats | `last_tool` + `last_progress_age_ms` on SubagentProgress; `land_status=pending` on dispose artifacts |
| R2 | Path allowlists | Optional `allowed_paths` on `task` spawn ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ `SubagentRequest` / `meta.json`. Non-empty allowlist: `land_subagent` refuses any path outside the prefixes (fail closed); `diff_subagent` filters shown files/diff. Paths normalized (`/` , strip `./`, reject `..` escape / absolute). Omit = unrestricted (prior behavior). |
| R3/R5 | RO isolation + ultracode | explore/plan/oracle and read-only default `isolation=none`; `/ultracode` slash alias |
| R4 | spawn_many | Fan-out Task spawns + optional wait barrier |
| R6 | Durable LoopCheckpoint | session `loops/<run_id>/checkpoint.json` mirror |
| R7 | NVIDIA fixtures | Extra null tool_call index deser fixture + BUILD_INSTALL test cmds |

## Fixed in v0.2.109

- **xAI HTTP 426 / `x-grok-client-version`.** Release CI stamps
  `GROK_VERSION` from the root `VERSION` file into the binary. The `v0.1.0`
  marketing tag set that header to `0.1.0`, which production rejects
  (minimum **0.1.202**). Releases must use the monorepo lockstep version
  (currently `0.2.110`). Upgrade with a fresh `install.sh` run.

## Open (accepted for v0.2.110)

| ID | Severity | Topic | Notes |
|----|----------|--------|--------|
| Modes | deferred | Amp-style lowÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“ultra agent modes | **ÃƒÂ§Ã‚Â¼Ã¢â‚¬Å“ÃƒÂ¥Ã‚ÂÃ…â€œ** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â [design-modes.md](./design-modes.md) Ãƒâ€šÃ‚Â§0ÃƒÂ£Ã¢â€šÂ¬Ã¢â‚¬Å¡ÃƒÂ§Ã…Â½Ã‚Â°ÃƒÂ¦Ã…â€œÃ¢â‚¬Â°ÃƒÂ¦Ã‚Â¨Ã‚Â¡ÃƒÂ¥Ã…Â¾Ã¢â‚¬Â¹ÃƒÂ©Ã¢â‚¬Â¦Ã‚ÂÃƒÂ§Ã‚Â½Ã‚Â®ÃƒÂ¥Ã‚Â·Ã‚Â²ÃƒÂ¥Ã‚Â¤Ã…Â¸ÃƒÂ¯Ã‚Â¼Ã¢â‚¬ÂºÃƒÂ¤Ã‚Â¸Ã‚ÂÃƒÂ¤Ã‚Â½Ã…â€œÃƒÂ¤Ã‚Â¸Ã‚ÂºÃƒÂ¥Ã‚ÂÃ¢â‚¬ËœÃƒÂ¥Ã‚Â¸Ã†â€™ÃƒÂ§Ã‚Â¼Ã‚ÂºÃƒÂ¥Ã‚ÂÃ‚Â£ÃƒÂ£Ã¢â€šÂ¬Ã¢â‚¬Å¡ |
| Oracle | done (Phase 0/1) | Stronger-model pin + trigger UX | spawn ÃƒÂ¥Ã‚ÂÃ…â€™ÃƒÂ¦Ã‚Â¨Ã‚Â¡ÃƒÂ¥Ã…Â¾Ã¢â‚¬Â¹ toastÃƒÂ£Ã¢â€šÂ¬Ã‚Â`/doctor` pin ÃƒÂ¦Ã‚Â£Ã¢â€šÂ¬ÃƒÂ¦Ã…Â¸Ã‚Â¥ÃƒÂ£Ã¢â€šÂ¬Ã‚Â`spawn_subagent` ÃƒÂ¨Ã‚Â§Ã‚Â¦ÃƒÂ¥Ã‚ÂÃ¢â‚¬ËœÃƒÂ¦Ã¢â‚¬â€œÃ¢â‚¬Â¡ÃƒÂ¦Ã‚Â¡Ã‹â€ ÃƒÂ¥Ã‚Â·Ã‚Â²ÃƒÂ¨Ã‚ÂÃ‚Â½ÃƒÂ¥Ã…â€œÃ‚Â° ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â [design-oracle.md](./design-oracle.md)ÃƒÂ£Ã¢â€šÂ¬Ã¢â‚¬Å¡Phase 2 harness ÃƒÂ¤Ã‚Â¿Ã‚Â¡ÃƒÂ¥Ã‚ÂÃ‚Â·ÃƒÂ¦Ã…â€œÃ‚ÂªÃƒÂ¦Ã…Â½Ã¢â‚¬â„¢ÃƒÂ¦Ã…â€œÃ…Â¸ÃƒÂ£Ã¢â€šÂ¬Ã¢â‚¬Å¡ |
| Flaky test | low | `scrollback::entry::tests::test_truncated_height_cache_hits_when_key_unchanged` | ÃƒÂ¤Ã‚Â»Ã¢â‚¬Â¦ÃƒÂ¥Ã¢â‚¬Â¦Ã‚Â¨ÃƒÂ©Ã¢â‚¬Â¡Ã‚ÂÃƒÂ¥Ã‚Â¹Ã‚Â¶ÃƒÂ¨Ã‚Â¡Ã…â€™ÃƒÂ¨Ã‚Â·Ã¢â‚¬ËœÃƒÂ¦Ã¢â‚¬â€Ã‚Â¶ÃƒÂ¥Ã‚ÂÃ‚Â¶ÃƒÂ¨Ã‚Â´Ã‚Â¥ÃƒÂ¯Ã‚Â¼Ã‹â€ ÃƒÂ§Ã‚ÂºÃ‚Â¦ 1/5 ÃƒÂ¦Ã‚Â¦Ã¢â‚¬Å¡ÃƒÂ§Ã…Â½Ã¢â‚¬Â¡ÃƒÂ¯Ã‚Â¼Ã¢â‚¬Â°ÃƒÂ¯Ã‚Â¼Ã…â€™ÃƒÂ¥Ã‚ÂÃ¢â‚¬Â¢ÃƒÂ¨Ã‚Â·Ã¢â‚¬ËœÃƒÂ¥Ã‚Â¿Ã¢â‚¬Â¦ÃƒÂ¨Ã‚Â¿Ã¢â‚¬Â¡ÃƒÂ¯Ã‚Â¼Ã¢â‚¬ÂºÃƒÂ§Ã¢â‚¬â€œÃ¢â‚¬ËœÃƒÂ¤Ã‚Â¼Ã‚Â¼ÃƒÂ¥Ã‚Â¹Ã‚Â¶ÃƒÂ¨Ã‚Â¡Ã…â€™ÃƒÂ¦Ã‚ÂµÃ¢â‚¬Â¹ÃƒÂ¨Ã‚Â¯Ã¢â‚¬Â¢ÃƒÂ©Ã¢â‚¬â€Ã‚Â´ÃƒÂ¥Ã¢â‚¬Â¦Ã‚Â¨ÃƒÂ¥Ã‚Â±Ã¢â€šÂ¬ÃƒÂ¥Ã‚Â¤Ã¢â‚¬â€œÃƒÂ¨Ã‚Â§Ã¢â‚¬Å¡/ÃƒÂ¤Ã‚Â¸Ã‚Â»ÃƒÂ©Ã‚Â¢Ã‹Å“ÃƒÂ§Ã…Â Ã‚Â¶ÃƒÂ¦Ã¢â€šÂ¬Ã‚ÂÃƒÂ¦Ã‚Â±Ã‚Â¡ÃƒÂ¦Ã…Â¸Ã¢â‚¬Å“ÃƒÂ¯Ã‚Â¼Ã…â€™ÃƒÂ¥Ã‚Â±Ã…Â¾ÃƒÂ¦Ã¢â‚¬â€Ã‚Â¢ÃƒÂ¦Ã…â€œÃ¢â‚¬Â°ÃƒÂ©Ã…Â¡Ã¢â‚¬ÂÃƒÂ§Ã‚Â¦Ã‚Â»ÃƒÂ§Ã‚Â¼Ã‚ÂºÃƒÂ¥Ã‚ÂÃ‚Â£ÃƒÂ¯Ã‚Â¼Ã…â€™ÃƒÂ©Ã‚ÂÃ…Â¾ÃƒÂ¥Ã…Â Ã…Â¸ÃƒÂ¨Ã†â€™Ã‚Â½ÃƒÂ¥Ã¢â‚¬ÂºÃ…Â¾ÃƒÂ¥Ã‚Â½Ã¢â‚¬â„¢ÃƒÂ£Ã¢â€šÂ¬Ã¢â‚¬Å¡ |
| Non-Darwin Unix process ID | low | BSD without libproc | `is_grok_process` falls back to liveness-only on non-Linux non-macOS Unix. Rare for Hyper targets (we ship Linux/macOS/Windows). |

## Fixed in tree

### S0 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â coexistence / branding / Messages URLs

| ID | Topic | Fix |
|----|--------|-----|
| F-1 | `is_grok_process` ignored `turbo` | Recognizes basenames `turbo` / `grok`, `xai-grok-*` / `xai_grok_*` test bins, and `~/.turbo/bin` / `~/.grok/bin` paths. |
| F-2 | MiniMax / Fireworks Messages 404 | Messages `base_url_override` values are normalized to end in `/v1` before the sampler joins `/messages`. |
| F-3 | Branding | `community-build` (default on the Turbo binary) makes `--version` and `completions` emit `turbo`. |
| F-9 | Local builds without community-build | `xai-grok-pager-bin` defaults include `community-build`. |

### S1 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â OAuth refresh storms + oracle discoverability

| ID | Topic | Fix |
|----|--------|-----|
| F-4 | Kimi lock-held refresh vs 45s follower | Entire Kimi refresh retry loop is capped at **40s** (`REFRESH_TOTAL_BUDGET_SECS`), below the 45s flock wait. Blocking multi-thread resolvers also use the **20s** op timeout. |
| F-5 | Kimi/Codex sticky permanent-failure | Process-local sticky cache keyed by RT fingerprint (char-safe); 401/`invalid_grant` short-circuits force-refresh for 5 minutes; 5xx bodies are not sticky; cleared on login/logout/successful refresh. |
| F-7 | Child Task tool text omitted `oracle` | Nested `CHILD_TASK_DESCRIPTION` and `TaskToolInput` schema list `oracle`. |
| F-1-linux | Leader argv false positives | Linux classification uses **argv0 only** (not later args like `sleep hyper`). |

### S2 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â macOS process identity + logout UX

| ID | Topic | Fix |
|----|--------|-----|
| F-1-mac | macOS/BSD liveness-only process check | macOS/iOS uses `proc_pidpath` + the same basename/path rules as Linux/Windows. |
| F-8 | Bare logout only cleared xAI | Bare logout prints remaining Kimi/Codex scopes; `turbo logout --all` clears xAI + Kimi + Codex (not BYOK keys). |

## Intentional / accepted

| Topic | Behavior |
|--------|----------|
| Shell confine is not an OS sandbox | `--confine` is path-prefix + fail-closed program classifier (`confineShellEnforcement: fail-closed`). Windows AppContainer / Linux Landlock / bwrap are **out of scope** for this package; set `GROK_CONFINE_SHELL_MODE=operand` only for the legacy write-operand scan. |
| Ecosystem / MCP verify plan trust | Clone-and-delegate baseline verify RCE and `delegate_run.verify` live in the **bridge plugin**, not this Hyper tree ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â tracked separately. |
| Shared `~/.grok` | Config, auth, sessions, and leader IPC live under the upstream home. Binary install root is `~/.turbo`. |
| Shared Kimi + Codex proxy | Catalog id (`kimi-code/*` vs `openai-codex/*`) selects credentials; ambiguous URL alone does not guess a family. |
| Hyper Modes | **Deferred** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Amp four-tier modes will not ship as designed; see [design-modes.md](./design-modes.md) Ãƒâ€šÃ‚Â§0. |
| Oracle upgrade | Design in [design-oracle.md](./design-oracle.md); pin + trigger productized (Phase 0/1); Phase 2 harness signals not scheduled. Do **not** pin Oracle to NVIDIA Ultra until `agent_ready`. |
| Read-only children cannot nest Task | `capability_mode: read-only` strips `ToolKind::Task` so explore/oracle/`/deepaudit` cannot spawn write-capable nested agents. |
| Worktree implementation | May still be clone/linked sandbox rather than always `git worktree list`; recovery is via `snapshot_ref` / `changes.patch`. |
| Sticky refresh cache | In-process only (not shared across processes); multi-process still uses flock + compare/adopt. |
| Logout `--all` vs BYOK | Platform API keys under `platform/*` scopes stay until `/logout provider` / `/providers clear`. |

## Coexistence with official `grok`

- Different binaries: `turbo` vs `grok`.
- Shared runtime state under `~/.grok` (including `leader*.sock` / `leader*.lock`).
- Prefer `hyper leader kill` / `grok leader kill` only against leaders you own; both binaries recognize the other product process by name when cleaning locks (Linux argv0, Windows image path, macOS `proc_pidpath`).
- Community builds never run the upstream self-updater that targets `~/.grok/bin/grok`.
