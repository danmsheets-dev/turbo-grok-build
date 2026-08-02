> **Superseded for r9 (2026-08-02):** see [HYPER_DEVELOPER_FEEDBACK_20260802.md](./HYPER_DEVELOPER_FEEDBACK_20260802.md) and top-level `H:\Apps\grok build\HYPER_DEVELOPER_FEEDBACK_20260802.md`.
> This file remains as the 2026-08-01 NVIDIA/worktree feedback archive.
# Hyper Developer Feedback â€” NVIDIA Link, Subagents & Worktrees

**Date:** 2026-08-01  
**Reporter session:** parent `019fbed2-0196-7ba0-9a6e-f33e49631c3d`  
**Workspace:** `H:\Apps\testing` (Windows)  
**CLI:** Hyper (`C:\Users\dan_m\.hyper\bin\hyper.exe`) + Grok Build  
**Scope:** Live audit of NVIDIA Integrate models via **built-in `spawn_subagent`**, **worktree isolation**, concurrency up to 4, plus Grok 4.5 and OpenAI Codex Terra controls.

Related artifacts:
- `results/NVIDIA_MODELS_USES_TABLE.md` â€” model capability table
- `results/LIVE_TEST_LOG.md` â€” chronological run log
- `results/EVALUATION_REPORT.md` â€” prior completion-only coding eval
- `results/HYPER-DEV-REPORT-nemotron-ultra-subagents.md` â€” earlier Ultra-focused report (still valid; this doc supersedes/expands)

**New in this revision:** Â§3.5 Ideal worktree handling Â· Â§3A Features that help supervisors develop

---

## 1. TL;DR for Hyper engineers

| Priority | Issue | Impact |
|----------|--------|--------|
| **P0** | Tool/stream **deserialization** `invalid type: null, expected u32` on many NVIDIA models | Tool-using subagents die after successful API tokens |
| **P0** | **Worktree directory deleted** on complete/fail; path in `meta.json` is a tombstone | Supervisors report â€œwork disappearedâ€; hard to find agent edits |
| **P0** | **No hard timeout** on subagents â†’ NVIDIA can stall **20â€“30+ min** with no progress | Wasted cost; blocks concurrency slots |
| **P0** | No **structured land/diff** of child work for parent orchestrators | Parent cannot reliably merge multi-agent development |
| **P1** | Worktrees are **not** visible in `git worktree list` (clone-like full `.git` dir under `~/.grok/worktrees/`) | Mental model mismatch; debugging harder |
| **P1** | NVIDIA **strict API** quirks: `prompt_cache_key`, `max_completion_tokens > max_model_len`, **single tool-call only** (Llama 70B) | 400s look like â€œmodel brokenâ€ |
| **P1** | Catalog still offers **EOL 410** / **404** models | Bad UX for users picking models |
| **P2** | Snapshot refs exist (`refs/grok/subagents/<id>`) but are **undiscoverable** in product UI | Work is recoverable only if you know the ref |
| **P2** | Concurrent 4 is fine for Hyper; nemotron-bridge defaults to 2 | Document dual limits |
| **P2** | Oracle pinned to Ultra â†’ tool oracle likely broken | Wrong default for real code investigation |

**Controls prove the stack works:** Grok 4.5 (63s, 14 tools, 10/10+9/9) and Terra (109s, 17 tools, 10/10+9/9) completed full bug hunt + coding in isolated worktrees. NVIDIA failures are largely **provider + policy + deser**, not â€œsubagents fundamentally broken.â€

---

## 2. Environment facts

| Item | Value |
|------|--------|
| OS | Windows |
| NVIDIA Integrate | ready; 102 models |
| Auth | `platform/nvidia` key in Hyper auth |
| Worktree root observed | `C:\Users\dan_m\.grok\worktrees\apps-testing\subagent-<uuid>` |
| Session meta | `~\.grok\sessions\<encoded-cwd>\<parent>\subagents\<id>\meta.json` |
| Snapshot refs | `refs/grok/subagents/<id>` (persist after WT delete) |
| Parent pollution | **None** â€” probe files and bugfixes stayed in worktrees only |

---

## 3. Worktrees â€” special focus (supervisor pain)

### 3.1 What we observed

