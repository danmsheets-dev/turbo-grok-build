# Smoke report — Turbo 0.2.119-r1 (path-qualified binary)

- **Binary:** `H:\Apps\grok build\turbo-grok-build\target\release-dist\turbo.exe`
- **Version:** `turbo 0.2.119-r1 (addf1459f)` (matches `VERSION`)
- **Branch:** `rc15`
- **CWD for smoke:** `H:\Apps\grok build\turbo-grok-build`
- **Install:** not run (path-qualified only)
- **PATH turbo:** same version at `C:\Users\dan_m\.turbo\bin\turbo.exe` (not used for smoke after rebuild)
- **Disk (H:):** ~227 GB free (OK)
- **Prior dirty tree:** large uncommitted RC15 work present; junk files under `crates/codegen/xai-grok-shell/H?dev-cachetmp…` (pre-existing path pollution) not cleaned as part of product fix

## Matrix A–G

| ID | Check | Result | Notes |
|----|--------|--------|-------|
| **A1** | `--version` | **Pass** | `0.2.119-r1` |
| **A2** | TUI opens on this repo | **Blocked** | No interactive TUI in this harness; headless `-p` works |
| **A3** | Folder trust | **Pass** | Headless sessions on this repo continue without re-prompt; trust tests 42/42 |
| **B1–B2** | Ctrl+G open/close | **Blocked** | Needs interactive TUI; action registry binds Ctrl+G → ToggleGameMode (unit tests) |
| **B3** | 2–3 subagents, no thrash | **Pass (unit)** | `views::game_mode` 24/24; 80×24 stage → Normal tier (office art); compact mass-clear only when `!uses_office_art` |
| **B4** | Ctrl+G from fullscreen child | **Blocked** | Interactive only; code has parent-first ToggleGameMode |
| **C1** | isolation=worktree CWD | **Pass** | Child CWD `C:\Users\dan_m\.grok\worktrees\…\subagent-019fcf94-…` |
| **C2** | Child boot card honesty | **Pass (code + unit)** | tool CWD + `infer_isolation_label`; boot_card 7/7 |
| **C3** | Edit isolated until land | **Pass (unit)** | land/worktree package tests green |
| **C4** | `turbo subagent diff` | **Pass** | Showed baseline_snapshot diff for live marker file |
| **C5** | Land merge fail-closed | **Pass (unit)** | land.rs plan-first; subagent_worktree + land tests green |
| **C6** | Land binary bytes | **Pass (unit)** | `git_capture_bytes` / `apply_file_content` raw bytes |
| **C7** | allowed_paths write deny | **Pass (unit)** | write + search_replace enforce at write time; allowed* tests 20/20 |
| **C8** | Discard deep tree | **Pass** | Discard removed live worktree; long paths OK |
| **D1** | Primary boot card | **Pass** | Present in headless sessions |
| **D2** | Child tool CWD | **Pass** | See C1/C2 |
| **D3** | developer_log | **Pass** | File + dedup (occurrence_count 1→2) |
| **D4** | feature_request_log | **Pass** | Filed + `turbo features list` |
| **D5** | issues/features CLI | **Pass** | path/list/file/set-dir twice works |
| **E1–E3** | Trust / worktree fold | **Pass (unit)** | folder_trust 42/42; apply uses source_repo parent |
| **F1** | Non-zero exit codes | **Pass (unit)** | PowerShell `$LASTEXITCODE` wrap in shell.rs |
| **F2** | Native `/flag` not MSYS-mangled | **Pass (code)** | `MSYS_NO_PATHCONV` / `MSYS2_ARG_CONV_EXCL` on Git Bash |
| **F3** | Session artifacts not `C:\tmp` | **Pass** | Under `%TEMP%\grok\sessions` → `H:\dev-cache\tmp\grok\sessions` |
| **G1** | Real bugs filed | **Pass** | `inc_019fcf7e6c8a731180ed3786f8a8d33d` (P0 tool_schema) + earlier smoke gaps |
| **G2** | FRL when missing | **Pass** | land merge UI + spawn exposure requests |
| **G3** | Fix landed if P0 | **Pass** | See patches below |

## P0 found

