# RC12 Harness Audit & Q&A Plan

| Item | Content |
|------|---------|
| Product | Turbo Grok Build (`turbo`) |
| Wire version | **0.2.114-r12** |
| Date | 2026-08-03 |
| Binary under test | `turbo 0.2.114-r12 (7b9464885)` (same as this session) |
| Phase | **1 complete** → Phase 2 Q&A next |
| Audits | 3× Grok 4.5 explore subagents (isolation/worktree, workspace-tree+ADL/FRL+MCP, Game Mode+deepaudit) + live CLI + open ADL |

---

## 0. Executive summary

RC12 is a **ship-with-notes** isolation release plus densify-scale lifecycle, Workspace Tree tools, Game Mode visual polish, and MCP timeout/catalog hardening.

| Area | Code audit | Live Q&A priority |
|------|------------|-------------------|
| Isolation FS jail + land/diff/discard | **SHIPPED** (policy jail, not OS sandbox) | **P0** re-verify e2e |
| Worktree live marker / keep-N / disk guard / health | **SHIPPED** | **P0** stress + open incidents |
| Coordinator queue / cancel / teardown | **SHIPPED** | **P1** densify storms |
| Workspace Tree tools | **PARTIAL** (tools yes; inject/slash/CLI no) | **P1** tools; FR for gaps |
| ADL + Feature Request Log | **SHIPPED** | **P1** live tool path |
| MCP 120s / list_changed / remap / ACL | **SHIPPED** | **P2** |
| Game Mode hover + SNES floor + Ctrl+G | **SHIPPED** (docs lag) | **P2** manual TUI |
| deepaudit slug normalize + ValidateType 15s | **SHIPPED** | **P2** |

### Open ADL incidents (pre-existing, densify session)

| Sev | ID | Class | Title |
|-----|-----|-------|-------|
| **P0** | `inc_019fc8a670fa77d0b8428a9dabf7b2f5` | work_lost_risk | Write reports success but file not readable; shell CWD tombstoned |
| **P1** | `inc_019fc8a2d77b7b23a698d7f1966837df` | worktree_tombstone | Subagent worktree CWD invalid (os error 267) |
| **P2** | `inc_019fc8b26b397661aede60a94ebe85fe` | work_lost_risk | Supervisor prune raced live densify worktrees |

These three are **Phase 3 fix targets** after Phase 2 repro. Theme: **worktree deleted under a live child** (prune race / tombstone) while write tools still report success.

### Feature Request Log

No open FRs under `H:\Apps\grok build\feature request`. Workspace Tree design gaps (inject card, `/tree`, CLI, freshness) should be filed as FRs during Phase 2 if still desired.

---

## 1. RC12 claim verification (code)

### 1.1 Isolation / worktree (19 claims)

| # | Claim | Status |
|---|--------|--------|
| 1 | FS jail: ConfineRoot, ConfinedFs, DisplayCwd remap, enforce_write_path, shell EditPathContext | **SHIPPED** (policy) |
| 2 | Resume isolation fail-closed | **SHIPPED** |
| 3 | Land allowlist fail-closed | **SHIPPED** |
| 4 | Land baseline missing refuse without force | **SHIPPED** |
| 5 | Discard always advances meta | **SHIPPED** |
| 6 | `.grok-subagent-live` marker | **SHIPPED** |
| 7 | Pre-spawn disk guard (default 2 GiB) | **SHIPPED** |
| 8 | Worktree materialize health check | **SHIPPED** |
| 9 | Land `only_missing` | **SHIPPED** |
| 10 | Manifest union-merge | **SHIPPED** |
| 11 | Atomic land write + retry | **SHIPPED** |
| 12 | Completion isolation / worktree fields | **SHIPPED** |
| 13 | `spawn_background` enqueue before return | **SHIPPED** |
| 14 | Queue visible in `get_task_output` | **SHIPPED** |
| 15 | `SubagentCancelTarget::LoopTaskId` | **SHIPPED** |
| 16 | Session teardown drains queue | **SHIPPED** |
| 17 | `LoopUnitActive` counts queued | **SHIPPED** |
| 18 | Soft-preserve keep-N (default 6), never prune live | **SHIPPED** |
| 19 | Boot card isolation honesty | **SHIPPED** (path heuristic) |

