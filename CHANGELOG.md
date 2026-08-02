# Changelog

All notable changes to **Hyper** (`hyper` binary) are documented here.

## [Unreleased]

### Added
- **RC9 worktree trust:** spawn **baseline** refs
  (`refs/grok/subagent-baselines/<id>`) so diff/land are **agent-only**
  (`baseline..snapshot`), not dirty-parent pollution. Land refuses >50 files
  unless `force=true`. `hyper subagent open <id> --restore` materializes a
  detached worktree. Soft-preserve live trees by default
  (`GROK_SUBAGENT_SOFT_PRESERVE=0` deletes immediately). Optional clean seed
  via `GROK_SUBAGENT_WORKTREE_SEED=clean`. Windows: `USERPROFILE` fallback when
  `HOME` unset. Completion summary surfaces snapshot/baseline/patch/top paths.
- **Agent Boot Card:** short operational briefing injected into system context
  on new sessions (subagents get a child stub). Configure with
  `GROK_BOOT_CARD=off|short|full`. Teaches worktree recovery, land safety, and
  tools without a full user-guide dump.
- **Copy-to-clipboard on messages:** `selection_buttons` defaults **on** so
  completed scrollback messages show a copy icon (bottom-right of selection)
  that copies full output.
- **Auto Developer Log (ADL):** structured product-issue store for agents and
  runtime detectors under `$GROK_HOME/developer-log/`. Agent tool
  `developer_log` files/dedups redacted incidents; `hyper issues
  list|show|export|ack|resolve|path` reviews and exports maintainer packs;
  auto-detectors cover worktree dispose without recovery artifacts, isolation
  fallback, and stall/timeout. Disable with `GROK_DEVELOPER_LOG=0`. See
  `docs/AUTO_DEVELOPER_LOG.md`.
- **`/ultracode`** (and `/ultra-code`): slash aliases for `/deepaudit`.
- **RO isolation residual (R3):** when spawn omits `isolation`, explore/plan/
  oracle and `capability_mode=read-only` default to `isolation=none` (skip
  worktree cost). Explicit isolation is never overridden; write agents still
  default to worktree.
- **`spawn_many` tool**: fan-out multiple Task spawns in one call (compose
  coordinator queue, max 4 concurrent). Optional `wait` barrier via multi-id
  `get_task_output`. Empty `tasks` rejected; max 20 entries.
- **Durable LoopCheckpoint**: `continuous-improve` still writes workflow scratch
  `loop_checkpoint.json`; host also mirrors to session
  `loops/<workflow_run_id>/checkpoint.json` for cross-process resume.

## [0.2.114-r8] - 2026-08-01

RC8 reliability + deep-audit release (full plan): NVIDIA agent path, worktree
recovery, land/diff tools, stall detection, and multi-agent workflows.

### Added
- **`/deepaudit`** (aliases `/deep-audit`, `/ultracode`, `/ultra-code`):
  Ultracode-style codebase audit â€” Scope â†’ Investigate â†’ Verify â†’ Report.
  Size `small|medium|large`.
- **`continuous-improve`** builtin workflow: research â†’ plan â†’ implement
  (worktree) â†’ verify â†’ report.
- **`timeout_ms` / `stall_timeout_ms` / `retain_worktree` on spawn**: hard
  wall-clock, progress stall (default 10m when budgets set), optional keep path.
- **`diff_subagent` + `land_subagent` + `discard_subagent` tools**: parent merges
  or drops child work from live worktree, snapshot ref, or `changes.patch`
  (merge fails closed on conflict; discard keeps snapshot by default).
- **`hyper subagent` CLI**: `list` / `open` / `diff` / `land` / `discard` /
  `prune --older-than 24h` over session `subagents/<id>/` metadata (tool parity).
- **Worktree dispose**: always export `changes.patch` + diffstat before delete;
  completion surfaces `snapshot_ref`, `worktree_state`, `patch_path`.
- **NVIDIA platform defaults**: no `prompt_cache_key` stamp without compat;
  catalog EOL hide; Nano 9B token clamp; `agent_ready` / `max_parallel_tool_calls`
  on compat; Llama 70B single tool-call wire; **10 min hard timeout** and
  **3 min stall** defaults when spawn omits budgets on nvidia/nemotron models.
