# Deep Audit: Subagent Isolation & Worktrees

| Item | Content |
|------|---------|
| Date | 2026-08-03 |
| Scope | `isolation=worktree`, soft-preserve, land/diff/baseline, boot card, absolute path escape |
| Trigger | Tools written only on parent despite worktree spawn; land never applied |
| Status | **Second deep-audit 2026-08-03 · residual shell escape remains; tool-path jail largely shipped** |
| Related | ADL `inc_019fc76003af7bb2b22577f14a73c113`, FR `fr_019fc76003af7bb2b225780e38eca280` |

---

## 1. Incident (reproducible class)

1. Supervisor spawns write-capable child with **`isolation=worktree`**.
2. Harness creates live worktree under `~/.grok/worktrees/<slug>/subagent-<id>`.
3. Child “completes”; `worktree_state=preserved`, `land_status=pending`.
4. **New files exist only under the parent repo**, not under the soft-preserved worktree.
5. Land never ran (or would be empty / conflict-ridden).

This is **not** primarily “forgot isolation.” Isolation was requested and a worktree existed. The failure mode is **filesystem escape + incomplete agent guidance + land/baseline footguns**.

---

## 2. Architecture (actual vs intended)

```text
INTENDED
  parent ──spawn isolation=worktree──► worktree CWD
       all writes ──► worktree only
       complete ──► snapshot + soft-preserve
       supervisor ──► diff (baseline..snapshot) ──► land ──► parent

ACTUAL
  parent ──spawn──► worktree CWD ✓  (git tree + default cwd)
       relative writes ──► worktree ✓
       absolute parent paths ──► PARENT DISK ✗  (no jail / no DisplayCwd remap)
       soft-preserve worktree (may lack agent files)
       land_status=pending (no auto-land)
```

**Isolation today = separate git worktree + child CWD, not an FS jail.**

---

## 3. Verified findings (bugs / product defects)

### P0 — Absolute paths bypass worktree isolation (confirmed)

| | |
|--|--|
| **What** | `resolve_model_path` returns absolute paths as-is when `DisplayCwd` is unset. Subagent spawn always passes `prompt_display_cwd: None`. Write/edit tools accept absolute paths (schema encourages them). |
| **Evidence** | `xai-grok-tools` `types/resources.rs` path resolve; `handle_request.rs` subagent spawn `None` for display cwd; `LocalFs::write_file` writes given abs path; boot card: `allowed_paths` land-time only. |
| **Effect** | Model given `H:\Apps\...\file` writes parent even when CWD is worktree. Worktree soft-preserved “empty”; parent dirty; land still pending. |
| **Incident fit** | **Primary root cause class.** Supervisor prompts often include parent absolute workspace path (as this session did for tools agent). |

### P0 — Child boot card lies about isolation (confirmed)

| | |
|--|--|
| **What** | Child injection hardcodes `ctx.isolation = "worktree"` regardless of spawn. |
| **Evidence** | `xai-grok-agent` `prompt/context.rs` ~331–332; `render_child` text. |
| **Effect** | Child told isolation=worktree even for `isolation=none` or shared fallback → false confidence, no verify-cwd behavior. |

### P1 — No write-time confine for isolated children (confirmed)

| | |
|--|--|
| **What** | Process `--confine` / `GROK_CONFINE` exists for nested turbo; **not** installed for isolation=worktree children. `allowed_paths` is land/diff only. |
| **Evidence** | `pager/confine.rs`; workspace `fs_confinement_active` default off; boot card line on allowed_paths. |
| **Effect** | Escape is always open via abs paths / shell redirections. Grok Build Plugin’s bridge has a stricter isolation prompt (`isolation.md`); Turbo shell does not mirror that jail. |

### P1 — Soft-preserve + land=pending misread as “isolation worked” (confirmed product UX)

| | |
|--|--|
| **What** | Soft-preserve default keeps live tree; dispose sets `land_status=pending`. Neither means parent is clean or child wrote only to worktree. |
| **Evidence** | `handle_request` dispose; `SubagentCompletedOutput` tags; boot card soft-preserve text. |
| **Effect** | Supervisors assume worktree holds the work; reality may be parent-only writes. |

### P1 — Dirty parent seed + baseline soft-fail inflate land (confirmed residual)