### 1.2 Workspace Tree

| Piece | Status |
|-------|--------|
| `xai-workspace-tree` crate + durable store | **SHIPPED** |
| Tools `workspace_tree` / `resolve_path` on default/explore/plan | **SHIPPED** |
| Session kickoff (trusted, non-blocking) | **SHIPPED** |
| Path miss suggestions (cache-only) | **SHIPPED** |
| Session inject “tree card” | **MISSING** (design Phase 1) |
| `/tree` slash, `turbo tree` CLI | **MISSING** |
| Honest freshness / incremental / worktree overlay | **MISSING / stub** |
| Tools on orchestrator/concise/hashline | **NOT** on those sets (changelog only claims default/explore/plan) |

### 1.3 ADL / FRL / MCP / Game Mode / deepaudit

| Area | Status |
|------|--------|
| `developer_log` + `feature_request_log` tools + CLI | **SHIPPED** |
| Boot card routing bugs vs FRs | **SHIPPED** |
| MCP default timeout 120s | **SHIPPED** |
| `tools/list_changed` re-register | **SHIPPED** |
| Qualified name remap | **SHIPPED** |
| Hub safety ceiling + Windows OAuth ACL | **SHIPPED** |
| Game Mode Ctrl+G, hover, SNES floor | **SHIPPED** (user-guide still wrong on Ctrl+G) |
| deepaudit model slug normalize | **SHIPPED** |
| ValidateType timeout 15s | **SHIPPED** |
| Cancelled children not in completion buffer | **SHIPPED** |

### 1.4 Residual risks (not closed by RC12)

| Sev | Risk |
|-----|------|
| **P1** | Shell confine ≠ OS FS jail (KNOWN_ISSUES accepted) |
| **P1** | Baseline soft-fail on spawn → land later refuses without force |
| **P1** | Soft-preserve + `land_status=pending` misread as “isolation worked” |
| **P1** | CLI land vs tool land policy skew |
| **P2** | User guide soft-preserve “removed by default” contradiction |
| **P2** | Docs lag: Ctrl+G still tasks in user-guide / design Shift+G |
| **P2** | Boot card isolation is path-string heuristic |
| **P0 open** | Worktree tombstone under live densify (ADL P0/P1) |

---

## 2. Q&A matrix — everything to test

Priority: **P0** must pass before densify/ship confidence · **P1** RC12 new surfaces · **P2** polish/docs · **P3** design backlog FRs.

### Wave A — Isolation fail-closed (P0)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **ISO-01** | Absolute parent path write remaps into worktree | File under `~/.grok/worktrees/.../subagent-<id>/`, **not** parent | Spawn write child isolation=worktree; Write abs parent path |
| **ISO-02** | Shell abs write to parent denied | Confine/deny; parent clean | Child bash `echo x > <parent-abs>` |
| **ISO-03** | Shell residual escape (known risk) | Document outcome; ADL if product hole | Non-operand write program |
| **ISO-04** | Resume non-worktree + isolation=worktree fail-closed | Spawn fails without `ALLOW_SHARED_FALLBACK` | isolation=none complete → resume worktree |
| **ISO-05** | Shared fallback opt-in honesty | Tags `isolation_fallback` / summary not pure worktree | Same as ISO-04 with env=1 |
| **ISO-06** | Write-time `allowed_paths` | Write outside prefix denied at write, not only land | Narrow allowlist + write OOB |
| **ISO-07** | capability_mode=read-only on GP | No write/search_replace tools; write denied if forced | Spawn GP + RO + isolation=none |
| **ISO-08** | explore has no write tools | Tool list RO | Spawn explore |
| **ISO-09** | Completion isolation fields | Summary has isolation + worktree_path/state | After successful worktree child |
| **ISO-10** | Boot card honesty | Child card CWD + isolation match reality | Inspect child system context / tags |

