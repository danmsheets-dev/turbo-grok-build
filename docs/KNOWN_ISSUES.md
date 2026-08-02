# Hyper known issues

Living list of fork-specific gaps, fixed items, and intentional limits.
Update this file when closing an issue or shipping a release.

Last reviewed: 2026-08-01 (RC8 reliability + deep audit package).

## Fixed in v0.2.114-r8 (RC8)

| Topic | Fix |
|-------|-----|
| NVIDIA stream deser `null` vs `u32` | Null-tolerant Chat Completions usage/index/tool_calls |
| Subagent hang without timeout | `timeout_ms` + budget monitor; stall on no progress |
| Worktree â€œdisappearedâ€ | `changes.patch` + `snapshot_ref` + `worktree_state` on completion; `retain_worktree` |
| Parent cannot merge child work | `diff_subagent` / `land_subagent` tools or `hyper subagent land` |
| NVIDIA `prompt_cache_key` 400s | Platform defaults + opt-in stamp only |
| Catalog EOL / Nano token overflow | Hide EOL; clamp Nano 9B; `agent_ready` / max_parallel on compat |
| Deep multi-agent audit | `/deepaudit` + `continuous-improve` workflows |

## Open after RC8

| ID | Severity | Topic | Notes |
|----|----------|--------|--------|
| Worktree naming | low | Not always `git worktree list` | Implementation may still use clone/linked sandbox; recovery is via snapshot ref / patch |
| Ultracode free-text keyword | deferred | Auto-workflow on keyword | RC9; slash /ultracode / /deepaudit already ship |
| Fan-out `spawn_many` | deferred | Single-call matrix spawn | Coordinator queue already max 4 |
| Nightly NVIDIA matrix CI | deferred | Live conformance | Unit fixtures shipped |

## Implemented (pre-r8 residual)

| ID | Topic | Notes |
|----|--------|--------|
| R2 | Path allowlists | Optional `allowed_paths` on `task` spawn â†’ `SubagentRequest` / `meta.json`. Non-empty allowlist: `land_subagent` refuses any path outside the prefixes (fail closed); `diff_subagent` filters shown files/diff. Paths normalized (`/` , strip `./`, reject `..` escape / absolute). Omit = unrestricted (prior behavior). |

## Fixed in v0.2.109

- **xAI HTTP 426 / `x-grok-client-version`.** Release CI stamps
  `GROK_VERSION` from the root `VERSION` file into the binary. The `v0.1.0`
  marketing tag set that header to `0.1.0`, which production rejects
  (minimum **0.1.202**). Releases must use the monorepo lockstep version
  (currently `0.2.110`). Upgrade with a fresh `install.sh` run.

## Open (accepted for v0.2.110)

| ID | Severity | Topic | Notes |
|----|----------|--------|--------|
| Modes | deferred | Amp-style lowâ€“ultra agent modes | **ç¼“åœ** â€” [design-modes.md](./design-modes.md) Â§0ã€‚çŽ°æœ‰æ¨¡åž‹é…ç½®å·²å¤Ÿï¼›ä¸ä½œä¸ºå‘å¸ƒç¼ºå£ã€‚ |
| Oracle | done (Phase 0/1) | Stronger-model pin + trigger UX | spawn åŒæ¨¡åž‹ toastã€`/doctor` pin æ£€æŸ¥ã€`spawn_subagent` è§¦å‘æ–‡æ¡ˆå·²è½åœ° â€” [design-oracle.md](./design-oracle.md)ã€‚Phase 2 harness ä¿¡å·æœªæŽ’æœŸã€‚ |
| Flaky test | low | `scrollback::entry::tests::test_truncated_height_cache_hits_when_key_unchanged` | ä»…å…¨é‡å¹¶è¡Œè·‘æ—¶å¶è´¥ï¼ˆçº¦ 1/5 æ¦‚çŽ‡ï¼‰ï¼Œå•è·‘å¿…è¿‡ï¼›ç–‘ä¼¼å¹¶è¡Œæµ‹è¯•é—´å…¨å±€å¤–è§‚/ä¸»é¢˜çŠ¶æ€æ±¡æŸ“ï¼Œå±žæ—¢æœ‰éš”ç¦»ç¼ºå£ï¼ŒéžåŠŸèƒ½å›žå½’ã€‚ |
| Non-Darwin Unix process ID | low | BSD without libproc | `is_grok_process` falls back to liveness-only on non-Linux non-macOS Unix. Rare for Hyper targets (we ship Linux/macOS/Windows). |

