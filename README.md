<div align="center">

<h1>Turbo Grok Build</h1>

<img src="docs/assets/turbo-banner.jpg" alt="Turbo Grok Build — multi-agent terminal coding" width="720">

<p>
  <a href="https://github.com/danmsheets-dev/turbo-grok-build/releases"><img src="https://img.shields.io/github/v/release/danmsheets-dev/turbo-grok-build?display_name=tag" alt="Release"></a>
  <a href="https://github.com/danmsheets-dev/turbo-grok-build/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflows/release.yml/badge.svg?branch=dev" alt="Release CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License"></a>
  <img src="https://img.shields.io/badge/version-0.2.114--r14%20RC14-blue" alt="RC14">
  <img src="https://img.shields.io/badge/rust-1.92.0-orange?logo=rust" alt="Rust 1.92">
  <img src="https://img.shields.io/badge/platform-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-lightgrey" alt="Platforms">
  <img src="https://img.shields.io/badge/UI-English-brightgreen" alt="English UI">
</p>

**Turbo Grok Build** (CLI: **`turbo`**) is a heavily extended multi-agent coding
CLI forked from [xAI Grok Build](https://github.com/xai-org/grok-build). It keeps
the Rust TUI core and multi-provider stack, then layers production-grade
**folder worktrees**, recovery tooling, deep-audit workflows, Game Mode, field
logging, and agent orientation that the upstream product does not ship.

Current release line: **RC14** · wire version **`0.2.114-r14`**.

CLI binary: **`turbo`** (installs to `~/.turbo/bin`). Product name: **Turbo Grok Build**.

[What changed](#what-makes-turbo-different) ·
[Feature overview](#feature-overview) ·
[RC history](#community-rc-history) ·
[Install](#installation) ·
[Providers](#providers) ·
[Subagents & worktrees](#subagents--worktrees) ·
[Workflows & deep audit](#workflows--deep-audit) ·
[Web fetch](#web-fetch) ·
[Auto Developer Log](#auto-developer-log) ·
[Build](#building-from-source) ·
[Docs](#documentation) ·
[License](#license)

</div>

---

## What makes Turbo different

Upstream Grok Build is a strong single-session coding agent. **Turbo Grok Build
is a multi-agent development runtime** built on that foundation.

| Area | Upstream Grok Build | **Turbo Grok Build** |
|------|---------------------|----------------------|
| Product focus | Official agent CLI | Community multi-agent platform |
| Subagents | Present | Isolation by default, land/diff/discard, soft-preserve, restore |
| Folder worktrees | Optional / fragile | Default `isolation=worktree`, FS confine, baseline agent-only patches |
| Worktree recovery | Ephemeral / hard to find | Snapshot + baseline refs + `turbo subagent …` CLI |
| Dirty-parent pollution | Diff vs HEAD can explode | Spawn **baseline** refs → agent-only patches; land fails closed if huge |
| Deep audit | — | **`/deepaudit`** recipe (investigate → verify → report) |
| Workflows | Limited | Rhai recipes + NL soft-match + boot-card routing |
| Web content | Search / raw HTTP | **`web_fetch`** URL → clean markdown (token-aware) |
| Agent orientation | System prompt + project rules | **Agent Boot Card** + Workspace Tree atlas |
| Product field signal | `/feedback`, crashes | **Auto Developer Log** + **Feature Request Log** |
| Providers | xAI-centric | Multi-provider (Grok, NVIDIA Integrate, Codex, Kimi, OpenAI, Anthropic, …) |
| UX | TUI | TUI + **Game Mode** (`Ctrl+G` pixel office) |
| Branding / binary | `grok` · `~/.grok` | Product **Turbo Grok Build** · CLI **`turbo`** · binary under `~/.turbo` |

### Highlights (RC14)

RC14 focuses on **agent research tools** and **workflow honesty** (full notes:
[`CHANGELOG.md`](./CHANGELOG.md)):

- **`web_fetch`** — fetch any public URL to cleaned markdown (article/full/raw),
  SSRF-safe, DNS pin, challenge detection, token-aware windowing/links
- **Workflow routing** — boot card + system prompt teach agents to launch
  `deep-audit` / `deep-research` instead of inventing dual-subagent “audits”
- **Natural-language soft-match** — “Can you run a deep audit on the security app”
  launches the real Rhai workflow without a leading `/`
- Stock **`/deepaudit`** remains the only recommended deep-audit path

Prior themes still ship: isolation FS jail (RC12), Workspace Tree inject (RC13),
Game Mode (RC11), baselines + Boot Card + ADL (RC9–10), deep-audit (RC8).

Not affiliated with xAI. Based on Apache-2.0 Grok Build source.

---

## Feature overview

| Feature | What it does | Since |
|---------|----------------|-------|
| **Folder worktrees** | Subagents default to isolated git worktrees under `~/.grok/worktrees/…` | RC7 (harden RC6–RC12) |
| **Land / diff / discard** | Promote or drop child work via tools + `turbo subagent …` | RC8–RC9 |
| **Agent-only baselines** | Diff/land = `baseline..snapshot`, not dirty parent vs HEAD | RC9 |
| **Soft-preserve + keep-N** | Live worktrees kept for review; disk guard + prune | RC9 / RC12 |
| **FS confine (worktree)** | Write path + shell operand jail fail closed | RC12 |
| **`/deepaudit`** | Parallel investigate → independent verify → verified report | RC8 |
| **`/deep-research`** | Bounded research with claim cross-check | earlier + RC14 routing |
| **Workflow tool + NL** | Rhai recipes; free-text maps to stock launches | RC14 |
| **`web_fetch`** | URL → clean text for the model (tokens down, content up) | RC14 |
| **Workspace Tree** | `workspace_tree` / `resolve_path` + session inject card | RC12–RC13 |
| **Agent Boot Card** | Ops brief: tools, isolation, recovery, logging, workflows | RC9 / RC14 |
| **Auto Developer Log** | Structured product issues (`developer_log` + `turbo issues`) | RC9 |
| **Feature Request Log** | Missing capability surface (`feature_request_log` + `turbo features`) | RC11 |
| **Game Mode** | `Ctrl+G` pixel office of supervisor + subagent desks | RC11 |
| **Multi-provider** | Grok, NVIDIA Integrate, Codex, Kimi, OpenAI, Anthropic, … | r2+ |
| **Headless honesty** | Streaming-json tool/subagent events, confine, trust gates | RC6 |

---

## Community RC history

| RC | Wire | Theme |
|----|------|--------|
| **r1–r5** | `0.2.114-r1` … `r5` | Community fork: providers, extensions, OMP resume, Linux glibc 2.17 floor |
| **r6** | `0.2.114-r6` | Isolation + headless honesty (confine is a boundary; isolation fails closed) |
| **r7** | `0.2.114-r7` | **Folder worktrees by default** (`isolation=worktree`) |
| **r8** | `0.2.114-r8` | **Deep audit**, land/diff/discard tools, NVIDIA harden, continuous-improve |
| **r9** | `0.2.114-r9` | Baselines, **Boot Card**, **Auto Developer Log**, soft-preserve |
| **r10** | `0.2.114-r10` | Deep-audit + ADL ship fixes; **Turbo** brand / `turbo` CLI |
| **r11** | `0.2.114-r11` | **Game Mode** + Feature Request Log |
| **r12** | `0.2.114-r12` | Isolation **FS jail**, densify lifecycle, MCP harden, Game Mode polish |
| **r13** | `0.2.114-r13` | **Workspace Tree** inject, densify engines, Game Mode performance |
| **r14** | `0.2.114-r14` | **`web_fetch`** + **workflow routing** (this release) |

Full per-release detail: [`CHANGELOG.md`](./CHANGELOG.md).

---

## Screenshots

| Welcome (English) | Game Mode |
| ----------------- | --------- |
| ![Turbo TUI](docs/assets/screenshot-welcome-en.png) | ![Game Mode](docs/assets/screenshot-game-mode.png) |

---

## Names at a glance

| | Official | This project |
|---|---|---|
| Product | Grok Build | **Turbo Grok Build** |
| CLI binary | `grok` | **`turbo`** |
| Install root | `~/.grok` | **`~/.turbo`** (binary only) |
| Config / auth / sessions | `~/.grok` | **same `~/.grok`** (shared) |
| Release line | upstream cadence | **RC14** · `0.2.114-r14` |
| Upstream | [xai-org/grok-build](https://github.com/xai-org/grok-build) | This fork (+ multi-provider / multi-agent patches) |

GitHub repo: **`turbo-grok-build`**. Product name is **Turbo Grok Build**; CLI is **`turbo`**.

---

## Installation

Prebuilt binaries (macOS arm64/x86_64, Linux arm64/x86_64 glibc 2.17+, Windows x86_64) on
[GitHub Releases](https://github.com/danmsheets-dev/turbo-grok-build/releases):

```sh
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/danmsheets-dev/turbo-grok-build/dev/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/danmsheets-dev/turbo-grok-build/dev/install.ps1 | iex
```

```sh
turbo --version
turbo login          # xAI / Grok session (browser OAuth)
turbo                # start the TUI
```

Pin a release:

```sh
curl -fsSL https://raw.githubusercontent.com/danmsheets-dev/turbo-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r14
```

```powershell
# Windows — pin RC14
irm https://raw.githubusercontent.com/danmsheets-dev/turbo-grok-build/dev/install.ps1 | iex
# or from a clone after a release tag exists:
# .\install.ps1 -Version v0.2.114-r14
```

Installer verifies `SHA256SUMS`, installs to `~/.turbo/bin/turbo`
(`%USERPROFILE%\.turbo\bin\turbo.exe` on Windows).

> **Note:** Prebuilt install requires a published GitHub Release for
> `v0.2.114-r14`. Until then, [build from source](#building-from-source) and copy
> `target/release-dist/turbo` into `~/.turbo/bin`.  
> Repo: [danmsheets-dev/turbo-grok-build](https://github.com/danmsheets-dev/turbo-grok-build).

### Install with Nix

```sh
nix run github:danmsheets-dev/turbo-grok-build#turbo-grok-build -- --version
nix profile install github:danmsheets-dev/turbo-grok-build#turbo-grok-build
```

From a clone:

```sh
git clone https://github.com/danmsheets-dev/turbo-grok-build
cd turbo-grok-build
nix run .#turbo-grok-build -- --version
nix develop
```

---

## Providers

| Platform | Auth | Notes |
| -------- | ---- | ----- |
| xAI / Grok | `turbo login` or `XAI_API_KEY` | First-party models |
| NVIDIA Integrate | platform key | Large catalog; Turbo adds agent-ready hardening |
| Kimi Code / Moonshot | OAuth / API key | `kimi-code/*`, open platform |
| ChatGPT Codex | ChatGPT OAuth | GPT-5.x + experimental live voice |
| OpenCode Go | subscription key | Chat Completions + Messages |
| OpenAI / Anthropic / DeepSeek-style | BYOK | Catalog platforms |
| Z.AI Coding Plan | platform key | International plan |
| Ollama Cloud | API key | Live roster sync |

Model ids look like `{platform}/{model}`. Config and credentials stay under
**`~/.grok`** (shared with upstream Grok Build).

---

## Subagents & worktrees

Turbo treats subagents as first-class workers with **folder worktrees** and
recovery (isolation-by-default since **RC7**, fail-closed + FS confine through
**RC12**):

```text
spawn (isolation=worktree)
  → spawn baseline  refs/grok/subagent-baselines/<id>
  → agent works in ~/.grok/worktrees/<slug>/subagent-<id>
  → complete snapshot refs/grok/subagents/<id>
  → soft-preserve live tree (or clean if GROK_SUBAGENT_SOFT_PRESERVE=0)
  → agent-only patch: baseline..snapshot
```

```bash
turbo subagent list
turbo subagent open <id>
turbo subagent open <id> --restore          # materialize snapshot
turbo subagent diff <id>
turbo subagent land <id>                    # refuses huge dirty-tree patches
turbo subagent discard <id>
```

Optional:

| Env | Effect |
|-----|--------|
| `GROK_SUBAGENT_SOFT_PRESERVE=0` | Delete live tree immediately after snapshot |
| `GROK_SUBAGENT_SOFT_PRESERVE_KEEP_N` | Max soft-preserved live trees (default 6) |
| `GROK_SUBAGENT_MIN_FREE_BYTES` | Free-space floor before create (default 2 GiB) |
| `GROK_SUBAGENT_WORKTREE_SEED=clean` | HEAD-only sandbox (no parent dirty copy) |
| `retain_worktree=true` on spawn | Keep path until land/discard |

Details: [`docs/RC9_FEATURES.md`](docs/RC9_FEATURES.md),
[`CHANGELOG.md`](./CHANGELOG.md) (RC7–RC13).

---

## Workflows & deep audit

Stock Rhai workflows (background; progress in `/workflows`):

| Recipe | Slash / NL | What it does |
|--------|------------|--------------|
| **deep-audit** | `/deepaudit`, `/deep-audit`, `/ultracode` · “run a deep audit on …” | Parallel find → independent verify → verified-only report (read-only) |
| **deep-research** | `/deep-research <query>` · “deep-research on …” | Bounded research shards + claim cross-check + citations |
| **continuous-improve** | `/workflow continuous-improve` | Research → plan → implement (worktree) → verify loop |

Agents with the `workflow` tool are instructed (Boot Card + system prompt) to
**launch these recipes** instead of spawning two ad-hoc explore/review
subagents. Host soft-match also intercepts clear free-text audit/research
requests when workflows are enabled (`GROK_WORKFLOWS=0` to disable).

```text
/deepaudit --size medium crates/codegen/xai-grok-tools
Can you run a deep audit on the security app
/deep-research Compare Postgres 17 vs MySQL 9 migration risks
```

User/project recipes: `~/.grok/workflows/*.rhai` and `.grok/workflows/*.rhai`
(filename should match `meta.name`).

---

## Web fetch

**`web_fetch`** turns a URL into **agent-usable clean text** (HTML → markdown,
article extract, windowing) with SSRF protection, DNS pin on direct egress,
challenge detection, and token-aware defaults (links off by default).

```text
# Agent tool (when enabled — default on)
web_fetch url=https://example.com/docs extract_mode=article
```

Disable: `GROK_WEB_FETCH=0` or `[features] web_fetch = false`. Optional
enterprise allowlist: `[toolset.web_fetch] allowed_domains = […]`. See user
guide configuration and [`CHANGELOG.md`](./CHANGELOG.md) RC14.

---

## Auto Developer Log

Agents are instructed (Boot Card) to **always** file product friction with the
`developer_log` tool. Humans triage with:

```bash
turbo issues list
turbo issues show <id>
turbo issues export --severity p0 --out ./turbo-issues-pack
turbo issues path
turbo issues set-dir D:/TurboLogs/developer-log   # persist custom root
turbo issues clear-dir
```

| Precedence | Log directory source |
|------------|----------------------|
| 1 | `turbo issues set-dir` (session + `~/.grok/developer-log.toml`) |
| 2 | `GROK_DEVELOPER_LOG_DIR` |
| 3 | `~/.grok/developer-log.toml` |
| 4 | Default `~/.grok/developer-log` |

Disable: `GROK_DEVELOPER_LOG=0`. Full writeup: [`docs/AUTO_DEVELOPER_LOG.md`](docs/AUTO_DEVELOPER_LOG.md).

---

## Agent Boot Card

On each **new** session (and by default on resume), Turbo injects a short system
briefing (`<turbo_boot_card>`) covering tools, **workflows catalog**, subagent
lifecycle, recovery commands, land safety, and **required** `developer_log` /
`feature_request_log` usage. Subagents get a tiny child stub only.

```text
GROK_BOOT_CARD=off|short|full    # default short
GROK_BOOT_CARD_ON_RESUME=0       # disable inject on resume
```

Workspace Tree inject (RC13) adds a budgeted `<workspace_tree_card>` atlas after
the boot card when indexing is enabled.

---

## Game Mode

`Ctrl+G` opens a pixel **office** view of the Supervisor (main agent) and
subagent desks (added **RC11**, polished RC12–RC13). Chat composer stays
available. Compact terminals fall back to a simpler layout. Tasks pane:
`Ctrl+Shift+G`.

![Game Mode](docs/assets/screenshot-game-mode.png)

---

## Building from source

Requirements: Rust (`rust-toolchain.toml`), [DotSlash](https://dotslash-cli.com)
for `bin/protoc`, CMake 3.5+.

```sh
cargo run -p xai-grok-pager-bin              # TUI (binary name: turbo)
GROK_VERSION=$(cat VERSION) cargo build -p xai-grok-pager-bin --profile release-dist
./target/release-dist/turbo --version
```

Windows PowerShell:

```powershell
$env:GROK_VERSION = (Get-Content VERSION -Raw).Trim()
cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
# Binary: target\release-dist\turbo.exe  (do not install over ~/.turbo until ready)
```

Community branding / updater: `--features community-build` (default on this tree).

### Disk hygiene (important on Windows)

This monorepo’s `target/` can grow past **100–200 GB** (debug incremental + PDBs,
plus independent `release-dist` caches). Prefer package-scoped `cargo check` /
`cargo test`. Before large builds, free space should be ≥40 GB (see
[`AGENTS.md`](./AGENTS.md)). After a successful ship binary:

```powershell
# Keep turbo.exe; drop release-dist rebuild caches (optional)
Remove-Item -Recurse -Force target\debug -ErrorAction SilentlyContinue
# Or only: target\release-dist\{incremental,deps,build,.fingerprint}
```

RC15 plans a productized `turbo disk report|clean` surface (see feature request
log / [`CHANGELOG.md`](./CHANGELOG.md) Unreleased).

---

## Changelog & known issues

- [`CHANGELOG.md`](./CHANGELOG.md) — **official changelog** · **RC14** (`0.2.114-r14`) is current
- [`docs/KNOWN_ISSUES.md`](./docs/KNOWN_ISSUES.md)
- [`docs/workspace-tree.md`](docs/workspace-tree.md) — Workspace Tree (RC12–RC13)
- [`docs/archive/RC11_RELEASE_NOTES.md`](docs/archive/RC11_RELEASE_NOTES.md) — Game Mode (historical)
- [`docs/archive/RC9_FEATURES.md`](docs/archive/RC9_FEATURES.md) — worktrees, Boot Card, ADL (historical)
- [`docs/archive/Q&A/rc9/RC10_HARNESS_FIX_PLAN.md`](docs/archive/Q&A/rc9/RC10_HARNESS_FIX_PLAN.md) — RC10 harness matrix (historical)

---

## Releasing

1. Set root [`VERSION`](VERSION) to the monorepo lockstep client version (stamp
   `x-grok-client-version`; xAI rejects clients below **0.1.202**).
2. Update `CHANGELOG.md`, commit on `dev`.
3. Tag and push:

```sh
VERSION=$(tr -d '[:space:]' < VERSION)
git tag "v${VERSION}"
git push origin "v${VERSION}"
```

Workflow: [`.github/workflows/release.yml`](.github/workflows/release.yml).
Artifacts ship as `turbo-<version>-<target>.tar.gz` / `.zip` + `SHA256SUMS`.

---

## Coexistence with official `grok`

| Surface | Official | Turbo (`turbo`) |
|---------|----------|-----------------|
| Binary | `grok` | `turbo` |
| Install root | `~/.grok/bin` | `~/.turbo/bin` |
| Config / auth / sessions | `~/.grok` | **same** |
| Leader IPC | under `~/.grok` | **same namespace** |

- Sessions and OAuth are shared — log in once for both.
- Updater is isolated: Turbo never overwrites `~/.grok/bin/grok`.
- Not affiliated with xAI / SpaceXAI.

---

## Documentation

| Doc | Content |
|-----|---------|
| [User guide](crates/codegen/xai-grok-pager/docs/user-guide/) | Product how-to (CLI is `turbo`) |
| [Changelog](./CHANGELOG.md) | RC14 + pedigree table |
| [Workspace Tree](docs/workspace-tree.md) | Atlas / inject / CLI |
| [Auto Developer Log](docs/AUTO_DEVELOPER_LOG.md) | Field logging for maintainers |
| [Feature Request Log](docs/FEATURE_REQUEST_LOG.md) | Missing-capability log |
| [Archive](docs/archive/) | Historical RC plans / Q&A (not product surface) |
| Official Grok Build | [docs.x.ai/build](https://docs.x.ai/build/overview) |

`SOURCE_REV` records the last monorepo sync point.

### Tracking remotes (for cherry-picks / Turbo vs Hyper compare)

| Remote | URL | Use |
|--------|-----|-----|
| `origin` | `danmsheets-dev/turbo-grok-build` | This Turbo fork (`dev` default) |
| `upstream` | `xai-org/grok-build` | Official Grok Build (fetch-only) |
| `community` | `DaviRain-Su/hyper-grok-build` | Hyper community (fetch-only) |

Turbo and Hyper share a common history but diverge after the rebrand. Hyper
`community/dev` is often ahead on wire version (e.g. 0.2.119-rN vs Turbo
0.2.114-r14). Use fetch + log for compare; do not merge casually.

```sh
git fetch origin
git fetch upstream
git fetch community
# Inspect without merging:
git log --oneline HEAD..community/dev | head
git log --oneline community/dev..HEAD | head
git rev-list --count HEAD..community/dev   # commits Hyper has that Turbo lacks
git rev-list --count community/dev..HEAD   # commits Turbo has that Hyper lacks
git show community/dev:VERSION
git show HEAD:VERSION
```

---

## Repository layout

| Path | Contents |
|------|----------|
| `crates/codegen/xai-grok-pager-bin` | Composition root → `turbo` binary |
| `crates/codegen/xai-grok-pager` | TUI |
| `crates/codegen/xai-grok-shell` | Agent runtime / subagents |
| `crates/codegen/xai-grok-developer-log` | Auto Developer Log store |
| `crates/codegen/xai-grok-agent` | Prompt assembly / Boot Card |
| `install.sh` / `install.ps1` | Release installers |
| `.github/workflows/release.yml` | Multi-target release CI |

> [!IMPORTANT]
> Root `Cargo.toml` is often treated as monorepo-generated — prefer editing
> per-crate manifests for durable local changes.

---

## License

Apache-2.0. See [`LICENSE`](LICENSE), [`NOTICE`](NOTICE), and
[`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES).

Based on [xai-org/grok-build](https://github.com/xai-org/grok-build).
**Turbo Grok Build** is an independent community fork — not an official xAI product.
