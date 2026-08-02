# Turbo Harness Fix Plan (from RC9 Q&A)

**Sources:**
- `docs/Q&A/rc9/HYPER_HARNESS_QA_REPORT.md`
- `docs/Q&A/rc9/DEVELOPER_LOG_FEEDBACK.md`
- `docs/Q&A/rc9/session_export.md` (boot card / session probes)

**Audits:** five Grok 4.5 explore subagents (capability_mode, land/diff, ADL, require-changes/allowed_paths, boot-card polish), 2026-08-02.

**Baseline product version in QA:** `0.2.114-r9`  
**Source tree note:** RC10 already landed deepaudit/developer_log toolset/headless wait/resume boot card — this plan is the **remaining harness P0–P2** from the live Q&A, not the earlier deepaudit Rhai list.

---

## 0. What the Q&A proved works (do not regress)

- Parallel `spawn_subagent` (Grok 4.5 + Nemotron nano/super/9b/omni)
- Worktree isolation + soft-preserve + `retain_worktree`
- Baseline/snapshot refs + agent-only **manual** `git diff baseline..snapshot`
- explore role (no write tools)
- kill/cancel, resume_from, worktree list, sessions export
- Invalid model slug fail-closed
- Live workflow validate + simple live run
- CLI `issues list/show/export` once schema-valid data exists

---

## 1. Priority stack (implementation order)

| Wave | Pri | Item | Est. complexity | Depends on |
|------|-----|------|-----------------|------------|
| **A** | P0 | Enforce `capability_mode=read-only` (block write re-injection) | S | — |
| **A** | P1 | Agent-only **land/diff** (`baseline_ref..snapshot_ref`) | L | — |
| **A** | P1 | CLI `subagent diff` for clone-style trees (no abs pathspec) | S | land/diff helper |
| **B** | P1 | `--require-changes` records write/search_replace paths | S | — |
| **B** | P1 | ADL operator path: set-dir validation, `issues file`, export honesty | M | RC10 toolset already in source |
| **C** | P2 | Boot card Model field filled | S | — |
| **C** | P2 | `allowed_paths` write-time or hard land filter + boot-card sentence | M | product choice |
| **C** | P2 | Doctor / catalog Ultra `agent_ready` reconcile | M | policy |
| **D** | P2–P3 | Workflow validate hints, monitor Windows examples, `&&` note, prune UX, scheduler baseline | S–M | — |

Ship **Wave A** as the next release slice (harness safety + multi-agent land).  
Ship **Wave B** with acceptance harness from §8.  
Wave C/D can land in the same RC or follow-up.

---

## 2. Wave A — P0 capability_mode (audit: Grok 4.5)

### Root cause
1. Spawn filters `tool_config` via `apply_child_tool_policy`.
2. **Does not stamp** `definition.capability_mode = effective_runtime.capability_mode`.
3. `AgentBuilder` re-injects `OpenCodeWriteTool` when `write_file_enabled && inject_default_tools`.
4. Final clamp only runs if `definition.capability_mode` is `Some` → skipped for general-purpose + runtime-only RO.
5. Dispatch never checks mode/`is_read_only()`.

explore works because curated RO toolset + `inject_default_tools: false` + definition `capability_mode: ReadOnly`.

### Fix steps
1. **Stamp mode** after intersect in `handle_request.rs` (~after capability intersect / `apply_child_tool_policy`):
   ```rust
   definition.capability_mode = effective_runtime.capability_mode;
   ```
2. **Re-filter after memory inject** (same file ~981–990) if edit/write re-added.
3. Optional: skip write inject in `builder.rs` when mode is ReadOnly/Execute.
4. Defense in depth: tool dispatch fail-closed on disallowed kinds under restrictive mode.

### Acceptance tests
- Spawn GP + `capability_mode=read-only` + `isolation=none` → tool list has no `write` / `search_replace` / shell mutate.
- Forced write tool call denied if still registered.
- explore still RO; `capability_mode=read-write` / `execute` still work (QA matrix).

### Primary files
- `crates/codegen/xai-grok-shell/src/agent/subagent/handle_request.rs`
- `crates/codegen/xai-grok-agent/src/builder.rs`
- `crates/codegen/xai-grok-subagent-resolution/src/definition.rs`
- `crates/codegen/xai-grok-tools/src/implementations/grok_build/task/types.rs` (filter table / stale test)

---

## 3. Wave A — P1 agent-only land/diff (audit: Grok 4.5)