### Wave B — Land / discard / baseline (P0–P1)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **LAND-01** | Agent-only land (`baseline..snapshot`) | Land applies only agent files, not dirty-parent inflate | Edit in worktree; land_subagent |
| **LAND-02** | Land allowlist invalid → deny-all | `path_allowlist_invalid` / refuse | Bad `allowed_paths` |
| **LAND-03** | Land allowlist OOB refuse | `path_allowlist_violation` | Edit outside prefix |
| **LAND-04** | Baseline missing refuse | `land_baseline_missing` unless force | Clear baseline_ref / force path |
| **LAND-05** | `only_missing` | Only new paths land; existing parent files skipped | Parent+child both have foo; child adds bar |
| **LAND-06** | Manifest union-merge | Name-keyed merge under `assets/manifest/*.json` | Concurrent densify-style manifests |
| **LAND-07** | Atomic land Windows | Retry under sharing; no silent partial without report | Optional open-handle stress |
| **LAND-08** | Discard when path already gone | Meta `discarded` / `cleaned` / removed=true | rm worktree then discard_subagent |
| **LAND-09** | CLI vs tool land | Prefer tool; document CLI gaps | Compare `turbo subagent land` vs tool |
| **LAND-10** | diff_subagent filter allowlist | Diff only shows in-allowlist paths | allowed_paths + multi-file child |

### Wave C — Worktree lifecycle / densify scale (P0 — open incidents)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **WT-01** | Live marker written | `.grok-subagent-live` exists while RUNNING | Spawn worktree; inspect path |
| **WT-02** | keep-N never deletes live | Concurrent keep-N+2; live trees survive | Spawn ≥8 isolation=worktree |
| **WT-03** | Soft-preserve after complete | Tree on disk; land_status pending | Default soft-preserve |
| **WT-04** | Materialize health check | Empty/broken checkout refused | Break path / empty dir |
| **WT-05** | Disk guard | Huge MIN_FREE → refuse create (or fallback only with env) | Env override |
| **WT-06** | **Tombstone: write after worktree deleted** | Write must **fail** (not success + missing file) | Repro ADL P0; mid-run delete CWD |
| **WT-07** | **Tombstone: shell after delete** | Shell fails closed with clear error; no fake success | Repro ADL P1 os error 267 |
| **WT-08** | **Prune race** | Product prune never deletes RUNNING live markers; supervisor bulk rm is footgun | Repro ADL P2 |
| **WT-09** | `turbo subagent open --restore` | Restores usable worktree from snapshot | Completed soft-preserved child |
| **WT-10** | Snapshot recovery without live tree | `git show refs/grok/subagents/<id>:<path>` works | After cleaned worktree |

### Wave D — Coordinator / scheduler (P1)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **COORD-01** | spawn_background race | Immediate get_task_output never not_found for enqueued ids | Concurrency cap + burst |
| **COORD-02** | Queue visibility | Queued status visible in inspect/get output | Cap=1, N>1 spawns |
| **COORD-03** | Cancel drops queue | Cancel queued id → cancelled, not later start | Queue then kill |
| **COORD-04** | LoopTaskId cancel | Scheduler delete cancels all children + queue | Loop multi-child |
| **COORD-05** | No post-delete check-loop fire | LoopUnitActive false after cancel | Scheduler delete race |
| **COORD-06** | Session teardown drains queue | Queued get parent-torn-down cancel | Tear down mid-queue |
| **COORD-07** | Cancelled not in completion buffer | Parent turn has no completion noise for cancel/kill | Cancel mid-run |
| **COORD-08** | ValidateType under densify load | No flaky 2s-era coordinator unreachable | Storm + ValidateType |

### Wave E — Workspace Tree (P1)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **TREE-01** | `workspace_tree` summary | Top dirs + stats; target/node_modules collapsed | This monorepo session |
| **TREE-02** | `resolve_path` basename | Ranked real paths for known file | e.g. `boot_card.rs` |
| **TREE-03** | `workspace_tree` search | Finds Cargo.toml / known names | action=search |
| **TREE-04** | refresh + stats | Completes without hang | action=refresh then stats |
| **TREE-05** | Miss suggestions | Wrong read_file path may suggest nearest when cache warm | Intentional miss |
| **TREE-06** | explore/plan tool presence | Both tools available | Spawn explore |
| **TREE-07** | No full tree dump in prompt | System context not multi-MB tree | Inspect inject (expect **no** card today) |
| **TREE-08** | Untrusted root | Kickoff/tools fail soft | Outside trusted if possible |

