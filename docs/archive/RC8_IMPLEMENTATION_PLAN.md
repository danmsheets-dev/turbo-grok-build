# Hyper RC8 Implementation Plan

**Release target:** `0.2.114-r8` (RC8)  
**Date:** 2026-08-01  
**Inputs:**
- `docs/HYPER_DEVELOPER_FEEDBACK.md` (live NVIDIA + subagent audit)
- In-tree audit of r7 codebase (`dev` @ `345ad14` + feedback doc)
- Four Grok 4.5 research passes (loop reliability, worktree lifecycle, NVIDIA provider, HCIL architecture)
- Claude Code Ultracode / dynamic-workflow docs + live screenshot audit (`nemotron-ultra-subagent-audit`, 91 agents / 7.4M tokens)

**Positioning:** r7 shipped isolation-by-default (snapshot then delete). RC8 is the **reliability + supervisor ergonomics + deep-audit** release: make multi-agent development and multi-hour improvement loops safe, recoverable, and timeout-bounded ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â especially with NVIDIA Integrate ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â and ship a first-class **`/deepaudit`** workflow (HyperÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢s Ultracode-style adversarial repo audit).

---

## 1. Executive summary

| Theme | Status after r7 | RC8 goal |
|-------|-----------------|----------|
| Subagent isolation | Correct (parent unpolluted) | **Keep isolation**; fix recovery UX |
| Worktree lifecycle | Snapshot ref exists; tree deleted; path tombstone | Patch + diffstat + structured completion; retain/land path |
| Hard timeouts | Only agent-def (`oracle` = 180s); GP unbounded | `timeout_ms` on spawn + platform defaults |
| Stall handling | Stream idle only; progress heartbeats are UI-only | Progress-based stall fail + free slots |
| NVIDIA tools | Deser / request quirks break agent path | Null-tolerant deser + platform defaults + catalog hygiene |
| Land pipeline | Manual `git show` archaeology | `land` / `diff` / `open` tools + CLI |
| Multi-hour loops | Bones exist (queue=4, workflow, resume) | Stock improve workflow + checkpoint + reliability substrate |
| **Deep audit (`/deepaudit`)** | Only `/deep-research` + ad-hoc branch review workflow | Bundled **investigate ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ verify ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ report** audit; slash + size guidelines |

**Controls already prove the stack works:** Grok 4.5 and Codex Terra complete multi-tool coding in worktrees. NVIDIA failures are **provider + deser + policy**, not ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œsubagents broken.ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â

**Ultracode parity note:** ClaudeÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢s Ultracode is not ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œone smarter modelÃƒÂ¢Ã¢â€šÂ¬Ã‚Â ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â it is a **scripted fan-out** (investigate ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ hypothesis explosion ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ independent verify ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ filtered report). Hyper already has the runtime (`xai-workflow` Rhai, `/deep-research`, `/workflows` UI). RC8 productizes that pattern as **`/deepaudit`**.
---

## 2. Codebase audit (as-is)

### 2.1 What works (do not regress)

| Capability | Location | Notes |
|------------|----------|-------|
| Isolation default = worktree | `xai-grok-subagent-resolution`, r7 CHANGELOG | Fail-closed if create fails |
| Snapshot ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ `refs/grok/subagents/<id>` | `handle_request.rs` dispose ~1918ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“1984 | Transfer into parent main repo |
| Resume rehydrate from snapshot | `ResumeWorktreeAction` in `subagent/mod.rs` | Works if ref persisted |
| Slot pool max 4 + FIFO queue | Coordinator (`DEFAULT_MAX_CONCURRENT_SUBAGENTS`) | Env/config override |
| Progress publisher (2s / 8s) | Subagent progress ACP | No auto-cancel |
| Agent-def budgets | `AgentDefinition.timeout_secs` | **Only oracle** has defaults (180s / 40 tools) |
| Workflow Rhai | `xai-workflow` | `agent`/`parallel`/`pause`/`journal`; same-process resume |
| `/deep-research` builtin | `session/workflows/deep_research.rhai` + slash | Plan ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ research ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ verify ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ cited report (web-oriented) |
| Branch multi-review workflow | `.grok/workflows/review-current-branch.rhai` | Parallel reviewers + dual verification (code-oriented) |
| `/workflows` progress UI | `xai-grok-pager` views | Phase / agent roster / pause-stop |
| `prompt_cache_key` strip gate | `client.rs` + per-model TOML | Partially mitigated; not platform-wide |

### 2.2 Gaps vs feedback (evidence)

| Feedback P0 | Code evidence | Gap |
|-------------|---------------|-----|
| Deser `null, expected u32` | `Usage` / `ChatChunkChoice.index` / `ToolCallDelta.index` as bare `u32` in `xai-grok-sampling-types/src/types.rs` | NVIDIA nulls crash client after successful tokens |
| Worktree deleted; hard to find work | Dispose snapshots then **removes** tree; `SubagentCompletedOutput` has **no** `snapshot_ref` | Parent sees dead path or nothing |
| No hard timeout | `TaskToolInput` has no `timeout_ms`; GP `timeout_secs: None` | NVIDIA stalls 10ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“27+ min |
| No structured land/diff | No `land_subagent` tool; completion is text blob | Supervisors cannot merge multi-agent work |