| | |
|--|--|
| **What** | Default seed `PreserveWorkingTree` copies dirty/untracked parent into worktree. Baseline capture can soft-fail (warn only). Land without baseline attributes dirt to agent. |
| **Evidence** | `xai-fast-worktree` WorkingTreeMode; spawn baseline warn path; CLI land refuse >50 without baseline. |
| **Effect** | `turbo subagent land` conflicts (e.g. Cargo.lock); supervisors resort to `Copy-Item` (this session). |

### P2 — Docs / soft-preserve contradiction (confirmed)

| | |
|--|--|
| **What** | Boot card: soft-preserve default. User guide `16-subagents.md`: worktree “removed by default.” |
| **Effect** | Wrong recovery habits (panic copy vs open/land). |

### P2 — CLI vs tool land policy skew (confirmed)

| | |
|--|--|
| **What** | Tool land has size guard + force; CLI land lacks force; allowlist refuse vs filter differs. |
| **Effect** | “Land via CLI” ≠ `land_subagent`. |

### P0 — Resume of non-isolated source always fail-opens (confirmed, deep-audit C1)

| | |
|--|--|
| **What** | When `isolation=worktree` is requested but `resume_from` source has `worktree_path=None`, the child **always** continues shared on the parent. Unlike create/rehydrate/disk-guard failures, this path does **not** consult `GROK_SUBAGENT_ALLOW_SHARED_FALLBACK` and never `failure_result`s. |
| **Evidence** | `handle_request.rs` ~253–267: sets `isolation_fallback=true`, match `None => None` with no `allow_shared_fallback` gate. Contrast rehydrate Err / Shared / create Err (~291–502) which fail closed unless env opt-in. |
| **Impact** | `resume_from` on a pre-isolation / isolation=none / prior-fallback run silently reuses parent cwd under an isolation=worktree request. Edits land on live checkout; disposable worktree land cannot recover them. |
| **Fix** | Fail closed unless `ALLOW_SHARED_FALLBACK=1`, **or** force a **fresh** worktree when isolation=worktree resumes a non-worktree source. |

### Not primary for this incident

| Hypothesis | Why lower |
|------------|-----------|
| Silent isolation_fallback without env on **create** | Fail-closed unless `GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1`; create path is OK. Resume path (C1) is the hole. |
| Auto-land applied parent | No auto-land |
| COW shared inodes | Unlikely; abs path write is sufficient |

### Workflow deep-audit

Run `wf_019fc7639b8d7a63b1523eadd796ca3b` (`focus=subagents`, size large) independently confirmed **C1** (resume fail-open) and **C6** (absolute path escape via `resolve_model_path` + `Path::join`). Status: **Partial** (other candidates in unverified appendix of workflow output).

---

## 4. Contributing factors (this session)

| Factor | Role |
|--------|------|
| Supervisor prompt used **absolute parent path** for tools agent | Directly encourages escape (H1) |
| Supervisor edited **Game Mode on parent** while child ran | Parent pollution / race |
| Land failed on dirty parent → manual **Copy-Item** | Bypasses baseline..snapshot |
| No completion checklist for isolation tags | Missed verify step |

---

## 5. Fix plan (phased)

### Phase A — Fail closed on FS escape (P0 product)

1. **Install confine root = worktree** for every successful `isolation=worktree` spawn  
   - Set process/session confine to worktree path (or resource `ConfineRoot`).  
   - Reject (or remap) tool writes outside worktree.

2. **Fork-style DisplayCwd for isolated children**  
   - `prompt_display_cwd = parent` (optional product choice) **or** keep real worktree path in prompt but:  
   - **Always** remap absolute paths under parent → corresponding path under worktree (Plugin `isolation.md` contract).  
   - Absolute paths outside parent+worktree: reject.

3. **Write-time `allowed_paths`** when allowlist non-empty (fail closed).

4. **Tests**  
   - Isolated child write with parent abs path → remapped into worktree **or** denied.  
   - Relative write → only worktree.  
   - Soft-preserve + empty agent-only delta + parent file created → detector / isolation_path_escape metric.

### Phase B — Honesty signals (P0/P1)