1. **On spawn (`isolation: worktree`):** Hyper creates  
   `C:\Users\dan_m\.grok\worktrees\apps-testing\subagent-<subagent_id>\`  
   containing a full project copy (`.git` is a **directory**, not a gitdir file pointer).

2. **`git worktree list` in the parent repo only shows the main checkout.**  
   Live Hyper isolation dirs do **not** appear as git worktrees. Supervisors searching with `git worktree list` will conclude â€œno worktree exists.â€

3. **On completion or failure:** the filesystem worktree is **removed immediately**.  
   Confirmed mid-session: during runs dirs exist; after status completed/failed, `~\.grok\worktrees\apps-testing\` is empty (or only still-running agents).

4. **What survives:**
   - `meta.json` with `worktree_path`, `child_cwd`, `snapshot_ref`, model, error
   - Git ref: `refs/grok/subagents/<id>` â€” commit message like `subagent worktree snapshot â€¦`
   - Parent transcript / tool result text (if the agent returned a summary)

5. **What is hard to find:**
   - Actual edited files after auto-delete (must `git show` / `git checkout` the snapshot ref)
   - Probe files agents wrote (e.g. `results/grok45_worktree_probe.txt`) vanish with the tree
   - Failed runs still delete the tree even when no useful snapshot of edits exists

6. **Isolation correctness (good):** Parent `tasks/debugging/json_path.py` remained intentionally buggy while Grok/Terra fixed copies only inside their worktrees. **Do not lose this property.**

### 3.2 Why supervisors say â€œworktree was removed / work hard to findâ€

This is **by design today**, but the product does not make recovery obvious:

- UI/tool result often shows `WORKTREE_CWD: C:\Users\dan_m\.grok\worktrees\...\subagent-â€¦` which is **already gone** when the supervisor opens it.
- No first-class â€œOpen snapshotâ€ / â€œLand worktreeâ€ affordance in the default subagent completion card.
- Naming: â€œworktreeâ€ implies `git worktree`, but implementation is closer to **ephemeral clone + snapshot ref**.

### 3.3 Recommendations (worktrees)

| # | Recommendation | Why |
|---|----------------|-----|
| W1 | **Keep worktree on disk until parent lands or user discards** (or TTL 24h) | Default supervisor expectation |
| W2 | On complete, surface: **snapshot SHA**, **how to restore**, **diffstat vs parent** | Make work findable |
| W3 | Command: `hyper subagent land <id>` / `hyper subagent open <id>` | One-step recovery |
| W4 | Optional `isolation: worktree` + `retain_worktree: true` | Power users / eval harnesses |
| W5 | If delete remains default: **auto-export patch** to `~\.grok\sessions\...\subagents\<id>\changes.patch` | Survive deletion |
| W6 | Document honestly: â€œephemeral sandbox; not `git worktree add`â€ | Fix mental model |
| W7 | Show live path in a panel **only while running**; after exit show snapshot ref not dead path | Stop 404 confusion |
| W8 | Include `worktrees/` under project in snapshot **or** exclude nested eval worktrees from clone cost | Clones currently copy large `worktrees/` trees (slow/heavy) |

### 3.4 Snapshot recovery (works today if you know the trick)

```text
git show refs/grok/subagents/<subagent_id>:tasks/debugging/json_path.py
git diff HEAD refs/grok/subagents/<subagent_id> -- tasks/
```

Verified: Grok bug-hunt snapshot contains the fixed `json_path.py` after the live directory was deleted.

### 3.5 Ideal ways subagents should handle worktrees

This section is a **target design** for Hyper engineers: what â€œgoodâ€ looks like for supervisors, multi-agent pipelines, and eval harnesses. Grounded in this sessionâ€™s pain (deleted trees, dead paths in meta, invisible to `git worktree list`, good isolation).

#### Principles

| Principle | Meaning |
|-----------|---------|
| **Isolate by default** | Child never writes the parent working tree unless explicitly landed. |
| **Never lose work** | Complete, fail, timeout, or kill always leaves a recoverable artifact. |
| **Honest naming** | Call it a sandbox if it is not `git worktree add`; document both. |
| **Supervisor-visible** | Parent always knows: live path (while running), snapshot ref, diffstat, land status. |
| **Explicit lifecycle** | Create â†’ work â†’ snapshot â†’ (review) â†’ land **or** discard â†’ cleanup. Never silent delete-only. |
| **Cheap when possible** | Prefer true git worktrees or sparse checkouts over full clones that copy nested `worktrees/`. |
| **Safe concurrency** | N worktrees, one branch/ref each, no cross-talk; land order is parent-controlled. |

#### Ideal lifecycle (state machine)

```text
  SPAWN
    â”‚
    â–¼
  CREATED â”€â”€ worktree path live, branch/ref allocated, meta.status=running
    â”‚
    â”œâ”€(agent works)â”€â”€â–º RUNNING  (heartbeat: last tool, files dirty, tokens)
    â”‚
    â”œâ”€ success â”€â”€â–º SNAPSHOTTED  (ref + optional patch + diffstat; path still live OR archived)
    â”‚                  â”‚
    â”‚                  â”œâ”€ parent: LAND     â†’ MERGED into parent (or PR branch) â†’ CLEANED
    â”‚                  â”œâ”€ parent: DISCARD  â†’ CLEANED (ref kept for N days optional)
    â”‚                  â””â”€ TTL expire       â†’ ARCHIVED (tar/patch only) â†’ CLEANED
    â”‚
    â”œâ”€ fail / timeout / kill â”€â”€â–º SNAPSHOTTED (best-effort dirty tree) â†’ same land/discard
    â”‚
    â””â”€ never: DELETE_WITHOUT_SNAPSHOT