| Feedback P1 | Evidence | Gap |
|-------------|----------|-----|
| Platform NVIDIA defaults | `supports_prompt_cache_key` default **true**; NVIDIA only forced false via user TOML / missing field quirks | Residual 400s when `request_compat` missing |
| Token overflow Nano 9B | Catalog `max_completion_tokens=131072` vs API `max_model_len=128000` | 400 on tools |
| Single tool-call Llama 70B | No `parallel_tool_calls` on request; no `max_parallel_tool_calls` | Multi-tool 400 |
| EOL catalog | step-3.5, mistral-*, kimi-k2.6 still listed | Bad picker UX |
| Oracle ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ Ultra | User `[subagents.models] oracle = ultra`; product has no pin | Tool oracle broken until deser fixed |
| Heartbeats / error_class | Progress exists; `error: Option<String>` opaque | Cannot smart-retry |

### 2.3 Ideal worktree machine vs actual

```text
ACTUAL (r7 default):
  SPAWN ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ RUNNING ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ SNAPSHOT+DELETE ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ meta keeps tombstone path + snapshot_ref
  Parent tool text: answer + <subagent_meta>  (NO snapshot_ref)

IDEAL (Ãƒâ€šÃ‚Â§3.5):
  SPAWN ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ RUNNING ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ SNAPSHOTTED ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ LAND | DISCARD | TTL ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ CLEANED
  Never DELETE_WITHOUT_SNAPSHOT; structured worktree block always
```

### 2.4 Multi-hour loop bones vs missing pieces

**Have:** coordinator slots, wait/kill, workflow phases, progress ticks, snapshot refs, resume_from (completed only).

**Missing for ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œresearch ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ plan ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ implement ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ check ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ optimize ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ repeat for hoursÃƒÂ¢Ã¢â€šÂ¬Ã‚Â:**
1. Hard child lifetime + stall eviction  
2. Always-recoverable artifacts (patch + structured ref)  
3. Land tools (parent-controlled merge)  
4. `error_class` + selective retry  
5. Durable loop checkpoint (workflow journal is same-process only)  
6. Optional `timeout_ms` / `retain_worktree` on spawn and `AgentOpts`

### 2.5 Deep-audit gap (Ultracode parity)

| Have | Missing for `/deepaudit` |
|------|--------------------------|
| Rhai workflows + `agent()` / `parallel()` | Bundled **codebase audit** script (not only web research) |
| `/deep-research` slash + builtin registry | **`/deepaudit` slash** + `deep-audit` builtin name |
| Dual-verify pattern in branch review | Generic **find ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ explode claims ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ verify each** for any scope |
| `agent_budget` (default 128, max 1024) | **Size guidelines** (small/medium/large) + cost caution in UI |
| `/workflows` dashboard | Docs for audit cost, size, when to use vs single agent |

Observed Claude Ultracode audit (reference): 91 agents, 7.4M tokens, Investigate `find:*` + Verify `verify:*` phases.

---

## 3. Release themes (RC8)

### Theme A ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â NVIDIA Integrate reliability
Unblock tool-using NVIDIA models; stop 400s from client mistakes; honest catalog.

### Theme B ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Worktree never loses work
MVP recovery + structured completion; path toward land/retain.

### Theme C ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Subagent reliability (timeouts + stalls)
Bounded agents; free slots; classify failures.

### Theme D ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Supervisor land path
Parent can diff/land/open without git archaeology.

### Theme E ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Continuous improvement loop substrate
Stock workflow + checkpoint + docs so multi-hour loops are productized, not hand-rolled.

### Theme F ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â `/deepaudit` (Ultracode-style deep audit)
Bundled adversarial codebase audit: investigate fan-out ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ independent verification ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ filtered report. Slash command, size guidelines, RO-by-default agents. Depends on Theme C timeouts so large runs do not zombie slots.

**Out of RC8 (defer to RC9+):** real `git worktree list` force-migration, full path allowlists, pre-land hooks as first-class product, fan-out `spawn_many` tool, full subagent panel UI, cross-process workflow journal, Claude-style **`ultracode` keyword auto-trigger** and `/effort ultracode` session mode, dynamic model-authored workflow JS (Hyper stays Rhai).
---

## 4. Work packages (implementation order)

### WP0 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Release scaffolding
| Item | Detail |
|------|--------|
| Version | Bump `VERSION` ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ `0.2.114-r8` when shipping (keep Unreleased until merge) |
| Docs | This plan + update `KNOWN_ISSUES.md` + CHANGELOG |
| Feedback | Keep `HYPER_DEVELOPER_FEEDBACK.md` as source audit; link from CHANGELOG |

---

### WP1 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P0 NVIDIA deserialization (Theme A)

**Problem:** Stream/response parse dies on `null` for fields typed `u32`.

**Fix:**
1. Null-tolerant deser for Chat Completions:
   - `Usage.{prompt,completion,total}_tokens`
   - `ChatChoice.index`, `ChatChunkChoice.index`
   - `ToolCallDelta.index` (default + null ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ 0)
   - Nested usage detail `u32`s
