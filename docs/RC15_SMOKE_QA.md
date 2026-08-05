# RC15 Comprehensive Q&A (smoke + repo progress)

Use this as an interactive script with the **path-qualified** binary  
`target\release-dist\turbo.exe` and **cwd = this monorepo**.  
Each section advances product confidence **and** surfaces work for this tree.

---

## How to use

1. Launch Turbo on `H:\Apps\grok build\turbo-grok-build`.  
2. Ask each **Q** (or run the **Action**).  
3. Fill **Observed** / **Pass?** / **Follow-up**.  
4. For Fail: implement fix in-repo, re-build if needed, re-run that Q.  
5. End with rollup scores for the five features.

---

## 0. Baseline

| # | Q / Action | Expected | Observed | Pass? | Follow-up (issue / PR / log) |
|---|------------|----------|----------|-------|------------------------------|
| 0.1 | What version is `turbo.exe --version`? | Matches `VERSION` (`0.2.119-r1`) | | | |
| 0.2 | Is PATH turbo different from release-dist? | Document both paths | | | Prefer path-qualified |
| 0.3 | Free disk on drive with `target/`? | ≥40 GB recommended | | | AGENTS.md hygiene |
| 0.4 | `git status` dirty from prior agents? | List files; don't lose work | | | |

---

## 1. Game Mode

| # | Q / Action | Expected | Observed | Pass? | Follow-up |
|---|------------|----------|----------|-------|-----------|
| 1.1 | Press **Ctrl+G** | Game Mode office opens | | | |
| 1.2 | Press **Ctrl+G** again | Closes; composer intact | | | |
| 1.3 | Resize 80×24 → 120×40 while open | No crash; tier may change | | | |
| 1.4 | Spawn 3 subagents with Game Mode open | Desks fill; labels update | | | |
| 1.5 | Let one subagent **succeed** | Celebrate / handoff animation (not instant wipe) | | | Stage thrash regression? |
| 1.6 | Let one **fail** | Fail state; no success wall for that desk | | | |
| 1.7 | Open fullscreen subagent, **Ctrl+G** | Parent Game Mode, not empty child office | | | |
| 1.8 | Hover / Tab desk focus (pixel mode) | Highlight near real desk art | | | Pixel hit-test debt |
| 1.9 | Does Ctrl+Shift+G still open Tasks? | Tasks pane (not Game Mode) | | | |
| 1.10 | Feature gap: reduce-motion? | Document if missing | | | `feature_request_log` |

**Feature score after section:** ___%

---

## 2. Subagent isolation

| # | Q / Action | Expected | Observed | Pass? | Follow-up |
|---|------------|----------|----------|-------|-----------|
| 2.1 | Spawn `isolation=worktree` child | CWD under `.grok\worktrees\…\subagent-` | | | |
| 2.2 | Child system text / VERIFY CWD | Tool CWD = worktree (not parent DisplayCwd) | | | Boot card honesty |
| 2.3 | Child completion tags | `<isolation>worktree</isolation>`; no false fallback | | | |
| 2.4 | Child edits `README.md` line | Parent `git status` clean until land | | | |
| 2.5 | `turbo subagent diff <id>` | Diff matches child edits | | | |
| 2.6 | Land **merge** with parent conflict | `success=false`; **no** partial parent writes | | | C16 regression |
| 2.7 | Land **overwrite** after intentional | Parent updated | | | |
| 2.8 | Land a **binary** (add tiny PNG in child) | SHA-256 identical after land | | | Host apply bytes |
| 2.9 | Spawn with `allowed_paths: ["docs/"]`, write `src/x` | Write fails at tool time | | | Write-time allowlist |
| 2.10 | Discard completed worktree | Tree removed / long-path OK | | | |
| 2.11 | Soft-preserve: after complete, tree still on disk? | Soft-preserve default | | | keep-N prune |
| 2.12 | Resume isolation=none | Card says shared / none; not worktree | | | |
| 2.13 | CLI land vs tool land allowlist | Behavior documented | | | Policy skew P1 |

**Feature score after section:** ___%

---

## 3. Boot cards

| # | Q / Action | Expected | Observed | Pass? | Follow-up |
|---|------------|----------|----------|-------|-----------|
| 3.1 | New primary session | Boot card present (workflows, ADL, FRL) | | | |
| 3.2 | Card CWD for primary | This monorepo path | | | |
| 3.3 | Child card isolation string | Matches real layout | | | |
| 3.4 | Card mentions land via CLI / tools | Accurate (no write-time lie) | | | |
| 3.5 | Workspace tree card | Present if index enabled | | | |
| 3.6 | Card budget | No mid-tag truncation of ADL section | | | Truncate P1 |
| 3.7 | Disable ADL via env, restart | Card says disabled | | | |

**Feature score after section:** ___%

---

## 4. Incident log + feature request log

| # | Q / Action | Expected | Observed | Pass? | Follow-up |
|---|------------|----------|----------|-------|-----------|
| 4.1 | Call `developer_log` with isolation_fallback sample | File written; fingerprint | | | |
| 4.2 | Same fingerprint again | `occurrence_count` increments | | | |
| 4.3 | `turbo issues list` | Shows incident | | | |
| 4.4 | Call `feature_request_log` (e.g. land merge UI) | Request stored | | | |
| 4.5 | `turbo features list` | Shows request | | | |
| 4.6 | `turbo issues set-dir` to temp, twice | Second set-dir works on Windows | | | rename fix |
| 4.7 | Tools missing on codex toolset? | Documented gap | | | |
| 4.8 | Real smoke bug filed as ADL | Yes when found | | | |