- **`error_class`** on subagent completion/failure for smart-retry
  (`timeout`, `stall`, `serialize`, `provider_400`, `cancelled`, `budget`, â€¦).
- **LoopCheckpoint**: `continuous-improve` writes `loop_checkpoint.json` under
  workflow scratch after each phase; implement phase uses `retain_worktree`.

### Fixed
- **NVIDIA Chat Completions deser**: `null` usage/index/tool_calls no longer
  crash the client (`invalid type: null, expected u32`).
- **Stall detector**: no tool/token/turn progress â†’ cancel with
  `termination_reason=stall`.

### Docs
- `docs/RC8_IMPLEMENTATION_PLAN.md`, `docs/RC8_BUILD_INSTALL.md`, feedback.
- User guide: `/deepaudit`.

## [0.2.114-r7] - 2026-07-31

Subagent isolation-by-default release, plus image previews on terminals without
a graphics protocol.

### Changed
- **Subagents default to `isolation=worktree`.** Parallel writers no longer
  share the parent tree by default. Completed worktrees are snapshotted and then
  removed (opt out with `GROK_SUBAGENT_WORKTREE_SNAPSHOT=0`), and a worktree
  created for a spawn that aborts early is removed rather than left behind.
  Subagent worktrees additionally age out after 24h via auto-GC.
- **`isolation_fallback` is surfaced in tool output**, so a harness can see that
  a subagent did not get the isolation it asked for instead of inferring it.

### Fixed
- **Multi-model resolution.** An explicit `Task`/spawn model now wins over the
  `fork_context` parent pin, and an empty `model_ids` on resume is ignored
  instead of overriding the configured model.
- **Image previews on terminals without Kitty/iTerm graphics.** On Windows
  ConPTY and similar, chip hover and the Enter image viewer paint a truecolor
  half-block raster instead of showing metadata only. Kitty paths are unchanged.

## [0.2.114-r6] - 2026-07-31

Isolation and headless honesty release. Driven by a 4-source audit (two Grok 4.5
passes, a 20-agent multi-lens audit, and defects found by running the tool) plus
two field reports from operators running real Godot and Blender work.

### Fixed
- **Confinement is a boundary, not a set of heuristics.** `--confine` is now
  inherited by child processes (a nested `hyper`, an MCP server, or a hook
  previously ran completely unconfined). `apply_patch` presented the literal
  placeholder `AccessKind::Edit("apply_patch")` to the permission gate instead of
  the hunk's real target, then joined an absolute path that replaced the base â€”
  real hunk paths now reach the gate, absolute and parent components are
  rejected, covering Add/Delete/Update/Move-destination. MCP tool calls are
  confine-checked. Leader mode is vetoed under confine. A `ConfinedFs` choke
  point closes the gap where enforcement lived only in the permission actor.
- **A subagent that could not create its worktree silently ran in the shared
  workspace** â€” the user's live checkout â€” signalled only by a `tracing::warn!`
  no harness can see. Isolation now fails closed; the fallback is opt-in and
  reports `isolation_fallback`.
- **The shell confine check false-positived on ordinary compound commands**,
  reporting an entire command string in the `path` field while every operand was
  inside the root. It now recovers operands from `;`-separated, piped and
  PowerShell forms, and reports an unparseable command as a policy decision
  rather than a path violation.
- **Folder trust was inert on locally built binaries**, so every cloned
  repository's `.mcp.json`, `.grok/plugins`, `.grok/hooks` and `[permission]`
  rules auto-loaded with no prompt. Armed by default.
- `ask_user_question` could block a headless run for up to 30 minutes.
- `--rules` / `--append-system-prompt` was silently dropped on `--resume`.
- `--worktree` was a silent no-op in headless mode; it now fails loudly.
- A cross-directory `--resume` reported the process cwd while working elsewhere.