2. Prefer `deserialize_null_default` / `Option<u32>` ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ map to 0 in `TokenUsage` conversion.
3. Fixtures from NVIDIA-shaped payloads (usage nulls, index null, tool_calls null regression).
4. On residual serialize fail: classify `error_class=serialize`; keep raw snippet in logs (already partially present).

**Files:**
- `crates/codegen/xai-grok-sampling-types/src/types.rs`
- `crates/codegen/xai-grok-sampling-types/src/serde_helpers.rs` (if needed)
- `crates/codegen/xai-grok-sampler/tests/fixtures/nvidia_*.json` (new)
- Unit tests co-located with types/client

**Acceptance:**
- Fixture deser never panics/errors on documented null shapes
- Live smoke (if key available): Super/Nano/Ultra tool call stream completes without client serialize error

---

### WP2 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P0 worktree recovery MVP (Theme B)

**Problem:** Snapshot ref exists but parent/UI cannot see it; tree deleted; no patch.

**Dispose order (new):**
```text
snapshot ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ write changes.patch + diffstat ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ persist meta
  (snapshot_ref, snapshot_sha, worktree_state, patch_path)
ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ remove dir (if not retain) ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ clear live path in result
ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ structured worktree block on completion
```

**Changes:**
1. **Always** export `changes.patch` + diffstat under session `subagents/<id>/` before delete  
2. Meta: `worktree_state: live|cleaned|preserved`, `snapshot_sha`, `patch_path`, `diffstat`  
3. Clear or mark tombstone: never present deleted path as live  
4. Thread into `SubagentResult` / `SubagentCompletedOutput` / model text:
   - `<snapshot_ref>`, `<diffstat>`, `<patch_path>`, status  
5. Keep isolation property (no auto-land)

**Files:**
- `xai-grok-shell/src/agent/subagent/handle_request.rs` (dispose)
- `xai-grok-shell/src/agent/subagent/mod.rs` (`SubagentMeta`)
- `xai-tool-types/src/task.rs` (`SubagentCompletedOutput`)
- `xai-grok-tools/.../task/` (result mapping, task_output renderer)
- Helpers: `xai-fast-worktree` / `xai-grok-workspace` for patch export

**Acceptance:**
- After complete, `changes.patch` exists even if tree gone  
- Parent tool result includes `snapshot_ref` + non-zero diffstat when files changed  
- `meta.worktree_state=cleaned` when path removed  
- Isolation probe still passes (parent tree untouched)

---

### WP3 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P0 hard timeout + cancelÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢snapshot (Theme C)

**Problem:** GP subagents unbounded; kill mid-stall can leave empty recovery.

**API:**
```rust
// TaskToolInput
pub timeout_ms: Option<u64>,  // hard wall-clock; distinct from get_task_output wait
```

**Resolution order:**
1. Explicit `timeout_ms` on spawn  
2. `AgentDefinition.timeout_secs`  
3. Platform/model default (NVIDIA agent path: **600_000** ms)  
4. Config `[subagents] default_timeout_ms`  
5. Unbounded only if `allow_unbounded = true`

**Plumbing:**
- `SubagentExecutionBudget.timeout_secs` already drives budget monitor ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â map spawn override into it  
- Hard timeout ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ `status=timed_out`, `error_class=timeout`, **always** run WP2 dispose  
- Workflow `AgentOpts.timeout_ms` same field  

**Files:**
- `xai-tool-types/src/task.rs`
- `xai-grok-tools/.../task/types.rs`, coordinator
- `xai-grok-shell/src/agent/subagent/mod.rs` + `handle_request.rs`
- `xai-workflow/src/host.rs` + shell `host_service.rs`
- Config types for defaults

