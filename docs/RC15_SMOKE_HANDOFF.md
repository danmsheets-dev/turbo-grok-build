# RC15 Smoke-Test Handoff (Agent Brief)

**Product:** Turbo Grok Build · **Wire version:** `0.2.119-r1` · **Branch:** `rc15`  
**Repo:** `H:\Apps\grok build\turbo-grok-build`  
**Goal:** Smoke-test this release **against this monorepo** so bugs can be fixed and features progressed in-tree. Do **not** assume a clean install until explicitly told.

---

## 0. Build artifact (already produced or re-build)

```powershell
cd "H:\Apps\grok build\turbo-grok-build"
$env:RUST_MIN_STACK = "16777216"
$env:GROK_VERSION = (Get-Content VERSION -Raw).Trim()
cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
# Artifact:
#   H:\Apps\grok build\turbo-grok-build\target\release-dist\turbo.exe
& ".\target\release-dist\turbo.exe" --version
```

**Do not install** over `%USERPROFILE%\.turbo\bin` unless the human asks. Prefer running the **path-qualified** binary so the installed `turbo` is undisturbed.

```powershell
$Turbo = "H:\Apps\grok build\turbo-grok-build\target\release-dist\turbo.exe"
& $Turbo --version
```

---

## 1. Mission (what “done” looks like)

1. Exercise the **five feature areas** in a real interactive session **cwd = this repo**.
2. Log every failure with **file paths, repro, expected/actual**.
3. Prefer **fix root causes in this tree** over workarounds.
4. File product friction via `developer_log` / `feature_request_log` when tools work.
5. Produce a short **smoke report** (pass/fail matrix + top 5 bugs + next fixes).

You are allowed to **edit the repo** to fix bugs found during smoke testing.

---

## 2. Environment constraints

| Rule | Detail |
|------|--------|
| CWD | Always start sessions from `H:\Apps\grok build\turbo-grok-build` |
| Binary | Prefer `target\release-dist\turbo.exe` (not PATH `turbo` unless asked) |
| Install | **Do not** run `install.ps1` or overwrite `~/.turbo` unless human says so |
| Isolation | Use `isolation=worktree` subagents when densifying; prove with completion tags |
| Secrets | Never commit tokens; never paste API keys into logs |
| Git | No force-push / reset --hard / amend published history without explicit ask |

---

## 3. How to launch for dogfood (no install)

### Interactive TUI (preferred for smoke)

```powershell
cd "H:\Apps\grok build\turbo-grok-build"
$Turbo = "H:\Apps\grok build\turbo-grok-build\target\release-dist\turbo.exe"
# Optional: isolate product home from production profile
# $env:GROK_HOME = "$env:TEMP\turbo-rc15-smoke-home"
& $Turbo
```

### Headless / scripted where supported

```powershell
& $Turbo --help
& $Turbo --version
# Subagent CLI (non-TUI) when available:
& $Turbo subagent --help
& $Turbo issues --help
& $Turbo features --help
```

### Re-build after code fixes

```powershell
$env:GROK_VERSION = (Get-Content VERSION -Raw).Trim()
cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
```

Use package-scoped tests first (disk hygiene):

```powershell
$env:RUST_MIN_STACK = "16777216"
cargo test -p xai-grok-workspace --lib -- --test-threads=1 --quiet
cargo test -p xai-grok-tools --lib subagent_worktree -- --test-threads=1 --quiet
cargo test -p xai-grok-pager --lib views::game_mode -- --test-threads=1 --quiet
cargo test -p xai-grok-agent --lib boot_card -- --test-threads=1 --quiet
cargo test -p xai-grok-developer-log --lib -- --test-threads=1 --quiet
cargo test -p xai-fast-worktree --lib -- --test-threads=1 --quiet
```

---

## 4. Smoke matrix (execute in order)

Record **Pass / Fail / Blocked** for each row. Fix failures when root-cause is clear.

### A. Binary / launch
| ID | Check | Pass criteria |
|----|--------|----------------|
| A1 | `--version` | Prints `0.2.119-r1` (or current VERSION) |
| A2 | TUI opens on this repo | Welcome/agent UI; no crash |
| A3 | Folder trust | If prompted, trust this repo; session continues |