### Added
- **streaming-json `schemaVersion` 2**, documented in
  `docs/streaming-json-schema.md` with a JSON Schema. Emits `tool_call`,
  `tool_call_update`, `tool_result`, subagent lifecycle events, and
  `end.toolCalls` / `end.subagents` rollups. Previously no tool events existed
  at all, so a harness could not distinguish a thinking model from a long tool
  call â€” measured cost: a run appeared hung for 34 minutes during a `cargo test`.
- `--require-trust`, `--stream-tool-io`, `--require-subagent-success`.
- `start` reports `sessionCwd`, `originalCwd` and `folderTrust`.
- A global concurrency cap on native `task` subagent spawns, default 4.
- `confine_violation` on every denial.

### Known limitations
- A real OS sandbox (AppContainer / Landlock / bwrap) is **not** implemented.
  The shell path fails closed on unknown writers and reports the enforcement
  level actually in force. See `docs/KNOWN_ISSUES.md`.


### Added
- **`api_backend = "codex_responses"`** (alias `codex-responses`) â€” OpenAI Responses wire with ChatGPT Codex dialect for custom models and third-party Codex reverse proxies (ä¸­è½¬ç«™). Enables systemâ†’`instructions`, strips temperature/top_p/max_output_tokens, and uses the OpenAiCodex adapter without requiring `openai-codex/*` OAuth catalog IDs.

## [0.2.114-r5] â€” 2026-07-29

### Fixed
- **Linux glibc floor** â€” Release Linux `linux-gnu` binaries are linked with `cargo-zigbuild` against **glibc 2.17** (Ubuntu 16.04 / RHEL 7 class) instead of the ubuntu-24.04 runner libc. Host-built artifacts required GLIBC_2.39 (`pidfd_*`, `__isoc23_*`) and failed on older distros. CI refuses to publish if the binary's max `GLIBC_*` symbol exceeds the floor. Asset names stay `*-unknown-linux-gnu` (no musl; musl remains blocked by sqlite-vec/jemalloc CFLAGS).
- **Installer bundled skills extract** â€” `install.sh` only lists file members for `tar -T` (skip directory entries with trailing `/`). GNU tar 1.35 otherwise failed to extract `bundled/skills` from release archives.

### Notes
- This is community revision `0.2.114-r5`; it remains a normal GitHub Release so installers and `hyper update` treat it as latest.
- Existing asset names (`*-unknown-linux-gnu`) are unchanged; only the dynamic symbol floor improves.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r5
```

## [0.2.114-r4] â€” 2026-07-29

### Added
- **Native OMP session continuation** â€” `/resume-omp`, the foreign-session picker, recent-session Ctrl+U hint, and `[compat.omp].sessions` now discover OMP CLI sessions lazily behind the same bundled-runtime gate as Claude, Codex, and Cursor. Release archives ship the `resume-omp` skill plus the shared inert-history reader (`bundled/skills/shared/resume-session`), including OMP profile/XDG/custom-root and native-ID support.
- **Base16 Default Dark and OMP themes** â€” Adds stable theme IDs 18 (`base16-default-dark`) and 19 (`omp` / Titanium), terminal-capability clamping for syntax colors, and release packaging for the shared resume-session readers.
- **Extension author proc macros** â€” Recommended guest path is now `#[hyper_plugin]` / `#[hyper_hook]` / `#[hyper_tool]` so handlers stay ordinary named functions for IDE navigation; legacy `hyper_extension!` remains for source compatibility. WASM bootstrap ABI stays at version 1.
- **MCP enable/disable CLI and project unstick** â€” Server enable state can be persisted through user `disabled_mcp_servers` (and per-server `enabled` when present). Enabling can clear sticky project-level `enabled = false` without rewriting shared project configs on disable.

### Changed
- **Upstream sync** â€” Merged official `xai-org/grok-build` `main` at `5da6962` (monorepo `SOURCE_REV` `2a818575â€¦`), including session lifecycle reaping (child processes, LSP, stdio MCP, subagents), plan/minimal scrollback and reasoning separation, SuperGrok Plus tier surfaces, workspace `git_sync_base` / git_commit hardening, fuzzy @-file-search degradation, and circuit-breaker gRPC retry policy.
- **Symlink-preserving config persistence** â€” Atomic configuration and credential writes resolve final-component symlinks before replace so user-managed config/auth/MCP links are not clobbered.
- **Installer bundled runtime** â€” Unix and Windows installers install the release `bundled/` tree under `~/.grok/bundled` after the binary smoke-tests. Unix keeps checksum-versioned binary identity under `~/.hyper/downloads`; Windows continues to activate a fixed `~/.hyper/bin/hyper.exe` path with rollback.