**Acceptance:**
- Spawn with `timeout_ms=5000` dies ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ ~6s with timed_out  
- Snapshot/patch written on timeout when dirty  
- NVIDIA default 10 min documented and applied for nvidia/* unless overridden  

---

### WP4 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P0/P1 stall detector (Theme C / loop)

**Problem:** Agent alive but no progress holds a slot for 27+ minutes.

**Policy:**
```text
progress = tool_call_countÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬Ëœ OR tokens_usedÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬Ëœ OR turn_countÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬Ëœ
         OR active shell task progress (optional)
if idle > stall_timeout_ms ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ fail error_class=stall, free slot, snapshot
```

**Defaults:**
| Profile | stall_timeout_ms |
|---------|------------------|
| NVIDIA / smoke | 180_000 (3 min) |
| Default agent | 600_000 (10 min) |
| Multi-hour loop (explicit) | 900_000ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“1_800_000 |

**Implementation:**
- Share sampling with existing progress publisher (2s)  
- Extend signals with `last_progress_at`  
- Do **not** treat pure SSE keepalives as agent progress  

**Files:**
- `subagent/mod.rs` (monitor next to budget)
- Session signals / snapshot Running fields
- TaskToolInput optional `stall_timeout_ms`

**Acceptance:**
- Synthetic stuck child (no tools/tokens) fails with stall within policy  
- Long `cargo test` under bash counts as progress if wired; else document exception  

---

### WP5 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P0 structured completion + land/diff/open (Themes B/D)

**Structured worktree block (completion):**
```json
{
  "status": "completed|failed|timed_out|cancelled",
  "error_class": null,
  "worktree": {
    "state": "live|cleaned|preserved",
    "path": null,
    "snapshot_ref": "refs/grok/subagents/...",
    "snapshot_sha": "...",
    "diffstat": { "files_changed": 2, "insertions": 40, "deletions": 12 },
    "patch_path": ".../changes.patch",
    "land_status": "pending|landed|discarded|conflict"
  }
}
```

**New tools (parent orchestrator):**
| Tool | Behavior |
|------|----------|
| `diff_subagent` | Diff snapshot or live path vs parent HEAD |
| `land_subagent` | Apply via existing `workspace.apply_worktree` (Merge default) |
| `discard_subagent` | Cleanup path; keep ref optional |

**CLI (parity):**
```text
hyper subagent list
hyper subagent open <id>
hyper subagent diff <id>
hyper subagent land <id>
hyper subagent discard <id>
hyper subagent prune --older-than 24h
```

**Spawn flags:**
- `retain_worktree: bool` ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â skip delete after snapshot; state=live  
- Default for RC8: **export patch always**; retain dirty success **or** keep delete-after-patch for disk (product call ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â recommend **retain dirty success + 24h GC** to match Ãƒâ€šÃ‚Â§3.5)

**Files:**
- New tool modules under `xai-grok-tools`
- Wire `apply_worktree` from workspace RPC
- Pager CLI registration
- Docs user-guide `16-subagents.md`

**Acceptance:**
- Parent can land Grok/Terra worktree fix without manual git ref  
- Conflict does not silent-overwrite  
- Isolation preserved until explicit land  

---

### WP6 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P1 NVIDIA platform defaults + catalog (Theme A)

| Fix | Action |
|-----|--------|
| `prompt_cache_key` | NVIDIA `fallback_request_compat`: `supports_prompt_cache_key=false`; stamp only when compat true (opt-in) |
| Other NVIDIA quirks | `supports_store=false`, `supports_developer_role=false`, `supports_strict_mode=false`, `max_tokens` field |
| Token clamp | Runtime `min(requested, catalog.max, context_window)`; fix Nano 9B catalog (ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤128000, sane max out) |
| Parallel tools | `parallel_tool_calls: false` when `max_parallel_tool_calls=1`; set for Llama 3.1 70B NVIDIA |
| EOL/404 | Hide or `supported_in_api=false`: step-3.5-flash, mistral-small-4, mistral-large-3, kimi-k2.6 |
| `agent_ready` | Catalog flag; default NVIDIA false; allowlist carefully after WP1 |

**Files:**
- `xai-grok-models/src/provider_compat.rs`, `platforms.rs`
- `platform_catalog.json` + `scripts/sync_pi_providers.py`
- `xai-grok-sampler/src/client.rs` (stamp opt-in, parallel_tool_calls field on request)
- `xai-grok-sampling-types` request type

**Oracle pin (docs + doctor):**
```toml
# Recommended until NVIDIA agent_ready
[subagents.models]
oracle = "xai/grok-4.5"   # or current Grok agent slug
# Do NOT pin Ultra for tool oracle until WP1 green
```
- `/doctor` warn if oracle pin has `agent_ready=false` or known-broken tools  
- No forced code default model pin (keep inherit); documentation + doctor only  

---

### WP7 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â P1 error_class + heartbeats (Themes C/E)

**ErrorClass enum:**
`Serialize | Provider400 | Provider5xx | Stall | Timeout | Eol | Auth | Budget | Cancelled | Conflict | Unknown`

**Mapping:** pattern-match sampling errors + termination_reason + HTTP status.

**Heartbeats:**
- Enrich ACP progress: last_tool, last_progress_age_ms  
- Optional parent model injection at most every 60s (not every 8s UI tick)  

**Files:**
- Completion path in shell + tools result types
- Progress publisher

---

### WP8 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Continuous improvement loop (Theme E)

**HCIL = Hyper Continuous Improvement Loop** (compose, donÃƒÂ¢Ã¢â€šÂ¬Ã¢â€žÂ¢t replace).

```text
Outer: parent agent OR scheduler_create interval
Inner: stock workflow continuous-improve.rhai
  phase research  ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ explore RO (isolation none preferred)
  phase plan      ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ plan RO
  phase implement ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ parallel implementers (worktree, timeout, retain)
  phase verify    ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ reviewer RO on diffs + optional tests
  phase land      ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ land_subagent serial
  on stall        ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ free slot, checkpoint, replan or pause(no_progress)
  loop until stop criteria (max iters / hours / budget)
```

**RC8 deliverables:**
1. `LoopCheckpoint` JSON under session `loops/<id>/checkpoint.json`  
2. Stock workflow `.grok/workflows/continuous-improve.rhai` (or bundled)  
3. Skill/doc: how to run multi-hour improve with timeout/stall/land  
4. Extend `AgentOpts`: `timeout_ms`, `retain_worktree`  
5. Document dual concurrency: Hyper 4 vs nemotron-bridge 2  

**Defer:** full cross-process workflow journal restore; dedicated `/improve` CLI can be thin later.

---

### WP9 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Docs, known issues, release notes

| Doc | Update |
|-----|--------|
| `docs/KNOWN_ISSUES.md` | Add RC8 items; close fixed |
| `docs/user-guide` subagents | Lifecycle diagram; recover from snapshot; land CLI |
| `docs/user-guide` slash commands | `/deepaudit` next to `/deep-research` |
| `docs/design-oracle.md` | Anti-pin Ultra until agent_ready |
| NVIDIA agent-ready vs chat-ready | Badge semantics |
| CHANGELOG | RC8 section including `/deepaudit` |
| Honest naming | ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œsandbox / ephemeral clone-or-linkedÃƒÂ¢Ã¢â€šÂ¬Ã‚Â; not always `git worktree list` |

---

### WP10 ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â `/deepaudit` Ultracode-style deep audit (Theme F) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â **RC8 product feature**

**Problem:** Users want Claude Ultracode-class repo audits (dozens of agents, adversarial verify, one report) without hand-rolling Rhai or burning the parent context window. Hyper has `/deep-research` (web) and branch review, but **no first-class deep codebase audit command**.

**Product:**

```text
/deepaudit
/deepaudit nvidia subagent tool path
/deepaudit --size medium src/agent/subagent
/deepaudit {"scope":"ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦","focus":"bugs|security|nvidia|subagents|all","size":"small|medium|large"}
```

Also launchable as `/workflow deep-audit {ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦}` once registered.

**Quality pattern (must match Ultracode screenshots):**

```text
Scope        ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ map args + repo into investigation targets
Investigate  ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ parallel find:* explore agents (RO)
             ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ each emits falsifiable claims (id, file, claim, evidence hint)
Verify       ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ one agent per claim (or batch small); confirm | refute | unverified
Report       ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ Markdown: verified findings only + unverified appendix + coverage gaps
```

**Size guidelines (advice + hard caps):**

| Size | Target agents | `agent_budget` | When |
|------|---------------|----------------|------|
| `small` | &lt; 5 | 16 | One module / smoke |
| `medium` (default) | &lt; 15 | 48 | Feature / subsystem |
| `large` | &lt; 50 | 128 | Broad subsystem (closer to Claude mediumÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“large) |
| (script max) | never exceed budget | ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤1024 runtime max | Runaway guard |

**Cost caution:** If planned agents &gt; 25 or projected large, log/UI advisory (mirror Claude ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œLarge workflowÃƒÂ¢Ã¢â€šÂ¬Ã‚Â warning). Do not hard-block; user opted into `/deepaudit`.

**Agent defaults for audit children:**
- `capability_mode: read-only` (no parent tree edits)
- Prefer `isolation: none` for pure RO (skip worktree clone cost) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â once RO skip-sandbox lands; until then isolation=none is fine for RO
- Per-agent `timeout_ms` via WP3 (e.g. 10ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“15 min find, 5ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“10 min verify)
- Labels: `find:ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦` / `verify:ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦` for `/workflows` readability

**Implementation steps:**

1. **Script** `crates/codegen/xai-grok-shell/src/session/workflows/deep_audit.rhai`
   - Mirror structure of `deep_research.rhai` (phases, schemas, parallel, verify)
   - Borrow dual-confirm ideas from `review-current-branch.rhai`
   - Args: `scope`, `focus`, `size`, optional `paths[]`, optional `objective`
2. **Registry** add to `BUILTIN_WORKFLOWS` in `session/workflow/registry.rs`
3. **Slash** `/deepaudit` in `slash_commands.rs` (same path as `/deep-research`)
   - Parse free text ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ `args.scope` / `args.objective`
   - Optional flags: `--size small|medium|large`
4. **Pager empty-state copy** update `views/workflows.rs` (ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œStart with `/deep-research` or `/deepaudit`ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â)
5. **User-guide** `04-slash-commands.md` + optional `docs/deepaudit.md` short design
6. **Report sink:** write to session artifact + emit final report into conversation (same as deep-research)
7. **Smoke test:** `validate_only` workflow path + unit test that registry resolves `deep-audit`

**Depends on:** WP3 (`timeout_ms` on workflow `AgentOpts`) strongly recommended before advertising `large`. Can ship medium with agent_budget alone if timeouts not ready.

**Acceptance:**
- `/deepaudit <topic>` starts a background workflow visible in `/workflows`
- Phases include at least Investigate + Verify + Report
- Default medium stays under ~15 agents / budget 48
- Report lists only verified findings in the main body
- `large` does not exceed `agent_budget` (no runaway parallel())
- RO children do not mutate parent working tree

**Out of WP10 / RC9:**
- Keyword `ultracode` auto-trigger in composer
- `/effort ultracode` session mode
- Model-authored dynamic Rhai for arbitrary tasks (user can still ask agent to write a workflow)
- 90+ agent default (Claude-scale) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â opt-in large only, never default

---

## 5. Priority matrix (ship order)

| Order | WP | Priority | Risk | Est. scope |
|------|-----|----------|------|------------|
| 1 | WP1 NVIDIA deser | P0 | Medium (serde surface) | SmallÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“med |
| 2 | WP2 Patch + structured recovery | P0 | Medium | Medium |
| 3 | WP3 timeout_ms | P0 | LowÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“med | Medium |
| 4 | WP4 stall detector | P0/P1 | Med (false stalls) | Medium |
| 5 | WP5 land/diff/open + retain | P0/P1 | Med (land safety) | Large |
| 6 | WP6 NVIDIA platform/catalog | P1 | Low | Medium |
| 7 | WP10 `/deepaudit` | **P1 product** | Med (cost / scale) | Medium |
| 8 | WP7 error_class + heartbeats | P1 | Low | Medium |
| 9 | WP8 HCIL workflow + checkpoint | P1/P2 | Med | Medium |
| 10 | WP9 docs/release | ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â | Low | Small |

**Suggested RC8 cut lines:**

| Tier | Include |
|------|---------|
| **RC8-must** | WP1ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“WP3, WP2 full, WP5 tools at least `diff`+`land`, WP6 critical catalog/clamp/cache_key, **WP10 `/deepaudit` medium default** |
| **RC8-should** | WP4 stall, WP5 retain/CLI, WP7, WP8 stock improve workflow, WP10 large size + cost caution UI |
| **RC8-nice / RC9** | spawn_many, path allowlists, real git worktree force, panel UI, pre-land hooks, nightly NVIDIA matrix CI, ultracode keyword / effort mode |

---

## 6. File map (primary touch points)

```text
NVIDIA / sampling
  crates/codegen/xai-grok-sampling-types/src/types.rs
  crates/codegen/xai-grok-sampler/src/client.rs
  crates/codegen/xai-grok-models/src/{provider_compat,platforms}.rs
  crates/codegen/xai-grok-models/platform_catalog.json
  crates/codegen/xai-grok-models/scripts/sync_pi_providers.py

Subagent lifecycle
  crates/codegen/xai-grok-shell/src/agent/subagent/{mod,handle_request}.rs
  crates/codegen/xai-grok-shell/src/session/worktree.rs
  crates/codegen/xai-fast-worktree/
  crates/codegen/xai-grok-workspace/src/worktree/

Tools / types
  crates/common/xai-tool-types/src/task.rs
  crates/codegen/xai-grok-tools/src/implementations/grok_build/task/
  crates/codegen/xai-grok-tools/src/implementations/grok_build/ (new land/diff)

Workflow + deep audit
  crates/codegen/xai-workflow/src/host.rs
  crates/codegen/xai-grok-shell/src/session/workflow/
  crates/codegen/xai-grok-shell/src/session/workflows/deep_research.rhai   # pattern to copy
  crates/codegen/xai-grok-shell/src/session/workflows/deep_audit.rhai     # NEW WP10
  crates/codegen/xai-grok-shell/src/session/workflow/registry.rs         # BUILTIN_WORKFLOWS
  crates/codegen/xai-grok-shell/src/session/slash_commands.rs            # /deepaudit
  crates/codegen/xai-grok-pager/src/views/workflows.rs                   # empty-state copy
  .grok/workflows/review-current-branch.rhai                            # dual-verify reference

Config / agent defs
  crates/codegen/xai-grok-agent/src/config.rs
  crates/codegen/xai-grok-config-types/

Docs
  docs/RC8_IMPLEMENTATION_PLAN.md (this file)
  docs/HYPER_DEVELOPER_FEEDBACK.md
  docs/KNOWN_ISSUES.md
  CHANGELOG.md
  user-guide 04-slash-commands.md, 16-subagents
```

---

## 7. Testing strategy

### Unit
- Null usage / null index / tool_calls null fixtures  
- prompt_cache_key stamp/strip matrix (NVIDIA vs OpenAI vs no compat)  
- Token clamp  
- parallel_tool_calls=false when max=1  
- Meta worktree_state transitions  
- Patch written before remove  
- Budget timeout resolution order  

### Integration (local)
- Spawn worktree agent ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ complete ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ patch exists ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ land into temp parent  
- timeout_ms enforces kill + snapshot  
- Stall synthetic child  
- Isolation probe: parent file stays buggy, child fixed  
- **`/deepaudit` medium** on a tiny fixture dir: finishes, phases present, report has schema  
- `deep-audit` resolves from builtin registry; `validate_only` path green  
- Large size respects `agent_budget` (reject oversized parallel panel)  

### Live (NVIDIA key, optional nightly)
| Case | Models |
|------|--------|
| Text smoke max_tokens=64 | Catalog matrix |
| One tool | Super/Nano/Ultra/GLM after WP1 |
| Multi-tool + parallel=false | Llama 70B |
| EOL probe | Expect quarantine |
| Optional: `/deepaudit nvidia` medium | Grok/strong agent models (not Ultra tools until WP1) |

### Soak
- Existing `test_subagent_soak` still green  
- Concurrent 4 with short timeouts  
- Deep-audit medium does not leak active children after complete  

---

## 8. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| False stall on long compile | Count shell activity as progress; longer stall for multi-hour profile |
| Disk pressure from retain | 24h GC; patch-only archive after land; sparse/copy filters later |
| Land overwrites parent | Merge mode only default; no land_on_complete for multi-agent |
| Deser change breaks strict providers | NullÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢0 only; fixtures for OpenAI+NVIDIA |
| timeout confuses with wait timeout | Docs + distinct field names; schema descriptions |
| Workflow multi-hour process death | LoopCheckpoint JSON (not only Rhai journal) |
| Windows linked worktree issues | Keep clone/standalone path; fix UX first (WP2) |
| `/deepaudit` cost explosion (Claude-scale 7M+ tokens) | Default **medium**; size guideline; agent_budget hard cap; cost caution &gt;25 agents |
| Deep-audit false findings | Adversarial verify phase; drop refuted; label unverified |
| Deep-audit zombies without timeouts | Ship WP3 before advertising large; RO + timeout on every agent() |

---

## 9. Research agent findings (summary)

Four Grok 4.5 passes (exploreÃƒÆ’Ã¢â‚¬â€3 + oracle) converged:

1. **Timeout + snapshot honesty first** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â unblocks everything else  
2. **Land tools wrap existing apply_worktree** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â no second merge engine  
3. **Slot pool already exists (4)** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â loop driver must refill, not reimplement  
4. **Stall = progress signature flat** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â reuse Running snapshot fields  
5. **HCIL should be workflow + checkpoint**, not a new orchestrator actor  
6. **NVIDIA deser is types.rs null u32** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â highest confidence root cause  
7. **Oracle Ultra pin is user config** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â doctor + docs, not code pin to Ultra  
8. **`/deepaudit` is productized Ultracode** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â reuse Rhai + verify pattern; default medium, never 90-agent by default  

---

## 10. Ideal multi-hour loop (product target)

```text
1. Parent plans feature ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ checklist ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ LoopCheckpoint
2. spawn explore (RO, no/light worktree, timeout 15m)
3. spawn implementer A/B (worktree, allow paths later, timeout 15ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“45m)
4. Heartbeats ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ parent logs; stall/timeout free slots
5. Complete ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ structured diffs + patches + optional tests in sandbox
6. spawn reviewer (RO on snapshots)
7. parent land A, land B (serial; conflict ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ hold)
8. parent verifies main tree ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ replan ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ hours of iteration
```

**RC8 ships the substrate (1ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“7 reliability).** Full path allowlists and auto-reviewer chain can land as follow-ups once land+timeout are green.

---

## 11. Implementation checklist (track in PR(s))

### Must (RC8)
- [x] WP1: Null-tolerant Chat Completions deser + fixtures  
- [x] WP2: changes.patch + diffstat + meta state + snapshot_ref in completion  
- [x] WP3: `timeout_ms` on spawn + NVIDIA default 600s + timeout snapshot  
- [x] WP5a: `diff_subagent` + `land_subagent` tools  
- [x] WP6a: NVIDIA platform cache_key false + token clamp + EOL hide  
- [x] **WP10a: `/deepaudit` builtin workflow + slash (medium default, RO, verify phase)**  
- [x] WP9: CHANGELOG + KNOWN_ISSUES + subagent recovery + `/deepaudit` docs  

### Should (RC8 if capacity)
- [x] WP4: Stall detector  
- [x] WP5b: retain_worktree + CLI list/open/diff/land/discard/prune  
- [x] WP6b: parallel_tool_calls + agent_ready flags  
- [x] WP7: error_class (+ progress publisher exists; optional last_tool age polish residual)  
- [x] WP8: continuous-improve workflow + LoopCheckpoint  
- [x] **WP10b: size small/medium/large + agent_budget mapping + large cost caution**  
- [x] Oracle doctor warning for non-agent-ready pins  

### Pre-r8 residual batch (pull-forward; hold release until done)
Ship these before packaging r8. Supervisors may parallelize with worktree agents.

- [x] **R1** WP7 residual: `last_tool` + `last_progress_age_ms` on `SubagentProgress` ACP ticks; ensure completion meta `land_status=pending` when worktree artifacts exist  
- [x] **R2** Path allowlists: `allowed_paths` on spawn + land/diff filter (refuse out-of-allowlist)  
- [x] **R3** RO / explore|plan|oracle default `isolation=none` when spawn omits isolation (explicit worktree still honored)  
- [x] **R4** `spawn_many` tool (or Task multi-spawn) + barrier wait respecting max concurrency  
- [x] **R5** `/ultracode` slash alias â†’ deep-audit (full `/effort ultracode` mode can follow)  
- [x] **R6** Durable LoopCheckpoint under session `loops/<id>/` (cross-process readable) when continuous-improve runs  
- [x] **R7** NVIDIA conformance unit fixtures expanded + optional CI job (live key optional)  

### Later (true RC9 / post-r8)
- [ ] Real git worktree preference + sparse filters  
- [ ] Pre-land test hooks; required final schema  
- [ ] Subagent panel UI; session export report  
- [ ] Full cross-process workflow journal (beyond LoopCheckpoint)  
- [ ] Nightly live NVIDIA matrix (key-gated)  
- [ ] `/effort ultracode` session mode + free-text keyword auto-trigger  
- [ ] Model-authored dynamic workflows for arbitrary tasks (beyond bundled audit)  

---

## 12. Suggested PR stack

| PR | Title | Depends |
|----|-------|---------|
| PR1 | fix(sampler): null-tolerant Chat Completions deser (NVIDIA) | ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â |
| PR2 | feat(subagents): patch export + structured snapshot completion | ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â |
| PR3 | feat(subagents): timeout_ms + platform defaults | PR2 (dispose path) |
| PR4 | feat(subagents): land/diff tools | PR2 |
| PR5 | fix(models): NVIDIA platform compat, clamp, EOL catalog | PR1 |
| PR6 | feat(subagents): stall detector + error_class | PR3 |
| **PR7** | **feat(workflows): `/deepaudit` builtin + slash (Ultracode-style)** | **PR3 preferred** |
| PR8 | feat(loop): continuous-improve workflow + checkpoint | PR3ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“PR4 |
| PR9 | docs: RC8 release notes + recovery + deepaudit guide | all |

Can parallelize PR1 and PR2 immediately. Draft PR7 script early from `deep_research.rhai`; advertise `large` only after PR3 timeouts land.

---

## 13. Success criteria for RC8 ship

1. **NVIDIA tool smoke** on at least one Nemotron model completes without client serialize error (or model honestly `agent_ready=false` and tools gated).  
2. **No silent work loss:** every completed/failed/timed-out worktree agent leaves patch and/or snapshot_ref visible to parent.  
3. **No 27-minute zombie:** default or explicit timeout/stall frees slots.  
4. **Parent can land** child work with one tool/CLI without knowing `refs/grok/subagents/ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦`.  
5. **Grok/Terra control paths** remain green (isolation + coding).  
6. **`/deepaudit` works:** medium audit runs in background, shows Investigate/Verify/Report in `/workflows`, returns a verified-findings report without mutating the parent tree.  
7. **Docs** explain recovery, NVIDIA chat vs agent readiness, and `/deepaudit` cost/size.  
8. Version **0.2.114-r8** CHANGELOG accurate.

---

## 14. Immediate next actions

1. ÃƒÂ¢Ã…â€œÃ¢â‚¬Â¦ Feedback read + full audit + research agents + this plan  
2. ÃƒÂ¢Ã…â€œÃ¢â‚¬Â¦ `/deepaudit` productized into RC8 plan (Theme F / WP10)  
3. **Start PR1** (NVIDIA deser) and **PR2** (patch + structured completion) in parallel  
4. Then PR3 (`timeout_ms`) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â highest field-impact for multi-agent **and** safe deep audits  
5. Wire land tools (PR4) so multi-agent feature development is possible  
6. Catalog/platform NVIDIA (PR5)  
7. **PR7 `/deepaudit`** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â bundled workflow + slash (draft script early; ship with/after PR3)  
8. Stall + HCIL stock workflow as capacity allows  

---

## 15. `/deepaudit` detailed design (Theme F / WP10)

Canonical implementation checklist lives in **Ãƒâ€šÃ‚Â§4 WP10**. This section is the design reference.

### 15.1 Reference: Claude Ultracode behavior

Observed run: `nemotron-ultra-subagent-audit-wf_ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦` ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â **91 agents**, **7.4M tokens**, **56m15s**.

| Phase | Pattern | Purpose |
|-------|---------|---------|
| **Investigate** (~7 agents) | `find:*` parallel map | Partition problem space |
| **Verify** (~80+ agents) | `verify:<hypothesis-slug>` | One agent per claim ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â adversarial confirm/refute |
| **Report** | synthesize survivors | Verified only; unverified appendix |

Not ÃƒÂ¢Ã¢â€šÂ¬Ã…â€œone smarter model.ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â Script holds plan ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ investigate fan-out ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ hypothesis explosion ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ verify fan-out ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ filter ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ report. Background; `/workflows` progress.

### 15.2 UX & args

```text
/deepaudit
/deepaudit nvidia subagent tool path
/deepaudit --size large src/agent/subagent
/workflow deep-audit {"scope":"ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦","focus":"nvidia","size":"medium"}
```

| Arg | Values | Default |
|-----|--------|---------|
| `scope` / free text | path, crate, topic | workspace |
| `focus` | `bugs` \| `security` \| `nvidia` \| `subagents` \| `all` | `all` |
| `size` | `small` \| `medium` \| `large` | `medium` |
| `paths` | optional string array | ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â |
| `objective` | free-text goal | from free text |

| Size | Agents (guide) | `agent_budget` |
|------|----------------|----------------|
| small | &lt; 5 | 16 |
| medium | &lt; 15 | 48 |
| large | &lt; 50 | 128 |

### 15.3 Phase machine

```text
Scope ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ Investigate (find:*) ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ Verify (verify:*) ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ Report ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ complete
```

- RO children; labels readable in `/workflows`
- Claims schema: id, title, severity, file, line?, claim, evidence_hint
- Verdicts: confirm | refute | unverified
- Cost caution when agents &gt; 25

### 15.4 Hyper mapping

| Claude | Hyper |
|--------|-------|
| Dynamic JS workflow | Rhai `deep_audit.rhai` |
| `/deep-research` | Pattern to copy |
| Branch multi-verify | `review-current-branch.rhai` |
| `/workflows` | Existing UI |
| Ultracode keyword | **RC9** |

### 15.5 Related: pasted image preview blur

Model receives sharp PNGs; Windows Hyper preview often uses half-block raster resized to cell grid (`halfblock.rs`) ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ looks blurry. Separate UX polish, not WP10-blocking.

---

*Plan authors: Hyper parent agent + Grok 4.5 explore/oracle research agents (2026-08-01). `/deepaudit` (Theme F / WP10) added from Claude Ultracode docs + live audit screenshots. Source of truth for implementation priorities until superseded by release notes.*
