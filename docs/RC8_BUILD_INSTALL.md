# Hyper RC8 (`0.2.114-r8`) — Build & Install (for deploy agent)

**Branch:** `dev`  
**Version:** `0.2.114-r8`  
**Date:** 2026-08-01

## What this release contains

| Area | Change |
|------|--------|
| NVIDIA | Null-tolerant Chat Completions fields (`usage` tokens, indexes) |
| Subagents | `timeout_ms` hard limit on spawn; budget monitor wired |
| Worktrees | Completion surfaces `snapshot_ref` + `worktree_state` after dispose |
| Product | `/deepaudit` / `/deep-audit` deep codebase audit workflow |

## Prerequisites

- **Rust** matching `rust-toolchain.toml` (stable channel pinned in-repo)
- **Windows:** MSVC Build Tools + Windows SDK, or use existing Hyper build env
- **Linux:** glibc floor per install docs (r5+)
- Optional: NVIDIA / OpenAI keys for runtime smoke (not required to build)

## Build (from repo root)

```powershell
# Windows PowerShell
cd "H:\Apps\grok build\hyper-grok-build"   # or clone path
git fetch origin
git checkout dev
git pull origin dev

# Release binary (hyper)
cargo build -p xai-grok-pager-bin --release

# Binary path (Windows)
# target\release\hyper.exe
# or target\release\xai-grok-pager-bin.exe depending on package binary name
```

```bash
# Linux / macOS
cd /path/to/hyper-grok-build
git fetch origin && git checkout dev && git pull origin dev
cargo build -p xai-grok-pager-bin --release
# target/release/hyper
```

Confirm version stamp:

```powershell
.\target\release\hyper.exe --version
# expect 0.2.114-r8 (or marketing string containing r8 / 0.2.114)
```

## Install (user machine)

### Option A — Copy release binary

```powershell
# Windows
New-Item -ItemType Directory -Force -Path "$env:USERPROFILE\.hyper\bin" | Out-Null
Copy-Item -Force .\target\release\hyper.exe "$env:USERPROFILE\.hyper\bin\hyper.exe"
# Ensure %USERPROFILE%\.hyper\bin is on PATH
```

```bash
# Linux / macOS
mkdir -p ~/.hyper/bin
cp -f target/release/hyper ~/.hyper/bin/hyper
chmod +x ~/.hyper/bin/hyper
# add ~/.hyper/bin to PATH
```

### Option B — Project install scripts

```powershell
# From repo after release artifacts exist; or use install.ps1 for remote install
.\install.ps1
```

```bash
./install.sh
```

Runtime state remains under `~/.grok` (config, sessions, worktrees). Binary install root is `~/.hyper`.

## Smoke checks after install

```powershell
hyper --version
hyper --help
# Interactive: /deepaudit --size small
# Interactive: /workflows
```

Optional unit verification before ship:

```powershell
cargo test -p xai-grok-sampling-types --lib null
# NVIDIA Chat Completions deser fixtures (null usage/index/tool_calls):
cargo test -p xai-grok-sampling-types --lib test_chat_completion_response_nvidia_null_fields
cargo test -p xai-tool-types --lib task_tool
cargo test -p xai-grok-tools --lib spawn_many
cargo check -p xai-grok-shell --lib
cargo check -p xai-grok-tools --lib
```

## Deploy notes

1. Do **not** strip `VERSION` / release CI stamps — HTTP client may require minimum version.
2. Shared `~/.grok` with official `grok` is intentional; Hyper binary is separate.
3. Subagent worktrees still snapshot to `refs/grok/subagents/<id>`; completion text now includes `<snapshot_ref>` after cleanup.
4. `/deepaudit large` is expensive (many agents); default is **medium**.

## Rollback

```powershell
git checkout 0.2.114-r7   # or previous tag/commit
cargo build -p xai-grok-pager-bin --release
# re-copy binary to ~/.hyper/bin
```