5. **Child boot card uses effective isolation** (not hardcoded `"worktree"`).  
6. **Child: verify CWD before first write** (boot card + subagent system prompt).  
7. **Parent boot card:**  
   - Parse isolation tags before land.  
   - Freeze overlapping parent paths while worktree children run.  
   - **Forbid** shell copy; land_subagent only.  
   - Explicit FOOTGUN for abs-path escape + isolation_fallback.  
8. **Completion payload:** always include `isolation_requested`, `isolation_effective`, `isolation_fallback`, `worktree_path`, optional `parent_writes_detected` if detectable.  
9. **Align user guide** soft-preserve + land-first wording with product.

### Phase C — Land / baseline hardening (P1)

10. **Fail closed on land** when isolation=worktree and baseline missing.  
11. Default seed for isolation worktrees: **clean** (or default-on for land-critical); dirty seed opt-in.  
12. Unify CLI/tool land (size, force, allowlist refuse).  
13. Restore dest under `~/.grok/restores/...` not parent `.grok-restore/`.

### Phase D — Operator / harness ergonomics (P2)

14. `turbo subagent doctor <id>`: print isolation tags, baseline, agent-only file count, “parent abs write?” heuristic if logs available.  
15. Workflow deep-audit focus=`subagents` stays green for these gates.

---

## 6. Proposed boot card deltas (summary)

Full copy-paste text: explore agent deliverable (boot card audit). Minimum viable:

**Child:** verify CWD under worktrees; stop + `developer_log(isolation_fallback)` if parent; never land/copy to parent.

**Parent Subagents/Recovery/Don't:** request ≠ proof; read completion tags; no parent edits on active child paths; no Copy-Item; land only.

**Injection fix:** plumb real isolation into `BootCardContext` for children.

---

## 7. Implementation PR stack (suggested)

| PR | Title | Scope |
|----|--------|--------|
| **PR-1** | Confine + remap absolute paths for isolated subagents | shell spawn + tools resolve_model_path + tests |
| **PR-2** | Boot card + child prompt honesty + parent freeze/land rules | boot_card.rs, context.rs, subagent_prompt.md |
| **PR-3** | Land fail-closed without baseline; clean seed default | handle_request, land CLI/tool, seed env default |
| **PR-4** | Docs: 16-subagents soft-preserve, land_subagent-first, isolation_fallback checklist | user-guide + AUTO_DEVELOPER_LOG |
| **PR-5** | Isolation doctor + parent_writes_detected (best-effort) | CLI + completion meta |

---

## 8. Acceptance criteria (definition of done)

1. Isolated child **cannot** create a new file on parent via absolute parent path (deny or remap).  
2. Child card isolation string matches spawn/fallback.  
3. Parent card forbids shell promote; documents isolation tag checklist.  
4. Land without baseline refuses for worktree isolation.  
5. Soft-preserve + pending land documented consistently.  
6. Integration tests cover H1 escape + land baseline fail-closed.  
7. Session that previously polluted parent no longer does under PR-1.

---

## 9. What supervisors should do **until PR-1 ships**

1. In child prompts: **never** pass parent absolute roots; pass “use relative paths from CWD only.”  
2. After complete: `Test-Path worktree/.../file` vs parent before land.  
3. Prefer `diff_subagent` / `turbo subagent diff` (agent-only) over Copy-Item.  
4. If parent has child files but worktree does not → treat as **isolation leak**, file `developer_log(isolation_fallback or work_lost_risk)`, do not claim isolated success.  
5. Avoid parent edits on the same crates while write children run.

---

## 10. Deep-audit method notes

- Parallel explore agents: isolation spawn path, land/baseline, boot card.  
- Workflow `deep-audit` launched with `focus=subagents`, `size=large` (async).  
- Primary confirmed mechanism is **code-backed (H1)**; not dependent on dirty parent.

---

## 11. Bottom line

| Question | Answer |
|----------|--------|
| Forgot isolation flag? | **No** — worktree isolation was requested and worktrees were created. |
| Harness bug? | **Yes (P0):** isolation does not jail absolute FS writes; child card hardcodes worktree. |
| Operator error? | **Also yes:** absolute parent paths in child prompts; parent edits during children; manual Copy-Item after land fail. |
| Fix priority? | **Confine+remap (PR-1)** then **boot card honesty (PR-2)** then **land baseline (PR-3)**. |
