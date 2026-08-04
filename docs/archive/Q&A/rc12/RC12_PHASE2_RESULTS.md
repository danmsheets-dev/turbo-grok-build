# RC12 Phase 2 Q&A Results (in progress)

| Item | Content |
|------|---------|
| Wire | 0.2.114-r12 |
| Date | 2026-08-03 |
| Binary | turbo 0.2.114-r12 (7b9464885) |
| Plan | [RC12_HARNESS_AUDIT_AND_QA_PLAN.md](./RC12_HARNESS_AUDIT_AND_QA_PLAN.md) |

---

## Live results so far

| ID | Result | Evidence |
|----|--------|----------|
| **ISO-01** | **PASS** | Worktree child wrote abs parent path → remapped into `~/.grok/worktrees/.../subagent-019fc8c0-...`; parent `Test-Path` False |
| **ISO-09** | **PASS** | Real git toplevel under worktrees; `.grok-subagent-live` present (`pid=…`) |
| **LAND-01** | pending | Not yet exercised end-to-end land of probe file |
| **TREE-01–04** | **FAIL** (pre-fix) | `workspace_tree` / `resolve_path` → `requires session Cwd` on subagents |
| **TREE-05** | **PASS** | Typo `boot_car.rs` → similar `boot_card.rs` |
| **ADL-02** | **PASS** | Smoke incident filed; `turbo issues list` shows it |
| **FRL-03** | **PASS** | FR filed for tree inject card; `turbo features list` shows 2 FRs |
| **ADL-09** | **PASS** | Roots match config: `H:\Apps\grok build\developer_log`, `H:\Apps\grok build\feature request` |
| **COORD-01** (unit) | **PASS** | `spawn_queues_when_at_concurrency_limit` ok |
| **TREE unit crate** | **PASS** | `xai-workspace-tree` 5 lib tests ok |
| **MCP-05** | **PARTIAL** | blender, chrome-devtools, docs-mcp, gitnexus, godot-docs, react-docs, tasks ready; guardian/resend/sentry/sourcegraph failed (config/auth) |

---

## Phase 3 fix started: TREE missing Cwd on subagents

**Incidents:**
- `inc_019fc8c12e8a79138dfff75a074f013f` (P1) workspace_tree/resolve_path requires session Cwd on subagent
- `inc_019fc8c29cde7231acec448b0089bca6` (P2) resolve_path same in worktree child

**Root cause:** New tools used only `ctx.get::<xai_tool_runtime::Cwd>()` (per-call override). Subagent dispatch rarely sets the override; session Cwd lives in `SharedResources` as `resources::Cwd`. Other tools use `resolve_cwd` + `shared_resources`.

**Fix (source):**
- `workspace_tree/mod.rs` and `resolve_path/mod.rs` → `resolve_cwd(&ctx, &resources)`
- Regression tests: `*_uses_resources_cwd_without_extension_override`

**Note:** Fix is in **source tree**; live turbo binary is still pre-fix until rebuild/reinstall.

---

## Open product backlog (from ADL/FRL)

### Bugs (fix)
| Sev | ID | Title |
|-----|-----|-------|
| P0 | inc_019fc8a670fa77… | Write success but file missing; CWD tombstoned |
| P1 | inc_019fc8c12e8a79… | workspace_tree Cwd (fixing in tree) |
| P1 | inc_019fc8a2d77b7b… | Shell os error 267 on dead worktree |
| P2 | inc_019fc8bf3e2b7f… | discard leaves status=running |
| P2 | inc_019fc8b26b3976… | Supervisor prune raced live worktrees |
| P2 | inc_019fc8bf3e1f71… | Boot card omits repo layout (FR adjacent) |

### Feature requests
| ID | Title |
|----|-------|
| fr_019fc8c0c9e67c… | Workspace Tree session inject card not wired |
| fr_019fc8c177ee7e… | Session boot injects compact repo layout + worktree inventory |

### Smoke (safe to resolve)
| ID | Title |
|----|-------|
| inc_019fc8c1e01a7… | ISO-01 harness probe |
| inc_019fc8c0c9dc7… | ADL smoke |

---

## Still to run (priority)

1. Rebuild turbo with Cwd fix → retest TREE-01–04 on subagent  
2. ISO-02 shell abs write deny  
3. ISO-04 resume fail-closed  
4. LAND-01/05 only_missing + land of ISO-01 probe  
5. WT-06/07/08 tombstone repro (P0 densify)  
6. GM unit tests + optional playground  
7. Docs wave (Ctrl+G, soft-preserve)

---

## Audit agents used

| Role | Id (short) |
|------|------------|
| Isolation explore | 019fc8ba-ea1a… |
| Tree/ADL/MCP explore | 019fc8ba-ea1a… (16ac) |
| Game Mode explore | 019fc8ba-ea25… |
| ISO-01 worktree GP | 019fc8c0-72d1… |
| TREE+ADL smoke GP | 019fc8c0-72d1… (6833) |