### Notes
- This is community revision `0.2.114-r4`; it remains a normal GitHub Release so installers and `hyper update` treat it as latest.
- Extension guests still target the core-WASM bootstrap ABI (`CORE_ABI_VERSION = 1`). Only trusted, enabled plugins load.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r4
```

## [0.2.114-r3] â€” 2026-07-28

### Fixed
- **OpenCode Go retry response parsing** â€” Accept valid Chat Completions chunks and Messages `message_start` events that omit only the provider response ID, while leaving semantic tool-call validation and other required stream fields unchanged.
- **Oversized Linux release binaries** â€” Distribution builds no longer embed debug metadata or retain non-runtime symbols. The release workflow now rejects binaries over 256 MiB and verifies that Linux artifacts contain no DWARF debug sections.

### Notes
- This is community revision `0.2.114-r3`; it remains a normal GitHub Release so installers and `hyper update` treat it as latest.
- The r2 size regression was Linux-specific: both Linux binaries unpacked to about 1.36 GB, while r2 macOS binaries were 174â€“188 MB and Windows was 151 MB. The stricter release profile is applied consistently to all five targets.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r3
```

## [0.2.114-r2] â€” 2026-07-28

### Added
- **Pi-aligned provider platform** â€” Data-driven registry and reproducible catalog sync pinned to `@earendil-works/pi-ai@0.82.1`, covering all 37 static Pi providers plus dynamic Radius discovery. Hyper now ships 42 provider rows and 1,144 catalog models with explicit endpoint, authentication, protocol, thinking, and request-compat metadata.
- **Native provider backends** â€” First-class Google GenerateContent (Gemini and Vertex), Amazon Bedrock ConverseStream, and Pi `pi-messages` adapters with streaming text/reasoning, tool calls, cache/reasoning usage, provider-reported cost, and provider-native continuation state.
- **Provider authentication** â€” GitHub Copilot OAuth and model discovery; Radius browser PKCE, device flow, refresh-token rotation, API-key priority, credential-scoped dynamic caching, single-flight refresh, and stale fallback; expanded API-key and hybrid-provider login/logout UX across CLI and TUI.
- **WASM extension platform** â€” Trusted plugins can load sandboxed Wasmtime guests and participate in session start/end, before-agent, before-model, pre-tool, stop-gate, and pre-compaction lifecycle points. Capability-gated guests can inject context, deny tools, continue a turn, register session-scoped tools, emit metrics, and retain bounded per-session state.
- **Extension author tooling** â€” New extension API/runtime/SDK crates, declarative Rust guest macros, checked-in examples, plugin init/build/validate commands, runtime details in `/plugins`, author documentation, and a path-filtered extension CI workflow.

### Changed
- **Upstream sync** â€” Merged official `xai-org/grok-build` `main` at `02d9359`, including scheduler foreground/background loop semantics, plan-exit batch barriers, leader sandbox confinement, workspace/hub reliability, and pager session-state improvements.
- **Provider routing architecture** â€” Sampling now dispatches through explicit backend adapters instead of inferring behavior from provider names. Opaque reasoning signatures are replayed only when model, backend, and endpoint identities match.

### Fixed
- **Provider stream correctness** â€” Hardened truncated/unknown event handling, zero-argument and interleaved tool calls, authoritative argument assembly, usage/cost accounting, idle timeouts, and portable fallback when native continuation identity changes.
- **OAuth and dynamic catalog safety** â€” Radius callback errors are accepted only after OAuth state validation, expiry skew is applied once, and dynamic model updates remain atomic and credential-isolated.
- **Extension lifecycle isolation** â€” WASM tools are scoped and cleaned up per session; concurrent sessions cannot unregister each otherâ€™s tools; fail-closed trap behavior, stop continuation caps, fuel/epoch bounds, and guest memory limits are covered end to end.