### B. Game Mode
| ID | Check | Pass criteria |
|----|--------|----------------|
| B1 | Ctrl+G opens office | Game Mode overlays scrollback |
| B2 | Ctrl+G closes | Restores normal view |
| B3 | 2–3 subagents while open | Seats update; no mid-handoff mass clear on 80×24 |
| B4 | Ctrl+G from fullscreen subagent | Parent office toggles (not empty child office) |

### C. Subagent isolation
| ID | Check | Pass criteria |
|----|--------|----------------|
| C1 | Spawn isolation=worktree | Child CWD under `.grok\worktrees\…\subagent-…` |
| C2 | Child boot card honesty | Claims isolation matching real CWD (not parent path) |
| C3 | Edit file in worktree | Parent tree clean until land |
| C4 | `turbo subagent diff <id>` | Shows expected changes |
| C5 | Land merge with conflict | Fail closed; **parent not partially mutated** |
| C6 | Land binary (e.g. small PNG) | Bytes preserved (SHA match) |
| C7 | `allowed_paths` write deny | Write outside prefix fails at write time |
| C8 | Discard deep tree | Completes; long paths OK |

### D. Boot cards + logs
| ID | Check | Pass criteria |
|----|--------|----------------|
| D1 | Primary session has boot card | System context includes Turbo boot card |
| D2 | Child card uses tool CWD | Worktree path, not parent DisplayCwd |
| D3 | `developer_log` call | Creates/dedups under log root |
| D4 | `feature_request_log` call | Creates request; `turbo features list` sees it |
| D5 | `turbo issues list` / `turbo features list` | CLI works against same log roots |

### E. Folder / worktree
| ID | Check | Pass criteria |
|----|--------|----------------|
| E1 | Trust parent → open worktree | No re-prompt for trust (case variants) |
| E2 | Shell outside worktree under isolate | Deny / confine |
| E3 | Apply/land to parent (linked) | Parent files update; worktree not “self” |

### F. Shell correctness (Windows)
| ID | Check | Pass criteria |
|----|--------|----------------|
| F1 | Agent runs failing command | Non-zero exit (`cmd /c exit 7` or bad `git`) |
| F2 | Native `/flag` (if MSBuild available) | Not MSYS-mangled under default pwsh |
| F3 | Session artifacts | Logs under `%TEMP%\grok\sessions\…` not `C:\tmp` |

### G. Repo progress (use smoke to improve this tree)
| ID | Check | Pass criteria |
|----|--------|----------------|
| G1 | File real bugs found as issues | developer_log or issue notes in report |
| G2 | Missing capability as FRL | feature_request_log when appropriate |
| G3 | At least one fix landed if P0 found | Commit-ready patch; tests for regression |

---

## 5. Priority when you find bugs

1. **P0:** Data loss, false-green land, wrong isolation CWD, parent mutated on merge fail, trust open hole  
2. **P1:** Game Mode thrash, allowlist bypass, Windows path identity, log write failures  
3. **P2:** Polish, pixel hit-test, perf  

Fix P0/P1 in this monorepo; re-run the related smoke row + package tests.

---

## 6. Context from prior agent work (do not re-break)

Recent production fixes (verify still present):

- PowerShell `$LASTEXITCODE` wrap (`xai-grok-config` shell)
- envrc Git Bash discovery; pidfile Windows `.lock`
- apply_worktree plan-first merge + binary bytes + `source_repo` parent
- write/search_replace `allowed_paths` enforcement
- Boot card tool CWD + isolation inference
- Game Mode tick uses `last_stage`; Ctrl+G parent-first
- Session folder under `%TEMP%\grok\sessions`

Known deferred (document, don’t block smoke unless critical):

- Pixel desk hit-rects vs sprite anchors  
- Full monorepo Windows test green  
- Windows persistent shell not implemented  
- Land sequential perf  

---

## 7. Deliverable back to human

```markdown
## Smoke report — Turbo 0.2.119-r1 (path-qualified binary)
- Binary: ...\target\release-dist\turbo.exe
- Version: ...
- Matrix: (table of A–G with Pass/Fail)
- P0 found: ...
- P1 found: ...
- Fixes landed: (paths)
- Tests re-run: ...
- Confidence delta: ...
- Recommend: ship dogfood / block / more fixes
```

---

## 8. Install (for a **different** agent — not this smoke agent)

See **Install instructions for install agent** in chat / `docs/RC15_INSTALL_INSTRUCTIONS.md`.  
Smoke agent: **do not install**.
