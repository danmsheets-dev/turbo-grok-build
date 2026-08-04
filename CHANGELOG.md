# Changelog

All notable changes to **Turbo Grok Build** (`turbo` binary).

Format: [Keep a Changelog](https://keepachangelog.com/).  
Wire versions: [`VERSION`](./VERSION) (`0.2.119-rN` community RC line).

English-only product surface (UI and public docs) as of RC14.

---

## Pedigree (community line)

Turbo Grok Build evolved from the Hyper community fork of
[xAI Grok Build](https://github.com/xai-org/grok-build). Multi-agent work
accelerated at r6; product rebrand to **Turbo** at r10.

| RC | Wire | Theme |
|----|------|--------|
| r6 | `0.2.114-r6` | Isolation + headless honesty |
| r7 | `0.2.114-r7` | Folder worktrees by default |
| r8 | `0.2.114-r8` | `/deepaudit`, land/diff/discard |
| r9 | `0.2.114-r9` | Baselines, Boot Card, Auto Developer Log |
| r10 | `0.2.114-r10` | Turbo brand / `turbo` CLI |
| r11 | `0.2.114-r11` | Game Mode, Feature Request Log |
| r12 | `0.2.114-r12` | Isolation FS jail, densify, MCP harden |
| r13 | `0.2.114-r13` | Workspace Tree inject, Game Mode perf |
| r14 | `0.2.114-r14` | **web_fetch**, **workflow routing**, English-only |
| **r15** | **`0.2.119-r1`** | **Upstream 0.2.119 sync**, security + Windows correctness |

Older release notes (r1–r13 detail) are archived under
[`docs/archive/`](./docs/archive/).

---

## [0.2.119-r1] - 2026-08-04

**Turbo Grok Build RC15.** The wire version jumps `0.2.114` → `0.2.119`: Turbo's
upstream base was actually **0.2.112** (newest bundled release notes were
`0.2.112.md`) while `VERSION` advertised `0.2.114`, so `--version` and the
What's-New surface were both wrong. RC15 syncs seven upstream releases and
re-stamps everything to one value.

### Sync point (read this before the next sync)

RC15 is where Turbo re-synced with the **Hyper community line**
(`DaviRain-Su/hyper-grok-build`, remote `community`) and with **xAI upstream**:

| Ref | Commit | Meaning |
|-----|--------|---------|
| xAI upstream | `e5478eff1` (`SOURCE_REV` `27d2088ae…`) | 0.2.119, merged in full |
| Hyper fork point | `c260695cc` | where Turbo and Hyper diverged (2026-07-29) |
| Hyper compared at | `7a48dd755` | Hyper `community/dev` head at audit time |

Cherry-picked from Hyper: `9cd8dffca` (sampler privacy + WASM decode),
`783989c01` (credential rotation, voice), `2c68d9d26` (model config reload),
`d49c3feb3` + `a831b5620` (DeepSeek V4). Hand-ported from Hyper's merge
`488edc10d`, which is reachable from neither branch tip: the circuit-breaker
probe fix.

**Deliberately NOT taken** (evaluated and declined): the Comet desktop app (does
not run on Windows), Hypercore (default-off, would add a second turn engine),
the `packages/*` reorg (git rename detection already bridges the layouts at
2,945 renames, so declining costs nothing), upstream's headless reducer rewrite
(Turbo's `streaming-json` schemaVersion 2 contract is richer), and the Codex
turn-state / remote-compaction work.

### Security

- **`x-grok-*` headers no longer leak to third-party providers.** Turbo stamped
  `x-grok-deployment-id` / `x-grok-user-id` / `x-grok-client-identifier` on every
  sampling request with no base-URL check, and set no redirect policy, so
  reqwest followed up to 10 hops including cross-origin. As the heavier
  multi-provider fork (NVIDIA/Nemotron, OpenRouter, Ollama, Kimi, Azure-style
  proxies) Turbo leaked xAI product/session identity on ordinary traffic. Now
  gated on an HTTPS-only, suffix-safe first-party allowlist, stripped from the
  *finalized* header map so a late injector cannot reintroduce them, and
  redirects follow only same-origin HTTPS.
- **Allow-rule traversal escape** (introduced by the sync, caught by upstream's
  own tests): `./../../etc/passwd` textually matched the glob `./**` while
  denoting `/etc/passwd`. Allow rules no longer match a raw spelling containing
  a parent-dir component; deny/ask keep the full multi-spelling union.
- **Subagent path-segment validation.** Model-supplied task/session/agent ids
  reach filesystem paths in more places in Turbo than upstream and were
  unvalidated. Now fail-closed against traversal, separators, drive/UNC
  prefixes, NUL/control chars, alternate data streams, Windows reserved device
  names, and trailing dot/space.
- **Strict WASM guest decode** — over-long or invalid UTF-8 from a guest is
  rejected rather than silently truncated or lossily decoded.
- **MCP** credential storage hardened against multi-process wipe (via sync).

### Fixed

- **Self-update was broken on both platforms.** `release.yml` ships the whole
  `bundled/` tree; the extractor hard-bailed `"archive entry is nested"` on the
  second path component, so every `bundled/skills/*.md` aborted the update. The
  extractor now accepts `bundled/**` at any depth while rejecting zip-slip,
  absolute/rooted paths, drive prefixes, symlinks, reserved device names and
  depth > 32, with entry/byte caps. Activation is a compensating transaction
  (bundle → binary → state) so a crash leaves either the old bundle or the new
  one, never a merge.
- **Circuit breaker double-admitted half-open probes** — a zero-valued claim
  timestamp was ambiguous with "no claim", and reservation was not atomic
  against state transitions.
- **Credential rotation** now keys on the refresh-token *family* rather than
  access-token expiry, and four paths no longer discard a freshly minted token.
  Fires on concurrent refresh — Turbo's `spawn_many` / worktree profile.
- **`/compact` no longer orphans running background tasks and subagents**, and
  recovers instead of failing when the summarizer input exceeds the window.
- **DeepSeek V4 on Ollama Cloud** advertised 256k context / 32k max output
  against a real **1M / 384k** — users compacted at a quarter of the true window
  and were capped at a twelfth of the real output limit.
- **Windows: bundled skills shipped as CRLF** while Unix got LF, so the two
  platforms shipped different bytes for the same runtime.
- **Windows: mixed CRT.** The workspace sets `+crt-static` but had no CMake
  toolchain file, so a CMake-built native dep (bundled Opus) selected `/MD`
  against a `/MT` Rust runtime.
- **Windows: POSIX shell commands kept working.** The sync replaced the
  always-`sh -c` helper with hardcoded `cmd /C`, which would have silently
  broken any configured `auth_provider_command` using `$VAR`, `exit N` or
  pipes. Now routed through the shared shell detector (Git Bash when present,
  else pwsh/cmd, honouring `GROK_SHELL`).
- Stale RC14 rename leftovers: the published `streaming-json` schema still
  identified as Hyper, and the GitHub Release body still pointed at
  `DaviRain-Su/hyper-grok-build`'s install scripts.

### Added (from upstream 0.2.113–0.2.119)

- **LSP pull diagnostics** — the language server is no longer torn down on
  every edit (headline symptom: C# diagnostics never arriving).
- **`GROK_EXTRA_CA_BUNDLE`** — corporate TLS roots for inspecting proxies.
- **`/model` re-reads `config.toml`** on invocation, replacing only
  config-owned catalog rows so remote/provider/default rows survive.
- **`x.ai/task_completed` frames bounded to 32 KiB** so ACP clients with a
  line-length limit (Python asyncio, most Node readline setups) stop hard-failing
  on a large task log.
- File watching skips nested checkouts; `git-head-changed` fires on same-branch
  commits; session `/delete`; slash-command mode-support matrix; broad TUI
  polish across six releases.

### Changed

- **Rust 1.93.0** (required by upstream 0.2.118+ code and lint config).
- `RUST_MIN_STACK` is set for cargo-run processes: the 0.2.119 prompt-turn
  future exhausts the default 2 MiB Rust thread stack (the PE main thread's
  16 MiB `/STACK` does not cover libtest/tokio threads).
- Clippy bans unenrolled `Command::spawn` — an unenrolled child outlives its
  session.

### Known

- **The project picker is inert.** Upstream removed its own picker in 0.2.119
  and the sync accepted that in the non-conflicted files; Turbo's
  `project_picker` module and `AppView` state remain but nothing triggers them.
  Left in place rather than deleted or re-implemented.
- **The test suite is not green on Windows and never was.** 477 of ~5,900 tests
  fail on `dev` for POSIX reasons (test support hardcodes `/tmp`; some tests
  shell out with `printf` / `${VAR:-default}`). RC15's differential against that
  baseline shows **zero regressions**.
- `--include-partial-messages` is accepted but rejected with a pointer to
  `--output-format streaming-json`: it only ever applied to upstream's
  `streaming-messages-json` reducer, which Turbo does not carry.

---

## [Unreleased]

### Planned (RC16 candidates — feature request log)
Rust `target/` disk hygiene product surface (Windows monorepo often 100–300 GB):
- `turbo disk report` / `turbo disk clean --safe`
- Doctor free-space probe + agent preflight before heavy cargo / release-dist
- Windows PDB / light-agent cargo policy; post-ship cache trim (keep `turbo.exe`)
- Stale absolute-path target self-heal after renames; optional `turbo build ship`
- Auto developer_log on disk pressure

Review: `turbo features list` / `turbo features export`.

## [0.2.114-r14] - 2026-08-04

**Turbo Grok Build RC14** — production `web_fetch`, workflow routing so free-text
deep audits launch real Rhai recipes, English-only UI, docs reset, and product
rename prep (`turbo-grok-build`).

### Added
- **`web_fetch`** — URL → clean markdown for agents (article/full/raw extract,
  SSRF + DNS pin, challenge detection, token-aware windowing/links, browser-shaped
  client, apex↔www redirects, doctor status).
- **Workflow routing** — boot card catalog + system-prompt policy; natural-language
  host soft-match (“run a deep audit on …”) launches stock recipes.
- **Game Mode screenshot** — `docs/assets/screenshot-game-mode.png` for README.

### Changed
- Wire version **`0.2.114-r14`**.
- **English-only UI** — non-English locale packs and zh-CN user guide removed;
  language setting resolves to `en` only.
- **Docs cleanup** — public docs slimmed; historical RC/Q&A material under
  `docs/archive/`; changelog starts fresh from this RC with pedigree table.
- Stock **deep-audit** is the only recommended audit path (no `deep-audit-fixed`).

### Fixed
- Full `web_fetch` security/token hardening suite (allowlist enforcement, non-2xx
  errors, body streaming limits, charset decode, cache purge, overflow budget).

### Notes
- CLI: **`turbo`**. Install: `~/.turbo/bin`. Config/auth still `~/.grok`.
- Rebuild for binary:  
  `cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo`
- Tracking remotes (post-rename):
  - `origin` → your Turbo fork
  - `upstream` → `xai-org/grok-build` (official)
  - `community` → `DaviRain-Su/hyper-grok-build` (Hyper community, fetch-only)
- For **Turbo vs Hyper** compare: `git fetch community` then
  `git log --oneline HEAD..community/dev` (do not merge casually).