### Notes
- This is community revision `0.2.114-r2`; it remains a normal GitHub Release so installers and `hyper update` treat it as latest.
- The extension ABI is the documented core-WASM bootstrap contract. Only trusted, enabled plugins load; policy gates default to fail-open for compatibility unless `runtime.gate_fail` or `GROK_EXTENSION_GATE_FAIL` selects `closed`.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r2
```

## [0.2.114-r1] â€” 2026-07-28

### Added
- **OpenCode Go subscription provider** â€” Configure a Console-issued Go API key with `/providers opencode-go <key>` or `OPENCODE_API_KEY`, then use the bundled `opencode-go/*` catalog. Models are routed per official metadata across OpenAI Chat Completions and Anthropic Messages; `/login opencode-go` explains the documented API-key flow instead of reusing OpenCode's undocumented, CLI-specific OAuth client identity.
- **Isolated Hyper self-updates** â€” `hyper update` and startup auto-update now resolve only `DaviRain-Su/hyper-grok-build` GitHub Releases, verify `SHA256SUMS`, and keep managed binaries/update state under `~/.hyper`. The official `~/.grok/bin/grok` installation is never used as an update target.
- **12 preset themes** â€” A curated collection layered on top of the original five, distinguished primarily by background color: nine dark (`everforest`, `nord`, `dracula`, `gruvbox`, `catppuccin-mocha`, `solarized-dark`, `deep-ocean`, `ember`, `midnight-oled`) and three light (`solarized-light`, `catppuccin-latte`, `paper`). Pick via `/theme <name>`, Settings â†’ Appearance â†’ Theme, or the `auto` dark/light pairings. All are truecolor (RGB) and fall back to Grok Night on 256/16-color terminals. Each preset is defined from a compact palette expanded through a shared builder so semantic roles (error=red, success=green, sunken code blocks, scrollbar contrast) stay consistent across the set.
- **Translations for the 12 preset themes** â€” `settings.{theme,auto_dark_theme,auto_light_theme}.choice.*.description` for the nine dark + three light presets across all nine non-English locales (de, es, fr, ja, ko, pt-BR, ru, zh-CN, zh-TW); previously they fell back to English.
- **Nix flake packaging** â€” `flake.nix` / `flake.lock` package `hyper` from `xai-grok-pager-bin` with a matching `devShell`. Package version is read from the root `VERSION` file (no hardcoding). Documented in README as `nix build` / `nix run` / `nix develop`.

### Fixed
- **Bare login provider drift** â€” Bare `/login` and the welcome-screen Login action now resolve the advertised xAI `grok.com` / enterprise OIDC method on every invocation instead of reusing a prior explicit Kimi, OpenAI, or Claude selection. Third-party subscription login remains available only through its explicit provider command.
- **Same-version republish safety** â€” Community deployments use the release archive SHA-256 as part of their identity, so a deliberately republished tag installs once and converges. Downloads are locked, staged, smoke-tested, and atomically activated without overwriting the current binary first.
- **Theme switch appeared to need a restart** â€” On terminals that don't advertise truecolor, the Settings â†’ Appearance theme picker still offered the truecolor-only presets. Selecting one clamped the live view to Grok Night (screen unchanged) yet persisted the choice, so it only "took effect" after a restart (the startup path applies the persisted theme un-clamped). The picker now hides themes the current terminal can't render (mirrors `/theme`'s `available()` gate), so `theme` / `auto_dark_theme` / `auto_light_theme` only list renderable options.
- **Theme toasts bypassed i18n** â€” The `theme` / `auto_dark_theme` / `auto_light_theme` "âœ“ â€¦" confirmation toasts were hardcoded English (label + format). They now route through the localized `toast.saved` bundle like every other setting.

### Notes
- Community revision tag uses a `-rN` suffix (`0.2.114-r1`) so we can ship Hyper-only changes without claiming a clean upstream patch bump. Later community revisions on the same line can be `0.2.114-r2`, `0.2.114-r3`, â€¦
- Wire version remains lockstep with Hyper crate versions (`0.2.114-r1`). GitHub Release is published as a normal (non-prerelease) release so installers and `hyper update` treat it as latest.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r1
```

## [0.2.113] â€” 2026-07-27

### Added
- **Nexus relay gateway** â€” `/nexus <api_key> [base_url]` and `/providers nexus â€¦` persist a BYOK bearer plus an optional self-hosted gateway root, discover both OpenAI Chat Completions and Anthropic Messages catalogs, and route each discovered model through the matching protocol endpoint.
- **Anthropic Claude subscription login** â€” `/claude` and `/login claude` add Claude Pro/Max OAuth with PKCE, scoped credential storage, rotating refresh-token persistence, per-request bearer resolution, and built-in Claude subscription models.

### Fixed
- **`/live` idle disconnects** â€” The Codex Live sideband now sends protocol-level keepalive pings while no control messages are flowing, preventing proxies and load balancers from reaping otherwise healthy voice sessions after a few quiet minutes.
- **Subagent model-pin activation** â€” Saving a model pin in `/agents` now performs an acknowledged shell reload before releasing the modal, so fresh subagent spawns use the new model immediately even after a long-running session; resumed agents still retain their original model.
- **Claude OAuth safety** â€” Require an exact OAuth `state` match before callback success or token exchange, use Anthropicâ€™s registered callback for the bundled client, reject unsupported loopback redirects, and serialize rotating refresh tokens across processes.
- **Provider and policy regressions** â€” Preserve Nexus custom gateway URLs end-to-end, trusted managed-config signature controls, fail-closed settings-only startup prefetch, tolerant MCP parsing, and Unix file-descriptor limit raising.
- **Provider credential fallback** â€” When a BYOK or platform OAuth credential is cleared or expires, a newly locked provider model can no longer remain the active sampling model; the session falls back to an available default before the catalog update is published.
- **Pager registry coverage** â€” Reserve the new provider/scoped-model commands and aliases, complete translations across all supported locales, and refresh deterministic usage snapshots.

### Changed
- **Upstream sync** â€” Sync the community build with upstream `main` at `SOURCE_REV` `91d8cf309110a3b879c1b8198f7525aed545dfb4`, including instant UI startup with background model/settings fetch, bounded session-load and fork-replay memory, subagent lifecycle resource bounds, full-plan copy with `y`, terminal-version telemetry, and managed-policy hardening.

### Notes
- `v0.2.113` was republished after this upstream sync; replace any earlier archive and checksum file together because the rebuilt assets have different digests.
- Wire version remains lockstep with Hyper crate versions (`0.2.113`).

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.113
```

## [0.2.112] â€” 2026-07-25

### Added
- **Scoped models (Pi-style shortlist)** â€” `[models].enabled_models` globs (also reads Pi camelCase `enabledModels`), `/scoped-models` (`add` / `remove` / `set` / `clear`), and **Alt+]** / **Alt+[** to cycle only the shortlist. Empty shortlist cycles all usable models; invalid globs are rejected and never silently expand to â€œall modelsâ€. Full picker remains `/model` Â· Ctrl+M.
- **OpenAI prompt-cache affinity** â€” Every turn stamps `prompt_cache_key` (session id, â‰¤64 chars) on **Responses** and **Chat Completions** so OpenAI-compatible prefix caches stick to the session. Optional `GROK_PROMPT_CACHE_RETENTION=24h` (or `long`) for Responses extended retention; Codex still strips retention (backend rejects it). **No** Anthropic Messages multi-breakpoint `cache_control` expansion.
- **`/usage` cache hit rate** â€” Session usage shows a hit-rate line when providers report cached input tokens.
- **Docs** â€” User guide Â§29 models/providers/scoped selection (EN/zh), Â§30 OpenAI prompt caching (EN/zh), slash-command notes for `/scoped-models`.

### Fixed
- **macOS `/live` speaker silence** â€” The `__speaker-play` helper no longer forces mono/`i16`/48 kHz on CoreAudio. It opens the device default config (often stereo + `f32` + 44.1 kHz), resamples and upmixes the WebRTC mono stream, waits for a `READY`/`ERR` handshake before feeding PCM, and stops the playback queue if the player dies so failures surface instead of silent no-sound.
- **macOS `/live` crackle / stutter (æ’•æ‹‰)** â€” Match Linuxâ€™s continuous-stream model inside the helper: PCM goes into a continuous sample ring (not discrete `mpsc` chunks), callbacks use the same fill path as Windows (pull only what the buffer needs, hold last sample on underrun), prefer a 48 kHz device config when available, enlarge the playback queue to ~1s, and stop flushing every pipe write (which fragmented audio and starved CoreAudio).
- **Live Opus PLC double-decode** â€” After packet-loss concealment/FEC recovery, the decoder no longer immediately re-decodes the same payload without FEC (which corrupted state and could double-play or mute frames).

### Changed
- **Hyper auth is multi-provider first** â€” On community builds, first launch no longer auto-starts Grok OAuth. The welcome screen waits for you to choose: press `l` for the default login, or after entry use `/login openai`, `/login kimi`, `/providers <platform> <key>`, or `/model`. Consumer SuperGrok access gates no longer lock the whole TUI. When Grok free usage is exhausted, the modal also offers **Switch model or use API key** and **Dismiss** instead of only upgrade links.

### Notes
- Provider stance: prefer OpenAI-compatible APIs (Chat Completions / Responses) plus existing Messages paths; no first-class native Google / Bedrock / Azure clients.
- Wire version remains lockstep with Hyper crate versions (`0.2.112`).

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.112
```

## [0.2.111] â€” 2026-07-25

### Added
- **Codex Live voice sessions (`/live`)** â€” Full-duplex voice powered by ChatGPT Codex OAuth and `gpt-live-1-codex`, with realtime transcripts, mute and barge-in controls, native WebRTC/Opus audio, and spoken model responses. The Live assistant delegates coding work to the bound Hyper agent, relays tool-boundary progress, and returns the agent's final result to the voice conversation.
- **Sideband protocol hardening** â€” Precise WebSocket close-frame diagnostics (code + reason preserved), binary frame treated as protocol failure, EOF-without-close detection, and once-only error reporting via atomic `failure_reported` guard.
- **Error toast propagation** â€” Terminal transport/media errors are now preserved through to the user-facing toast (e.g. `"Live stopped: Codex live sideband closed (1008): policy changed"`) instead of a generic `"stopped unexpectedly"` message.
- **Log security** â€” `redact_live_error_for_log()` strips Bearer tokens, access tokens, cookies, session IDs, and passwords before writing errors to persistent diagnostic logs, with bounded length truncation.
- **Data-channel event gating** â€” Sideband-open atomic gate prevents duplicate `delegation.created`/transcript/turn events when both the sideband WebSocket and the data-channel deliver the same server payload.
- **Command queue reliability** â€” Capacity-aware critical drain: `CompleteDelegation` and `Shutdown` are queued with stable sequence IDs when the channel is full; commentary events are shed under pressure without silent protocol loss.
- **PCM hot-loop fix** â€” Closed PCM source no longer starves session teardown; the session remains responsive to `Shutdown`/`CompleteDelegation` commands after the microphone source ends.
- **Config unification** â€” Codex base URL now resolved through `PlatformId::OpenAiCodex.base_url()`, sharing the same `GROK_OPENAI_CODEX_BASE_URL` override as normal Codex inference.
- **Build isolation** â€” Linux musl target flags for RELRO/non-executable stack hardening in `.cargo/config.toml`.
- **Documentation** â€” Complete Codex Live user guide in English and Simplified Chinese (`/live` slash command, audio requirements, environment variables).

### Changed
- Sync the community build with the upstream `0.2.111` monorepo line (`SOURCE_REV` `9b8d35b`), including auth fail-closed refresh, voice interim commit-on-submit, workflow scratch quotas / resumable failed runs, leader process hardening, plugin subagent MCP inheritance, and the refreshed permissions / plugins / marketplaces docs.

### Notes
- `/live` uses an undocumented internal Codex Live protocol and may stop working when the backend changes. It is independent of the active coding provider but requires `grok login --openai`.
- Existing `/voice` dictation is unchanged. `/voice` and `/live` are mutually exclusive so they never compete for the microphone.
- Wire version remains lockstep with upstream crate versions (`0.2.111`).

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.111
```

## [0.2.110] â€” 2026-07-23

### Added
- Add `hyper dashboard --web`, a loopback-only, read-only web observability UI built with Axum and Leptos SSR.
- Add session overview, filtering, detail, timeline, chat, charts, active-process memory, unified-log, JSON API, and live SSE views over existing `~/.grok` artifacts.
- Add runtime-selectable TUI localization with ten language bundles and complete Simplified Chinese user-guide and hooks documentation.
- Add the built-in `xdotcom` subagent for X.com content workflows.

### Changed
- Sync the community build with the upstream `0.2.110` monorepo line.
- Refresh the project storefront with Hyper branding, real TUI screenshots, badges, and updated Oracle/modes design guidance.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.110
```

## [0.2.109] â€” 2026-07-22

**Wire-compatible release.** Hyper stamps `x-grok-client-version` from the root
`VERSION` file via `GROK_VERSION` at build time. xAI's API gate rejects clients
below **0.1.202** (HTTP 426). The previous `0.1.0` marketing tag was therefore
unusable against production Grok models (e.g. grok-4.5).

This tag **matches the monorepo lockstep crate version** (`xai-grok-pager` /
`xai-grok-version` / shell at `0.2.109`), which is also above the official
stable line (`grok 0.2.106` at time of release).

### Fixes
- Align release `VERSION` / GitHub tag with monorepo client version so API
  version gates accept the binary.
- Document that Hyper release tags must track the pager lockstep version, not
  an independent `0.1.x` marketing line.

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash
# pin:
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.109
```

The earlier `v0.1.0` assets remain on GitHub Releases for historical download
but must not be used against current xAI endpoints.

## [0.1.0] â€” 2026-07-22

First tagged Hyper community release of the multi-provider Grok Build fork.

> **Superseded for API use.** `x-grok-client-version: 0.1.0` is rejected by
> xAI (min **0.1.202**). Upgrade to **v0.2.109** or later.

### Highlights

- **Binary name `hyper`**, install root `~/.hyper/bin`, shared runtime state under `~/.grok` (compatible with official `grok` config/auth).
- **Multi-provider catalog**: Moonshot, Kimi Code OAuth, ChatGPT Codex OAuth, OpenAI/Anthropic BYOK, Z.AI, Ollama Cloud, and more.
- **Oracle** built-in read-only subagent for deep analysis (pin a strong model via `/agents` or `[subagents.models]`).
- **Community builds** disable the upstream self-updater so Hyper cannot overwrite `~/.grok/bin/grok`.

### Reliability (this release line)

- Keep xAI session bearer off third-party BYOK platforms (including live-only catalog models).
- Route-aware opaque reasoning replay (model + API backend + endpoint identity).
- Catalog-first OAuth identity for Kimi vs Codex (including shared reverse-proxy origins).
- Kimi/Codex sticky permanent-failure cache for revoked refresh tokens (process-local).
- Kimi lock-held refresh total budget capped below the cross-process flock wait.
- Multi-thread blocking resolvers bounded by a 20s operation timeout.
- MiniMax / Fireworks Messages bases normalized to `â€¦/v1` before `/messages` join.
- Leader cleanup recognizes `hyper` and `grok` product processes (Linux argv0, Windows image path, macOS `proc_pidpath`).
- `hyper logout --all` clears xAI + Kimi + Codex OAuth; bare logout hints remaining scopes.
- `hyper --version` / completions brand as `hyper` when built with `community-build` (default).

### Install

```sh
curl -fsSL https://raw.githubusercontent.com/DaviRain-Su/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.109
```

See [README.md](./README.md) and [docs/KNOWN_ISSUES.md](./docs/KNOWN_ISSUES.md).

### Not in this release

- Amp-style **agent modes** (low/medium/high/ultra) â€” design only (`docs/design-modes.md`).