### Wave F — ADL + Feature Request Log (P1)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **ADL-01** | Tools on default toolset | Session has both tools | This session (live) |
| **ADL-02** | File developer_log | Appears in `turbo issues list/show` | Call tool |
| **ADL-03** | File feature_request_log | Appears in `turbo features list` | Call tool |
| **ADL-04** | Dedup fingerprint | Second call increments/returns same id | Duplicate payload |
| **ADL-05** | explore can file both | RO child can call ADL/FRL | Spawn explore |
| **ADL-06** | Disable env | Clean refuse + card gates | GROK_*=0 (careful) |
| **ADL-07** | CLI file path | `turbo issues file` / `turbo features file` | Human path |
| **ADL-08** | Export honesty | Export counts only loaded bodies | export packs |
| **ADL-09** | Configured roots | Match boot card paths | issues path / features path |

### Wave G — MCP (P2)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **MCP-01** | Default 120s timeout | Hung tool aborts ~2 min not ~100 min | Optional long hang |
| **MCP-02** | list_changed refresh | Catalog updates without full reconnect | Server that emits notification |
| **MCP-03** | Bad tool name remap | Registered, not silent drop | Weird MCP names |
| **MCP-04** | Windows OAuth ACL | mcp_credentials owner-only if present | icacls check |
| **MCP-05** | Connected servers health | mcp_server_health / tools usable | Live: blender, chrome-devtools, … |

### Wave H — Game Mode (P2 manual)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **GM-01** | Ctrl+G toggles office | Opens/closes; composer still works | Interactive TUI |
| **GM-02** | Shift+G is GotoBottom | Not Game Mode | Interactive |
| **GM-03** | Ctrl+Shift+G tasks pane | Not Game Mode | Interactive |
| **GM-04** | Hover popups | Desk/supervisor/MCP/board | Mouse |
| **GM-05** | Subagent desks 1–6 | Badges for running/done/fail | Spawn children |
| **GM-06** | Compact fallback | Card/Unicode when small | Resize |
| **GM-07** | Playground bin | Renders without session | cargo run game-mode-playground |
| **GM-08** | Unit tests | game_mode tests green | cargo test -p xai-grok-pager --lib views::game_mode |

### Wave I — deepaudit / workflows (P2)

| ID | Test | Pass criteria | How |
|----|------|---------------|-----|
| **DA-01** | `/deepaudit --size small` focused | Completes report | Interactive or headless |
| **DA-02** | Model slug normalize | `openai/gpt-5.6-*` → codex prefix in children | --models flag |
| **DA-03** | Headless wait | Does not EndTurn at launch only | turbo -p deepaudit |
| **DA-04** | Aliases | ultracode / deep-audit | Slash |
| **DA-05** | Project workflow load | `.grok/workflows/*.rhai` with trust | review-current-branch.rhai |

### Wave J — Regression (must not rebreak RC10/11)

| ID | Test | Pass criteria |
|----|------|---------------|
| **REG-01** | Session starts with FRL registered | No “feature_request_log not found in registry” |
| **REG-02** | developer_log on toolsets | Boot card policy enforceable |
| **REG-03** | deepaudit Rhai trimmed() / models | No crash empty models |
| **REG-04** | Boot card on resume | Present unless GROK_BOOT_CARD_ON_RESUME=0 |
| **REG-05** | --require-changes records writes | Headless NoChanges false-fail fixed |
| **REG-06** | Worktree alternates / .git objects | Isolated worktree has parent objects |

### Wave K — Docs / honesty (P2)

| ID | Test / fix | Pass criteria |
|----|------------|---------------|
| **DOC-01** | User-guide Ctrl+G | Documents Game Mode, not tasks |
| **DOC-02** | Soft-preserve default | 16-subagents matches product (preserve, not remove) |
| **DOC-03** | design-game-mode Shift+G | Updated to Ctrl+G |
| **DOC-04** | Deep-audit doc status | Header matches RC12 shipped jail |
| **DOC-05** | KNOWN_ISSUES RC12 review date | Updated with tombstone / residual shell |