### Root cause
Spawn **creates** `baseline_ref` and export **writes** agent-only `changes.patch`, but:

| Surface | Actual behavior |
|---------|-----------------|
| `land_subagent` / CLI land | Live tree or `HEAD..snapshot` full dirty set |
| `diff_subagent` / CLI diff | Live: `git diff HEAD -- <abs wt path>` (fails outside repo); snap: `HEAD..snap` |
| soft-preserve | Prefer live → worst (full dirty) path |

Meta comment claims land uses `baseline..snapshot`; code does not.

### Fix steps
1. Shared helper `resolve_agent_delta(meta, parent)`:
   1. `baseline_ref..snapshot_ref` if both resolve  
   2. else patch / `changed_paths`  
   3. else live `git -C wt` vs baseline/HEAD  
   4. legacy `HEAD..snap` only with warn  
2. **Land default** = agent-delta paths only; merge 3-way base = baseline blob; overwrite only those paths.  
3. Prefer snapshot/patch over live when baseline present.  
4. **CLI `cmd_diff`:** never abs pathspec from parent; use refs or `git -C wt`.  
5. CLI `--force` parity with tool.  
6. Scheduler: fail loud if baseline missing; optional clean seed / exclude `.grok-restore/**`, `worktrees/**`.

### Acceptance tests
- Dirty parent + one agent file → land merge applies only probe; unrelated dirty untouched.  
- Clone-style worktree → `turbo subagent diff <id>` succeeds; matches `git diff baseline snapshot`.  
- Overwrite does not dump `worktrees/**` into parent.  
- No-op agent with baseline → empty/small diffstat.  
- Soft-preserve live tree still lands agent-only set.

### Primary files
- `crates/codegen/xai-grok-tools/src/implementations/grok_build/subagent_worktree/{mod,land,diff}.rs`
- `crates/codegen/xai-grok-pager/src/subagent_cmd.rs`
- `crates/codegen/xai-grok-workspace/src/worktree/mod.rs` (optional)
- Boot card land FOOTGUN wording (`boot_card.rs`)

---

## 4. Wave B — `--require-changes` (audit: Grok 4.5)

### Root cause
Headless only records edits when a **single** ACP event has `kind=Edit` **and** `status=Completed` **and** locations. Shell emits:

- Start update: Edit + locations, **no status**
- Completion: status Completed, **no kind/locations**

So `write` and `search_replace` never populate `filesChanged` → false `NoChanges` / exit 1.

### Fix steps
1. In `headless.rs` `note_edit_locations`: on ToolCallUpdate with Edit + locations, record **without** requiring Completed on same message.  
2. Optional: completion Diff content paths.  
3. Mirror in chat-state `record_agent_edited_path` for start updates.

### Acceptance tests
- Headless `--require-changes --always-approve` + write create → exit 0, `filesChanged.count >= 1`.  
- Same for search_replace.  
- No tools → still NoChanges.

### Primary files
- `crates/codegen/xai-grok-pager/src/headless.rs`
- Optionally `tool_calls.rs` / `acp_conversion.rs` / `updates.rs`

---

## 5. Wave B — ADL operator path (audit: Grok 4.5)

### Already in source (RC10) — verify on binary
- `DeveloperLogTool` on default / explore / plan / orchestrator / concise / hashline toolsets  
- Resume boot card inject; headless deepaudit wait  

**Still open from Q&A companion report:**

| Gap | Fix |
|-----|-----|
| set-dir → source tree | `set_configured_dir`: warn/reject git app trees; or force `<dir>/developer-log`; `ensure_layout` on set |
| No human file path | `turbo issues file` → `DeveloperLogStore::report` |
| Export summary vs bodies | Severity/table from **loaded** only; print skipped count; optional `--strict` non-zero |
| Boot card requires tool when missing | Gate REQUIRED block on tool presence **or** ADL enabled + tool registered |
| Doctor silent | Probe: ADL path looks like repo; tool on default toolset; unreadable bodies |
| Secondary toolsets | codex / opencode / computer still omit developer_log |

### Acceptance tests
- Interactive + headless + subagent: file incident → `issues list`  
- Bad set-dir → warn/reject  
- Corrupt body export → honest counts  
- `turbo issues file --title … --class feature_gap` works  

### Primary files
- `xai-grok-developer-log/src/{store,export}.rs`
- `xai-grok-pager/src/{issues_cmd,doctor_cmd}.rs`
- `xai-grok-agent/src/prompt/boot_card.rs`
- `xai-grok-agent/src/config.rs` (remaining toolsets)

---

