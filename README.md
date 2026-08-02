<div align="center">

<h1>Grok Build Turbo</h1>

<img src="docs/assets/turbo-banner.jpg" alt="Grok Build Turbo — multi-agent terminal coding" width="720">

<p>
  <a href="https://github.com/danmsheets-dev/hyper-grok-build/releases"><img src="https://img.shields.io/github/v/release/danmsheets-dev/hyper-grok-build?display_name=tag" alt="Release"></a>
  <a href="https://github.com/danmsheets-dev/hyper-grok-build/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflows/release.yml/badge.svg?branch=dev" alt="Release CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License"></a>
  <img src="https://img.shields.io/badge/rust-1.92.0-orange?logo=rust" alt="Rust 1.92">
  <img src="https://img.shields.io/badge/platform-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-lightgrey" alt="Platforms">
  <img src="https://img.shields.io/badge/i18n-10%20locales-brightgreen" alt="i18n">
</p>

**Grok Build Turbo** is a heavily extended multi-agent coding CLI forked from
[xAI Grok Build](https://github.com/xai-org/grok-build). It keeps the Rust TUI
core and multi-provider stack, then layers production-grade subagent worktrees,
recovery tooling, field logging, and agent orientation that the upstream
product does not ship.

CLI binary: **`turbo`** (installs to `~/.turbo/bin`). Product name: **Grok Turbo** / **Grok Turbo Beta**.

[What changed](#what-makes-turbo-different) ·
[Install](#installation) ·
[Providers](#providers) ·
[Subagents & worktrees](#subagents--worktrees) ·
[Auto Developer Log](#auto-developer-log) ·
[Build](#building-from-source) ·
[Docs](#documentation) ·
[License](#license)

</div>

---

## What makes Turbo different

Upstream Grok Build is a strong single-session coding agent. **Turbo is a
multi-agent development runtime** built on that foundation.

| Area | Upstream Grok Build | **Grok Build Turbo** |
|------|---------------------|----------------------|
| Product focus | Official agent CLI | Community multi-agent platform |
| Subagents | Present | Isolation by default, land/diff/discard, soft-preserve, restore |
| Worktree recovery | Ephemeral / hard to find | Snapshot + baseline agent-only diffs + `turbo subagent …` CLI |
| Dirty-parent pollution | Diff vs HEAD can explode | Spawn **baseline** refs → agent-only patches; land fails closed if huge |
| Agent orientation | System prompt + project rules | **Agent Boot Card** (ops brief, recovery, required field logging) |
| Product field signal | `/feedback`, crashes | **Auto Developer Log** (`developer_log` tool + `turbo issues`) |
| Providers | xAI-centric | Multi-provider (Grok, NVIDIA Integrate, Codex, Kimi, OpenAI, Anthropic, …) |
| Reliability track | Upstream cadence | RC7→RC8→RC9 community reliability + deep-audit workflows |
| Branding / binary | `grok` · `~/.grok` | Product **Grok Turbo** · CLI **`turbo`** · binary under `~/.turbo` |

### Highlights (RC8–RC9)

- **Isolated worktrees that stay recoverable** — soft-preserve by default, `turbo subagent open|diff|land|discard`, `open --restore`, agent-only baselines
- **Agent Boot Card** — every new session gets a short ops briefing (recovery CLI, land safety, required logging)
- **Auto Developer Log** — agents must file structured product issues; configurable log directory
- **Land safety** — refuse mega-patches from dirty-tree pollution unless `force=true`
- **`/deepaudit` / continuous-improve** — multi-phase audit workflows
- **Copy affordance** on completed messages (selection copy icon on by default)

Not affiliated with xAI. Based on Apache-2.0 Grok Build source.

---

## Screenshots

| English | 简体中文 |
| ------- | -------- |
| ![Turbo TUI (English)](docs/assets/screenshot-welcome-en.png) | ![Turbo TUI (中文)](docs/assets/screenshot-welcome-zh.png) |

---

## Names at a glance

| | Official | This project |
|---|---|---|
| Product | Grok Build | **Grok Turbo** (Grok Turbo Beta) |
| CLI binary | `grok` | **`turbo`** |
| Install root | `~/.grok` | **`~/.turbo`** (binary only) |
| Config / auth / sessions | `~/.grok` | **same `~/.grok`** (shared) |
| Upstream | [xai-org/grok-build](https://github.com/xai-org/grok-build) | This fork (+ multi-provider / multi-agent patches) |

Repo folder/history may still say `hyper-grok-build`. Product/CLI name is **Turbo** (`turbo`).

---

## Installation

Prebuilt binaries (macOS arm64/x86_64, Linux arm64/x86_64 glibc 2.17+, Windows x86_64) on
[GitHub Releases](https://github.com/danmsheets-dev/hyper-grok-build/releases):

```sh
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/danmsheets-dev/hyper-grok-build/dev/install.sh | bash
```

```powershell
# Windows PowerShell
irm https://raw.githubusercontent.com/danmsheets-dev/hyper-grok-build/dev/install.ps1 | iex
```

```sh
turbo --version
turbo login          # xAI / Grok session (browser OAuth)
turbo                # start the TUI
```

Pin a release:

```sh
curl -fsSL https://raw.githubusercontent.com/danmsheets-dev/hyper-grok-build/dev/install.sh | bash -s -- --version v0.2.114-r9
```

Installer verifies `SHA256SUMS`, installs to `~/.turbo/bin/turbo`
(`%USERPROFILE%\.turbo\bin\turbo.exe` on Windows).

### Install with Nix

```sh
nix run github:danmsheets-dev/hyper-grok-build#turbo-grok-build -- --version
nix profile install github:danmsheets-dev/hyper-grok-build#turbo-grok-build
```

From a clone:

```sh
git clone https://github.com/danmsheets-dev/hyper-grok-build
cd hyper-grok-build
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

Turbo treats subagents as first-class workers with isolation and recovery:

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
| `GROK_SUBAGENT_WORKTREE_SEED=clean` | HEAD-only sandbox (no parent dirty copy) |
| `retain_worktree=true` on spawn | Keep path until land/discard |

Details: [`docs/RC9_FEATURES.md`](docs/RC9_FEATURES.md),
[`docs/HYPER_DEVELOPER_FEEDBACK.md`](docs/HYPER_DEVELOPER_FEEDBACK.md).

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

On each **new** session, Turbo injects a short system briefing
(`<hyper_boot_card>`) covering tools, subagent lifecycle, recovery commands,
land safety, and **required** `developer_log` usage. Subagents get a tiny child
stub only.

```text
GROK_BOOT_CARD=off|short|full    # default short
```

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
```

Community branding / updater: `--features community-build` (default on this tree).

---

## Changelog & known issues

- [`CHANGELOG.md`](./CHANGELOG.md)
- [`docs/KNOWN_ISSUES.md`](./docs/KNOWN_ISSUES.md)
- [`docs/RC9_FEATURES.md`](./docs/RC9_FEATURES.md)

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
| [User guide (EN)](crates/codegen/xai-grok-pager/docs/user-guide/) | Product how-to (examples may say `grok`; CLI is `turbo`) |
| [用户指南 (中文)](crates/codegen/xai-grok-pager/docs/user-guide-zh-CN/) | Chinese guide |
| [RC9 features](docs/RC9_FEATURES.md) | Worktrees, Boot Card, copy UI, ADL |
| [Auto Developer Log](docs/AUTO_DEVELOPER_LOG.md) | Field logging for maintainers |
| Upstream | [docs.x.ai/build](https://docs.x.ai/build/overview) |

`SOURCE_REV` records the last monorepo sync point.

中文 README (may lag brand update): [README.zh-CN.md](README.zh-CN.md)

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
**Grok Build Turbo** is an independent community fork — not an official xAI product.