**Feature score after section:** ___%

---

## 5. Folder worktree / trust / confine

| # | Q / Action | Expected | Observed | Pass? | Follow-up |
|---|------------|----------|----------|-------|-----------|
| 5.1 | Trust this repo folder | Persists | | | |
| 5.2 | Nested worktree under trust | Still trusted (case variants) | | | Trust fold |
| 5.3 | List worktrees by source_repo mixed case | Finds rows | | | path_for_db |
| 5.4 | Shell write outside worktree (isolated child) | Denied by confine | | | session-first confine |
| 5.5 | Land from linked worktree | Parent updated | | | |
| 5.6 | Standalone worktree land (if used) | Parent = source_repo, not self | | | |
| 5.7 | Worktree base under `%USERPROFILE%\.grok\worktrees` | No `C:\tmp` | | | |
| 5.8 | Process `--confine` + isolation child | Tighter session root for shell | | | |

**Feature score after section:** ___%

---

## 6. Windows shell / env / sessions (cross-cutting)

| # | Q / Action | Expected | Observed | Pass? | Follow-up |
|---|------------|----------|----------|-------|-----------|
| 6.1 | Agent: `cmd /c exit 7` | Tool reports exit 7 | | | LASTEXITCODE wrap |
| 6.2 | Agent: bad git command | Non-zero | | | |
| 6.3 | `cd` then second command (non-persistent) | Second does **not** keep cwd | | | Documented |
| 6.4 | Session log path | Under `%TEMP%\grok\sessions\…` | | | |
| 6.5 | `.envrc` if present | Loads via Git Bash when installed | | | |
| 6.6 | `GROK_SHELL=pwsh` does not break auth | Auth still bash/cmd | | | GROK_AUTH_SHELL |

---

## 7. Repo-progress prompts (use smoke to improve **this** tree)

Ask the agent (or yourself) after each failure class:

| # | Prompt | Progress outcome |
|---|--------|------------------|
| 7.1 | "Reproduce the failure with a unit test under the owning crate" | Regression test |
| 7.2 | "Is this product bug or wrong assertion?" | Root-cause discipline |
| 7.3 | "File developer_log if Turbo friction" | ADL coverage |
| 7.4 | "File feature_request_log if capability missing" | FRL coverage |
| 7.5 | "Does deep-audit / continuous-improve fit?" | Workflow dogfood |
| 7.6 | "Update KNOWN_ISSUES.md if deferred" | Docs honesty |
| 7.7 | "Package-scoped cargo test after fix" | Disk-safe CI |
| 7.8 | "Does CHANGELOG Unreleased need a bullet?" | Release hygiene |

### Suggested in-repo workstreams while smoking

1. **Game Mode pixel hit-test** — align `DESK_ANCHORS` with layout desks  
2. **Land perf** — batch `git show` / reduce spawn storm  
3. **apply_patch + hashline allowlist** — extend write-time checks  
4. **Pager full lib green** — remaining fixture debt  
5. **Trust store migration** — re-canonicalize all keys on load  

---

## 8. Rollup

| Feature | Score /100 | Blockers remaining | Ship? |
|---------|------------|--------------------|-------|
| Game Mode | 75 | Interactive Ctrl+G not dogfooded in headless harness | Dogfood |
| Subagent isolation | 90 | Live land-merge-conflict demo deferred | Dogfood |
| Boot cards | 85 | Hardcoded subagents_enabled residual honesty | Dogfood |
| Incident / feature log | 95 | — | Dogfood |
| Folder worktree | 90 | — | Dogfood |
| **Overall** | **87** | Interactive Game Mode + residual honesty | **Ship dogfood** |

**Public release recommendation:** Ship dogfood  

**Full write-up:** `docs/RC15_SMOKE_REPORT.md`

**Top 5 next fixes (ordered):**  
1. Wire boot card `subagents_enabled` from real agent flag  
2. Interactive Game Mode smoke (Ctrl+G, multi-desk, 80×24)  
3. Prevent `H?dev-cachetmp*` junk under crate dirs on Windows tests  
4. Live land merge-conflict + binary PNG SHA integration demo  
5. apply_patch / hashline write-time allowlist parity  

### RC15 smoke session notes (2026-08-05)

| # | Observed | Pass? | Follow-up |
|---|----------|-------|-----------|
| 0.1 | `turbo 0.2.119-r1 (addf1459f)` | Yes | |
| 0.2 | PATH `~\.turbo\bin\turbo.exe` same version; smoke used release-dist | Yes | Prefer path-qualified |
| 4.1–4.6 | issues/features file+list+dedup+set-dir OK; flags are `--class` | Yes | |
| 2.1 | **P0:** `[subagents.models]` alone stripped `spawn_subagent` | Fixed | `SubagentsConfig.enabled` default true |
| 2.1 (retest) | Child CWD under `~\.grok\worktrees\…\subagent-…` | Yes | after rebuild |
| 2.5 / 2.10 | diff + discard OK | Yes | |

---

## 9. Agent system prompt snippet (paste into new agent)

```
You are the RC15 smoke + fix agent for Turbo Grok Build.
Repo: H:\Apps\grok build\turbo-grok-build
Binary: target\release-dist\turbo.exe (path-qualified; DO NOT install)
Read: docs/RC15_SMOKE_HANDOFF.md and docs/RC15_SMOKE_QA.md
Run the Q&A matrix against this monorepo. Fix P0/P1 in-tree.
Re-build with: GROK_VERSION from VERSION; cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
Package-scoped tests only. Report Pass/Fail matrix + patches.
```