## 6. Wave C — Boot card model + allowed_paths + agent_ready

### Boot card Model blank
- `placeholders()` never emits `"model"`; boot card reads empty.  
- **Fix:** `PromptContext.model` + plumb `models_manager.current_model_id()` in `agent_rebuild.rs` / `builder.rs`.

### allowed_paths
- Enforced only at land/diff refuse/filter; child can write outside.  
- **Product choice:** (B) write-time deny via permission/hook **or** (A) document “land/diff only” in boot card one sentence + filter `changed_paths` hard on land.

### Ultra agent_ready
- Catalog + NVIDIA platform override + doctor name heuristic all force not ready.  
- **Fix:** stop blanket `agent_ready=false` for proven Nemotron agent models; doctor trust catalog; update design-oracle docs.

---

## 7. Wave D — Polish backlog

| Item | Fix sketch |
|------|------------|
| Workflow validate errors | `with_rhai_hint`: cite `agent("prompt", #{…})`; kebab-case meta.name |
| Monitor Windows | Description: prefer `.ps1`; surface stderr on empty fail |
| Shell `&&` note | Shell-aware wording (PS 5.1 vs pwsh/cmd) |
| Land FOOTGUN boot card | Modes `merge\|overwrite`; tool `force=true` (no silent `--force` on CLI until added) |
| Prune UX | Filter list by session / pending land; warn >N preserved trees |
| Scheduler dirty inflation | Baseline required; clean seed option |
| Headless spawn thrash | Document spawn schema examples; optional better tool errors |

---

## 8. Suggested PR / worktree split

```
PR1  capability_mode stamp + tests                          [P0]
PR2  resolve_agent_delta + land/diff CLI/tools + tests      [P1]
PR3  require-changes ACP harvest + tests                    [P1]
PR4  issues file + set-dir validation + export honesty      [P1]
PR5  boot card model + allowed_paths policy + doctor Ultra  [P2]
PR6  workflow hints + monitor + shell note                  [P2]
```

Each PR: unit tests from §acceptance + re-run credit-light harness probes from QA report matrix (dirty parent land, RO write denial, require-changes write, issues file, subagent diff outside repo).

---

## 9. Harness retest matrix (post-fix)

| Probe | Pass criteria |
|-------|----------------|
| GP + capability_mode=read-only + isolation=none | No write file on parent |
| Dirty parent + worktree probe + land merge | Only agent paths land |
| `turbo subagent diff <clone-style-id>` | No outside-repo error; agent-only |
| Headless write + `--require-changes` | exit 0, filesChanged ≥ 1 |
| Parent/child/headless `developer_log` | Incident appears in `issues list` |
| set-dir to repo root | Warn/reject or nested log dir |
| Boot card | Model: non-empty |
| Nemotron tiny tool (optional credit) | Still completes |

---

## 10. Explicit non-goals for this plan

- Free-tier NVIDIA timeouts  
- Full MCP depth / hooks / plan-mode UX / images  
- Re-auditing deepaudit Rhai (already RC10) unless regression  

---

## 11. Subagent audit IDs (for resume)

| Audit | Focus | Subagent id |
|-------|--------|-------------|
| capability_mode RO | P0 write re-injection | `019fc2b8-755f-7863-a4bf-a6362bfaa36d` |
| land/diff agent-only | P1 land/diff/scheduler | `019fc2b8-7560-7c81-93e2-78f4685a26f9` |
| ADL set-dir/export | P1 operator path | `019fc2b8-7561-7a81-a4b0-afaefe8480b1` |
| require-changes / allowed_paths | P1/P2 | `019fc2b8-7562-7a53-ad3a-0068ce3e1058` |
| boot card / polish | P2 | `019fc2b8-7564-70b1-920f-aec2decee1e8` |

Resume any audit with `spawn_subagent(resume_from=<id>)` if implementation needs deeper context.

---

## 12. Bottom line

The Q&A shows the harness **already steers Grok 4.5 and Nemotron** well for small isolated tasks. Remaining work is almost entirely **parent-side control plane**:

1. **Make capability_mode real** (stamp + no write re-inject).  
2. **Make land/diff agent-only** (use the baselines you already create).  
3. **Make headless require-changes honest** for write tools.  
4. **Finish ADL operator loop** (set-dir safety, human file, export honesty).  
5. Polish boot card model, allowlist semantics, doctor Ultra.

**Recommended next action:** implement **PR1 + PR2** first (safety + multi-agent land), then PR3–PR4 for harness CI reliability.