1. **`[subagents.models]` (partial section) silently disabled `spawn_subagent`**  
   - **Class:** `tool_schema` / product bug  
   - **Repro:** User config has only `[subagents.models]` (no `enabled = true`). `SubagentsConfig.enabled` deserialized as `false` (bool Default) while `has_local_section=true`, so `resolve_enabled` treated it as intentional disable.  
   - **Symptom:** Boot card “Subagents: enabled”; kill/get tools present; **`spawn_subagent` absent** from model tool schema. Isolation smoke impossible.  
   - **Incident:** `inc_019fcf7e6c8a731180ed3786f8a8d33d`  
   - **Fix:** Default `enabled` to **true** on `SubagentsConfig` Deserialize/Default; regression tests for models/toggle-only sections.  
   - **Verify after rebuild:** Without `GROK_SUBAGENTS=1`, headless session lists `spawn_subagent`; worktree child CWD under `~\.grok\worktrees\…\subagent-…`.

## P1 found

1. **Boot card hardcodes `subagents_enabled: true`** even when task tool stripped — honesty gap (partially masked once config default fixed).  
2. **Interactive Game Mode** (Ctrl+G, multi-desk thrash, pixel hit-test) not exercised in headless harness — unit coverage only.  
3. **Pre-existing junk paths** `crates/codegen/xai-grok-shell/H?dev-cachetmp.*` (colon encoded as private-use char) — hygiene / possible tempfile CWD pollution from prior tests.  
4. **CLI `issues file` / `features file` flag names** (`--class` not `--error-class`) — docs/agent footgun only.

## Deferred (known, not blocking dogfood)

- Pixel desk hit-rects vs sprite anchors  
- Full monorepo Windows test green  
- Windows persistent shell  
- Land sequential perf  

## Fixes landed (this session)

| Path | Change |
|------|--------|
| `crates/codegen/xai-grok-shell/src/config/mod.rs` | `SubagentsConfig.enabled` defaults **true**; custom `Default` |
| `crates/codegen/xai-grok-shell/src/config/tests.rs` | RC15 regressions for models/toggle-only sections; corrected prior wrong expectation |

## Rebuild

```text
cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
# Finished release-dist in ~23m; BUILD_EXIT=0
# Artifact: target\release-dist\turbo.exe still reports 0.2.119-r1
```

## Tests re-run (package-scoped, `RUST_MIN_STACK=16777216`)

| Package / filter | Result |
|------------------|--------|
| `xai-grok-developer-log --lib` | 18 pass |
| `xai-grok-tools --lib subagent_worktree` | 16 pass |
| `xai-grok-tools --lib allowed` | 20 pass |
| `xai-grok-tools --lib land` | 1 pass |
| `xai-grok-agent --lib boot_card` | 7 pass |
| `xai-grok-workspace --lib folder_trust` | 42 pass |
| `xai-grok-config --lib shell` | 6 pass |
| `xai-fast-worktree --lib` | 145 pass |
| `xai-grok-pager --lib views::game_mode` | 24 pass |
| `xai-grok-shell --lib subagents_` | 35 pass (after fix) |

## Feature rollup (from QA)

| Feature | Score /100 | Blockers | Ship? |
|---------|------------|----------|-------|
| Game Mode | 75 | Interactive TUI not dogfooded here; unit green | Dogfood |
| Subagent isolation | 90 | Merge-conflict live land not re-run interactively | Dogfood |
| Boot cards | 85 | Hardcoded subagents_enabled honesty residual | Dogfood |
| Incident / feature log | 95 | — | Dogfood |
| Folder worktree | 90 | — | Dogfood |
| **Overall** | **87** | Interactive Game Mode + residual honesty | **Ship dogfood** |

## Confidence delta

- **Before fix:** Isolation headless smoke **blocked** on dogfood user config (spawn missing).  
- **After fix + rebuild:** Spawn + worktree CWD + CLI open/diff/discard **green**.  
- Core land/allowlist/trust/shell paths covered by package tests.

## Recommend

**Ship dogfood** with path-qualified `target\release-dist\turbo.exe` after this rebuild.

**Do not** treat as full public ship until:
1. Human interactive Game Mode pass (Ctrl+G, multi-desk, 80×24)  
2. One live land-merge-conflict fail-closed demo on a dirty parent  
3. Optional: boot card reflects real `subagents_enabled` when task is stripped  

## Top 5 next fixes

1. Wire boot card `subagents_enabled` from real agent builder flag (honesty)  
2. Interactive Game Mode smoke on real terminal  
3. Clean/prevent `H?dev-cachetmp*` junk under crate dirs on Windows tests  
4. Live land merge conflict + binary PNG SHA demo (integration, not only unit)  
5. `apply_patch` / hashline write-time allowlist parity  

---

*Agent: RC15 smoke + fix · no install · package-scoped tests only*