```

#### Ideal defaults by isolation mode

| Mode | When to use | Worktree behavior |
|------|-------------|-------------------|
| `isolation: none` | Tiny RO Q&A, no file writes | No sandbox; forbid writes or warn |
| `isolation: worktree` (default for write agents) | Coding, bugfix, multi-file | Sandbox always; **retain until land/discard** |
| `isolation: worktree` + `ephemeral: true` | Throwaway smoke tests | May auto-clean after snapshot + patch export |
| `retain_worktree: true` | Eval, long review, human inspect | Keep path until explicit prune |
| `land_on_complete: true` | Trusted single-agent â€œjust do itâ€ | Auto-merge clean tests-only diffs; else hold for review |

#### What every completed subagent should return to the parent

```json
{
  "subagent_id": "...",
  "status": "completed|failed|timed_out|cancelled",
  "worktree": {
    "state": "live|archived|cleaned",
    "path": "C:\\...\\subagent-...  | null if cleaned",
    "snapshot_ref": "refs/grok/subagents/...",
    "snapshot_sha": "abc123",
    "diffstat": { "files_changed": 2, "insertions": 40, "deletions": 12 },
    "patch_path": ".../changes.patch",
    "branch": "grok/subagent/...",
    "land_status": "pending|landed|discarded|conflict"
  },
  "error_class": null
}
```

Today we get a final text blob + meta with a **dead** `worktree_path`. Ideal: structured worktree block the parent can act on without guessing.

#### Ideal user/supervisor commands

| Command / action | Purpose |
|------------------|---------|
| `hyper subagent list` | Running + completed with land status |
| `hyper subagent open <id>` | Open live path or restore snapshot to temp dir |
| `hyper subagent diff <id>` | Show diff vs parent HEAD |
| `hyper subagent land <id>` | Apply snapshot to parent (or open PR) |
| `hyper subagent discard <id>` | Drop sandbox; keep ref optional |
| `hyper subagent prune --older-than 24h` | Reclaim disk |
| Parent tool: `land_subagent` / `get_subagent_diff` | So **orchestrator agents** can merge without shell archaeology |

#### Ideal git mechanics (preferred implementation)

**Option A â€” real git worktrees (preferred):**
- `git worktree add ../.grok/worktrees/... -b grok/subagent/<id>`
- Appears in `git worktree list`
- Snapshot = commit on that branch; land = merge/cherry-pick/PR
- Cleanup = `git worktree remove` after land/discard

**Option B â€” clone sandbox (current-like) but fixed UX:**
- Keep clone if Windows worktree issues force it
- Always write `changes.patch` + `snapshot_ref` before delete
- Never leave meta pointing at deleted paths without `state: cleaned`

**Either way:** exclude heavy dirs from sandbox materialization (e.g. nested `worktrees/`, `results/runs/`, `__pycache__`) via sparse checkout or copy filters â€” this sessionâ€™s sandboxes copied large eval trees unnecessarily.

#### Ideal concurrency rules

1. Each subagent gets **its own** worktree/branch; never share a dirty tree.  
2. Parent is the only process that **lands** (serial land queue or explicit merge order).  
3. If two agents edit the same files, land #2 reports **conflict** with a three-way diff â€” do not silently overwrite.  
4. Optional file-level leases: parent can pass `allowed_paths: ["tasks/debugging/**"]` so sandboxes cannot touch unrelated trees.  
5. RO text agents should skip worktree creation entirely (save disk/time) â€” this session created full trees even for â€œno toolsâ€ creative chat.

#### Ideal failure behavior

| Event | Worktree action |
|-------|-----------------|
| Agent crash / serialize error | Snapshot dirty tree if any; keep path or patch; status=failed |
| Timeout / kill | Same; status=timed_out |
| Clean success, zero file changes | Snapshot optional; mark `diffstat.empty=true`; cleanup OK |
| Success with changes | **Do not delete** until land/discard (or export patch + retain ref) |
| Parent session ends | Persist worktrees + meta; show â€œorphaned subagent workâ€ on next open |

#### Anti-patterns to avoid (observed or likely)

1. Delete sandbox immediately on complete with only an opaque ref.  
2. Print `WORKTREE_CWD` that 404s when the human clicks it.  
3. Name it â€œworktreeâ€ while `git worktree list` is empty.  
4. Full-repo clone including previous eval sandboxes.  
5. Land automatically with no test gate on multi-file agent output.  
6. Require humans to know `refs/grok/subagents/...` to recover a day of work.

#### Minimal viable fix vs ideal

| Priority | Change |
|----------|--------|
| **MVP (this week)** | Before delete: write `changes.patch` + show `snapshot_ref` + `diffstat` in completion; meta `worktree.state=cleaned` |
| **Next** | Retain path until land/discard; `open` / `diff` / `land` commands |
| **Ideal** | Real git worktrees + structured return object + path allowlists + RO skip-sandbox |

---

## 3A. Features that would help supervisors (and this agent) develop better

Context: the parent model is an **orchestrator**. It plans, spawns up to N children, merges results, and ships. Features below are ordered by how much they would have improved **this NVIDIA audit session** and day-to-day feature development.

### 3A.1 Worktree & result plumbing (highest leverage)

| Feature | Why it helps development |
|---------|---------------------------|
| **Structured completion payload** (status, diffstat, snapshot_ref, patch_path, tests_run) | Parent can land/verify without re-prompting the child or hunting git refs |
| **`land_subagent` / `diff_subagent` tools** | One tool call to merge a childâ€™s work into parent after review |
| **`retain_worktree` + live path while running** | Parent can peek mid-run (`read_file` in child cwd) for stuck agents |
| **Auto `changes.patch` always** | Survives delete; easy to attach to PRs or feedback docs |
| **Path allowlists** (`allowed_paths`) | Safe parallel agents (A owns `api/`, B owns `ui/`) without merge hell |
| **Skip sandbox for RO** | Faster matrix evals (we paid full clone cost for pure text heroes) |

### 3A.2 Runtime control & reliability

| Feature | Why it helps development |
|---------|---------------------------|
| **`timeout_ms` on spawn** | Enforce 10 min NVIDIA policy without manual kill polling |
| **Stall detection** (no tool/token progress) | Free slots early; mark model unusable for agents |
| **Error classes** (`serialize`, `provider_400`, `stall`, `eol`) | Auto-retry only retryable errors; donâ€™t retry EOL 410 |
| **Heartbeat to parent** (last tool, turn, tokens, dirty files) | Orchestrator can refill slots and write live logs honestly |
| **Cancel that always snapshots** | Kill without losing partial work (we killed 27m stalls with empty trees) |
| **Per-model capability flags** | `agent_ready`, `max_parallel_tool_calls`, `max_tokens`, `supports_tools` â€” stop sending tools to chat-only models |

### 3A.3 Orchestration & concurrency

| Feature | Why it helps development |
|---------|---------------------------|
| **Slot pool API** (`max_concurrent=4`, auto-queue) | â€œKeep 4 runningâ€ without parent busy-looping wait/kill/spawn |
| **Fan-out primitive** | `spawn_many([{model, prompt}, ...])` + barrier â€” multi-model evals in one call |
| **Land queue / merge planner** | Parallel implementers â†’ serial integration with conflict reports |
| **Shared read-only context pack** | Inject repo map / AGENTS.md once; children donâ€™t re-explore from zero |
| **Resume with new timeout** | Continue a stalled child after config fix without full re-prompt |

### 3A.4 Quality gates for real development (not just smoke)

| Feature | Why it helps development |
|---------|---------------------------|
| **Pre-land hooks** | Run `pytest` / `lint` in the **child worktree** before land; block red merges |
| **Required final schema** | e.g. must include TEST_RESULT; mark incomplete if missing (Llama8 â€œSMOKE onlyâ€) |
| **Diff review subagent** auto-chained | After implementer completes, reviewer reads snapshot diff only |
| **Coverage of acceptance checklist** | Parent passes checklist; child must tick items in structured form |
| **Deterministic task fixtures** | Built-in â€œcoding+debug smokeâ€ like this repoâ€™s `tasks/` for model certification |

### 3A.5 Observability & developer experience

| Feature | Why it helps development |
|---------|---------------------------|
| **Subagent panel**: 4 slots, model, elapsed, last tool, kill, open WT | Replaces shell archaeology mid-pipeline |
| **One-click â€œexport session reportâ€** | Meta + patches + table â†’ markdown (what we hand-built as LIVE_TEST_LOG) |
| **Model badge: chat-ready / agent-ready** | Stop wasting hours on Ultra tools when catalog already knows |
| **Cost/token per child in completion** | Choose Super vs OSS-120 vs Grok with real data |
| **Raw failed response snippet** on serialize errors | NVIDIA deser debug without guessing column 330 |
| **Worktree disk usage + prune UI** | Long eval days leave refs and clones; need hygiene |

### 3A.6 Ideal supervisor workflow (feature development)

What â€œideal Hyperâ€ enables for a multi-step feature:

```text
1. Parent plans feature â†’ writes checklist
2. spawn explore (RO, no worktree) â†’ architecture notes
3. spawn implementer A (worktree, allow paths ui/**, timeout 15m, model Grok)
4. spawn implementer B (worktree, allow paths api/**, timeout 15m, model Terra)
5. heartbeats â†’ parent logs progress; kill/restart only stalls
6. both complete â†’ structured diffs + patches + pytest in sandbox
7. spawn reviewer (RO on both snapshots) â†’ findings
8. parent land A, land B (or open stacked PRs)
9. parent verifies on main worktree â†’ done
```

**Blockers today:** no structured land, worktrees vanish, weak timeout, NVIDIA agent path broken, no path allowlists, no pre-land test gate.

### 3A.7 Priority for Hyper (supervisor-centric)

| P | Feature |
|---|---------|
| **P0** | Retain or patch-export worktrees; structured completion with snapshot_ref + diffstat |
| **P0** | `timeout_ms` + cancelâ†’snapshot |
| **P0** | NVIDIA tool deser (or hide tools for non-agent-ready models) |
| **P1** | `land` / `diff` / `open` subagent tools for parent |
| **P1** | Heartbeats + error_class |
| **P1** | `agent_ready` + max_parallel_tool_calls + token clamp |
| **P2** | Fan-out + slot queue; path allowlists; pre-land test hook |
| **P2** | Session export report; model badges; RO skip-sandbox |

### 3A.8 What would have changed this NVIDIA audit

If the ideal set existed, this session would have been:

1. `spawn_many` Ã— 16 models Ã— {text, one_tool} with `timeout_ms=600000`  
2. Auto table of pass/fail/error_class without manual kill/log  
3. Every tool success leaving a **patch + diffstat** even after cleanup  
4. No 27-minute silent stall  
5. No supervisor confusion about â€œwhere did the worktree go?â€  
6. Oracle not pinned to a non-agent-ready Ultra  

---

## 4. Subagent feature feedback

### 4.1 What works well

- Spawning up to **4 concurrent** subagents is stable for Hyper orchestration.
- `capability_mode: read-only` vs `all` is useful for isolating text vs tools.
- `meta.json` is rich enough for postmortems (model, duration, tool_calls, error, paths).
- Isolation prevents parent corruption (critical for multi-agent).
- Controls (Grok, Terra) complete multi-tool coding loops cleanly.

### 4.2 Timeouts (user-requested policy: 10 min NVIDIA)

| Observation | Detail |
|-------------|--------|
| No enforced max | GPT-OSS 120B tools ran **~27 min** with 5 tool calls, **file never changed** |
| Same model short task | GPT-OSS 120B short tools still **stalled at 10 min** (2 tools, no final) |
| Llama 3.3 RO hero | **10 min**, 0 tools, never answered â†’ kill |
| Step 3.7 short tools | **390s** (~6.5 min) â€” finished but slow |

**Recommendations:**

| # | Change |
|---|--------|
| T1 | Per-subagent `timeout_ms` (or `max_duration`) honored by runtime |
| T2 | Default **600s** for `nvidia/*` models; **900â€“1200s** for Grok/OpenAI agent models |
| T3 | Stall detector: **no tool progress + no tokens for N minutes** â†’ fail with `STALL` |
| T4 | Surface elapsed + last tool in parent UI while running |
| T5 | On kill: still write snapshot + meta `status: timed_out` |

### 4.3 Concurrency

- Hyper handled 4 parallel fine.
- Nemotron-build bridge reports `maxConcurrent: 2` â€” separate limit; document both.
- When one fails fast (4s serialize), refill immediately works (this sessionâ€™s pipeline).

### 4.4 Prompt / model packaging quirks

| Model | Behavior |
|-------|----------|
| Llama 3.1 70B | Once: *â€œI was not given a promptâ€* on RO creative task; later planning prompt **worked** |
| GPT-OSS 20B tools | Narrated tool JSON confusion mid-loop; ended without required deliverable |
| Step 3.7 tools | Delivered structured final message; **bugs invented** (not the real intentional bugs) |
| Llama 8B | Tools ran; output only `SMOKE: OK` â€” weak instruction following |

**Recommendations:** stronger system framing for weak models; optional â€œfinal answer schemaâ€ validation before marking completed.

### 4.5 Model slug validation (good)

Unknown slug `openai/gpt-5.6-terra` failed fast with valid alternatives list. Correct slug: `openai-codex/gpt-5.6-terra`. Keep this error style.

---

## 5. NVIDIA link â€” model-specific behavior

### 5.1 Serialization error (`null, expected u32`)

**Signature:**
```text
serialization error: invalid type: null, expected u32 at line 1 column ~310â€“330
```

| Model | Mode | Result |
|-------|------|--------|
| Ultra 550B | RO creative | FAIL (this session) â€” note: earlier sessions saw RO OK |
| Ultra 550B | tools | FAIL (prior + expected) |
| Super 120B | RO creative | OK |
| Super 120B | tools | FAIL ~4s |
| Nano 30B | RO creative | OK |
| Nano 30B | tools | FAIL ~4s |
| GLM 5.2 | tools | FAIL after **9 tools / 4 model calls** (late crash) |
| MiniMax M3 | RO creative | FAIL 162s |

**Important:** API often returns tokens successfully (`outputTokens` > 0, multi-second `apiDurationMs`); crash is **client-side parse**.

**Ask:** capture raw body at failure column; use `Option<u32>` / defaults; add NVIDIA tool-call conformance tests; try Ultra with thinking disabled.

### 5.2 `prompt_cache_key` (known, partially mitigated)

NVIDIA 400 if Hyper sends `prompt_cache_key`. Local config sets `supports_prompt_cache_key = false` per model.  
**Ask:** platform-wide default for `nvidia` route, not per-model TOML.

### 5.3 `max_completion_tokens` overflow (Nano 9B)

```text
max_completion_tokens=131072 cannot be greater than max_model_len=128000
```

**Ask:** clamp requested max tokens to model catalog limit.

### 5.4 Single tool-call only (Llama 3.1 70B)

```text
This model only supports single tool-calls at once!
```

Hyper likely emits parallel tool calls.  
**Ask:** model capability flag `max_parallel_tool_calls = 1` and serialize tool use for those models.

### 5.5 Catalog dead entries

| Model | Error |
|-------|--------|
| step-3.5-flash | 410 EOL 2026-07-27 |
| mistral-small-4 / mistral-large-3 | 410 EOL (prior eval) |
| kimi-k2.6 | 404 (prior eval) |

**Ask:** hide or badge EOL models; periodic catalog health check.

### 5.6 Stall-prone models under tools

| Model | Pattern |
|-------|---------|
| gpt-oss-120b | Long hangs mid-tool loop; few tools; no disk writes |
| llama-3.3-70b | RO hang with no tokens progress |
| gpt-oss-20b | Partial thrash on tool args |

### 5.7 Models that are agent-viable (limited)

| Model | Verdict |
|-------|---------|
| step-3.7-flash | Can finish short tool tasks (~6+ min); quality dubious |
| gpt-oss-* | Good completion; agent loop unreliable |
| Nemotron family | Prefer **text-only** until deser fixed |
| Grok 4.5 / Terra | **Production agent quality** (controls) |

### 5.8 Oracle pin risk

```toml
[subagents.models]
oracle = "nvidia/nvidia/nemotron-3-ultra-550b-a55b"
```

Oracle needs tools to read code â†’ likely hits P0 serialize. Recommend pin to Grok or GPT-OSS until fixed; badge Ultra as **chat/planning** not **agent**.

---

## 6. Control results (subagent + worktree process)

### Grok 4.5

| Task | Result | Dur | Tools |
|------|--------|-----|-------|
| Hero + worktree probe | OK; path under `.grok\worktrees`; write succeeded | 29s | 2 |
| Bug hunt + coding | debug **10/10**, coding **9/9** | 63s | 14 |

Notes: excellent instruction following; worktree deleted after success; snapshot retained fixes.

### OpenAI Codex GPT-5.6 Terra

| Task | Result | Dur | Tools |
|------|--------|-----|-------|
| Hero + worktree probe | OK | 38s | 7 |
| Bug hunt + coding | debug **10/10**, coding **9/9** | 109s | 17 |

Notes: solid agent; slug must be `openai-codex/gpt-5.6-terra`.

### Process comparison (NVIDIA vs controls)

| Dimension | Controls | NVIDIA |
|-----------|----------|--------|
| Finish full coding agent task | Yes | No (except weak short tools) |
| Latency predictability | High | Low (stalls) |
| Tool reliability | High | Serialize / multi-tool / stall |
| Worktree isolation | Same mechanism | Same mechanism |
| Cost of failure | Low | High if no timeout |

---

## 7. Feedback on improving â€œthis processâ€ in Hyper

### 7.1 Eval / multi-model harness UX

1. Built-in **matrix runner**: list models Ã— (text smoke, one-tool, multi-tool) with timeout.  
2. Export results table + meta zip for support.  
3. Live panel: 4 slots, status, last tool, kill button.  
4. Auto-append to `LIVE_TEST_LOG` style report.

### 7.2 Subagent API / supervisor ergonomics

1. `timeout_ms` parameter on spawn.  
2. `retain_worktree` / `land_on_complete`.  
3. Return structured: `{ status, model, duration_ms, worktree_path_live, snapshot_ref, diffstat, error_class }`.  
4. Error classes: `serialize`, `provider_400`, `stall`, `eol`, `prompt_ignored` â€” not one opaque Internal error.  
5. Optional mid-run heartbeat events to parent.

### 7.3 NVIDIA platform defaults (config)

```toml
# Desired platform-level (pseudo)
[platform.nvidia]
supports_prompt_cache_key = false
supports_store = false
supports_developer_role = false
clamp_max_tokens_to_model = true
default_subagent_timeout_ms = 600000
default_max_parallel_tool_calls = 1  # override per model if known
agent_ready = false  # until deser fixed; override per model allowlist
```

Allowlist candidates once fixed: start with step-3.7 / gpt-oss carefully.

### 7.4 Documentation gaps

- Worktree lifecycle diagram (create â†’ run â†’ snapshot â†’ delete).  
- How to recover files from `refs/grok/subagents/*`.  
- NVIDIA â€œagent-readyâ€ vs â€œchat-readyâ€ badges.  
- Model slug prefixes (`nvidia/â€¦` vs wire id without first `nvidia/`).

---

## 8. Severity-ranked action list

### Must fix (P0)

1. **Deser** `null` vs `u32` for NVIDIA tool/stream payloads (Ultra/Super/Nano/GLM/MiniMax patterns).  
2. **Worktree lifecycle (ideal MVP):** before delete â€” `changes.patch` + `snapshot_ref` + `diffstat`; meta `worktree.state`; never leave dead paths as if live. Prefer **retain until land/discard**.  
3. **Subagent timeout** (default 10 min for NVIDIA); cancel always snapshots.  
4. **Structured completion + parent `land`/`diff`/`open`** so orchestrators can develop multi-agent features (see Â§3.5 and Â§3A).

### Should fix (P1)

5. Platform NVIDIA request_compat defaults (`prompt_cache_key`, etc.).  
6. Clamp `max_completion_tokens` to model max (Nano 9B).  
7. `max_parallel_tool_calls = 1` for Llama 3.1 70B on NVIDIA.  
8. Hide/badge EOL 410 and 404 models; **agent_ready** badges.  
9. Change default oracle off Ultra until tools work.  
10. Heartbeats + `error_class`; path allowlists; RO skip-sandbox.

### Nice to have (P2)

11. Real `git worktree add` (or document clone sandbox honestly).  
12. Stall detector (no progress); fan-out + slot queue.  
13. Pre-land test hooks; required final schema.  
14. Conformance CI: text + one tool per catalog model.  
15. UI matrix / session export report.  
16. Mid-session reload of model `request_compat` for subagents.

---

## 9. Session evidence index

| Subagent (short id) | Model | Status | Note |
|---------------------|-------|--------|------|
| 019fbed8â€¦992 | Ultra | failed | serialize RO hero |
| 019fbed8â€¦efd | Super | completed | hero OK |
| 019fbed8â€¦e6c | GPT-OSS 120 | completed | hero OK |
| 019fbed8â€¦1a9 | Nano 30B | completed | hero OK |
| 019fbedaâ€¦ec7 | GPT-OSS 120 tools | killed ~27m | stall |
| 019fbedaâ€¦b20 | Super tools | failed | serialize |
| 019fbedaâ€¦b92 | Nano tools | failed | serialize |
| 019fbedaâ€¦eb9 | GLM tools | failed | serialize after tools |
| 019fbee3â€¦33d | Grok hero+WT | completed | isolation OK |
| 019fbee3â€¦8a8 | Grok bug/code | completed | 10/10 + 9/9 |
| 019fbeedâ€¦d8fc | Terra hero+WT | completed | isolation OK |
| 019fbeedâ€¦aac5 | Terra bug/code | completed | 10/10 + 9/9 |
| 019fbef3â€¦c1 | Llama70 hero | weak | ignored prompt |
| 019fbef3â€¦c4 | Step3.7 hero | completed | OK |
| 019fbef3â€¦0d | GPT-OSS20 tools | partial | thrash |
| 019fbef3â€¦14 | Nano9b tools | failed | max tokens 400 |
| 019fbef4â€¦4cf | Llama3.3 hero | killed@10m | stall |
| 019fbef4â€¦296 | MiniMax hero | failed | serialize RO |
| 019fbef4â€¦ea8 | GPT-OSS120 short | killed@10m | stall |
| 019fbef4â€¦67d | Llama8 tools | partial | SMOKE only |
| 019fbefeâ€¦2f4 | Omni hero | completed | OK |
| 019fbefeâ€¦d5a | Step3.5 tools | failed | 410 EOL |
| 019fbefeâ€¦bf0 | GPT-OSS20 hero | completed | OK |
| 019fbefeâ€¦48d | Llama70 plan | completed | OK |
| 019fbeffâ€¦e2be | Step3.7 tools | completed | 390s; wrong bugs |
| 019fbeffâ€¦d603 | Llama70 tools | failed | single tool-call |
| 019fbeffâ€¦c279 | Ultra plan | completed | strong |
| 019fbeffâ€¦890d | Super plan | completed | OK |

---

## 10. Closing statement

Hyperâ€™s **subagent + isolation design is directionally right** (proven by Grok and Terra). The NVIDIA integration is **usable for chat/planning** on several models and **not yet trustworthy for tool-using development agents**, primarily due to:

1. fragile response deserialization,  
2. provider capability mismatches (tokens, parallel tools, EOL catalog),  
3. missing timeouts,  
4. worktree deletion without obvious recovery UX.

**Ideal worktree handling** (Â§3.5): isolate by default, never lose work, explicit create â†’ snapshot â†’ land/discard â†’ cleanup, structured return objects, real git worktrees when possible, RO skip-sandbox, concurrent path allowlists.

**Features that help supervisors develop** (Â§3A): land/diff/open tools, timeout + heartbeat, agent_ready flags, fan-out slots, pre-land tests, session export â€” so the parent can run real multi-agent feature pipelines instead of archaeology.

Fixing P0 items would unlock Ultra/Super/Nano as real coding agents; until then, product should **label agent-ready models honestly** and **make worktree artifacts impossible to lose**.

â€” End of report â€”