## Fixed in tree

### S0 â€” coexistence / branding / Messages URLs

| ID | Topic | Fix |
|----|--------|-----|
| F-1 | `is_grok_process` ignored `hyper` | Recognizes basenames `hyper` / `grok`, `xai-grok-*` / `xai_grok_*` test bins, and `~/.hyper/bin` / `~/.grok/bin` paths. |
| F-2 | MiniMax / Fireworks Messages 404 | Messages `base_url_override` values are normalized to end in `/v1` before the sampler joins `/messages`. |
| F-3 | Branding | `community-build` (default on the Hyper binary) makes `--version` and `completions` emit `hyper`. |
| F-9 | Local builds without community-build | `xai-grok-pager-bin` defaults include `community-build`. |

### S1 â€” OAuth refresh storms + oracle discoverability

| ID | Topic | Fix |
|----|--------|-----|
| F-4 | Kimi lock-held refresh vs 45s follower | Entire Kimi refresh retry loop is capped at **40s** (`REFRESH_TOTAL_BUDGET_SECS`), below the 45s flock wait. Blocking multi-thread resolvers also use the **20s** op timeout. |
| F-5 | Kimi/Codex sticky permanent-failure | Process-local sticky cache keyed by RT fingerprint (char-safe); 401/`invalid_grant` short-circuits force-refresh for 5 minutes; 5xx bodies are not sticky; cleared on login/logout/successful refresh. |
| F-7 | Child Task tool text omitted `oracle` | Nested `CHILD_TASK_DESCRIPTION` and `TaskToolInput` schema list `oracle`. |
| F-1-linux | Leader argv false positives | Linux classification uses **argv0 only** (not later args like `sleep hyper`). |

### S2 â€” macOS process identity + logout UX

| ID | Topic | Fix |
|----|--------|-----|
| F-1-mac | macOS/BSD liveness-only process check | macOS/iOS uses `proc_pidpath` + the same basename/path rules as Linux/Windows. |
| F-8 | Bare logout only cleared xAI | Bare logout prints remaining Kimi/Codex scopes; `hyper logout --all` clears xAI + Kimi + Codex (not BYOK keys). |

## Intentional / accepted

| Topic | Behavior |
|--------|----------|
| Shell confine is not an OS sandbox | `--confine` is path-prefix + fail-closed program classifier (`confineShellEnforcement: fail-closed`). Windows AppContainer / Linux Landlock / bwrap are **out of scope** for this package; set `GROK_CONFINE_SHELL_MODE=operand` only for the legacy write-operand scan. |
| Ecosystem / MCP verify plan trust | Clone-and-delegate baseline verify RCE and `delegate_run.verify` live in the **bridge plugin**, not this Hyper tree â€” tracked separately. |
| Shared `~/.grok` | Config, auth, sessions, and leader IPC live under the upstream home. Binary install root is `~/.hyper`. |
| Shared Kimi + Codex proxy | Catalog id (`kimi-code/*` vs `openai-codex/*`) selects credentials; ambiguous URL alone does not guess a family. |
| Hyper Modes | **Deferred** â€” Amp four-tier modes will not ship as designed; see [design-modes.md](./design-modes.md) Â§0. |
| Oracle upgrade | Design in [design-oracle.md](./design-oracle.md); pin + trigger productized (Phase 0/1); Phase 2 harness signals not scheduled. Do **not** pin Oracle to NVIDIA Ultra until `agent_ready`. |
| Read-only children cannot nest Task | `capability_mode: read-only` strips `ToolKind::Task` so explore/oracle/`/deepaudit` cannot spawn write-capable nested agents. |
| Worktree implementation | May still be clone/linked sandbox rather than always `git worktree list`; recovery is via `snapshot_ref` / `changes.patch`. |
| Sticky refresh cache | In-process only (not shared across processes); multi-process still uses flock + compare/adopt. |
| Logout `--all` vs BYOK | Platform API keys under `platform/*` scopes stay until `/logout provider` / `/providers clear`. |

## Coexistence with official `grok`

- Different binaries: `hyper` vs `grok`.
- Shared runtime state under `~/.grok` (including `leader*.sock` / `leader*.lock`).
- Prefer `hyper leader kill` / `grok leader kill` only against leaders you own; both binaries recognize the other product process by name when cleaning locks (Linux argv0, Windows image path, macOS `proc_pidpath`).
- Community builds never run the upstream self-updater that targets `~/.grok/bin/grok`.