### Wave L — Design backlog (FR, not ship blockers)

| ID | Item | Class |
|----|------|-------|
| **FR-01** | Session inject Workspace Tree card | feature_request |
| **FR-02** | `/tree` + `turbo tree` | feature_request |
| **FR-03** | Real freshness + incremental invalidate | feature_request |
| **FR-04** | Worktree base+overlay tree indexes | feature_request |
| **FR-05** | parent_writes_detected / doctor isolation check | feature_request |
| **FR-06** | Safer supervisor prune helper (never live) | feature_request or bug if prune is product API |
| **FR-07** | Write fail-closed when CWD tombstoned | **bug** (ADL P0) — Phase 3 |
| **FR-08** | CLI `subagent discard` discoverability / worktree aliases | docs or CLI polish |

---

## 3. Phase 2 execution order

1. **Live self-probes (this session):** ADL-01/02/03/09, TREE-01–05, MCP-05 health  
2. **Isolated write e2e:** ISO-01, ISO-02, ISO-09, LAND-01, WT-01, WT-03  
3. **Tombstone repro:** WT-06, WT-07, WT-08 (against open ADL)  
4. **Resume / allowlist / only_missing:** ISO-04, LAND-02–05  
5. **Coordinator storm (careful disk):** COORD-01–03, WT-02  
6. **Unit tests (package-scoped):** shell/tools isolation + game_mode  
7. **Manual TUI:** Wave H (human or playground)  
8. **deepaudit small:** DA-01/02 if budget allows  
9. **File FRs** for Wave L accepted gaps; **fix** Wave C tombstone bugs in Phase 3  

### Disk hygiene

Before densify-scale or `cargo test --workspace`: check free space on `H:` (≥40 GB package tests; ≥60 GB workspace). Prefer:

```powershell
cargo test -p xai-grok-tools --lib -- --test-threads=4
cargo test -p xai-grok-shell --lib -- --test-threads=4
cargo test -p xai-grok-pager --lib views::game_mode -- --test-threads=4
```

---

## 4. Phase 3 target list (from current signal)

| Priority | Item | Source |
|----------|------|--------|
| **P0 fix** | Write tool must not report success when worktree CWD is gone / path not verifiable | ADL P0 |
| **P0 fix** | Refuse or re-materialize when live worktree tombstoned mid-run | ADL P0/P1 |
| **P1 fix** | Product prune / keep-N: never delete RUNNING + live marker; document no bulk Remove-Item | ADL P2 + WT-02 |
| **P1 fix** | Shell error clarity for os error 267 on dead worktree CWD | ADL P1 |
| **P2 docs** | Ctrl+G, soft-preserve default, deep-audit status | Wave K |
| **FR** | Workspace Tree inject / slash / freshness | Wave L |

---

## 5. Live environment snapshot (Phase 1)

```
turbo 0.2.114-r12 (7b9464885)
ADL root:  H:\Apps\grok build\developer_log  (config developer-log.toml)
FRL root:  H:\Apps\grok build\feature request  (config feature-request-log.toml)
Open incidents: 3 (P0 write tombstone, P1 CWD 267, P2 prune race)
Open FRs: 0
```

---

## 6. Audit evidence map

| Audit agent | Focus | Key paths |
|-------------|-------|-----------|
| Isolation/worktree | 19 RC12 claims + residual | `subagent/handle_request.rs`, `subagent_worktree/*`, `confined_fs.rs`, `resources.rs`, `coordinator.rs` |
| Tree + ADL + MCP | Partial tree; shipped logs/MCP | `xai-workspace-tree/`, `workspace_tree/`, `developer_log/`, `xai-grok-mcp` |
| Game Mode + deepaudit | UI + workflow polish | `views/game_mode/*`, `deep_audit.rhai`, `boot_card.rs` |

---

## 7. Status

| Phase | Status |
|-------|--------|
| **1 — Audit + test list** | **Done** (this doc) |
| **2 — Extensive Q&A** | Next |
| **3 — Fix incidents + implement FRs** | After Phase 2 findings |

_Generated for RC12 harness Q&A. Update this file with PASS/FAIL as Phase 2 proceeds._
