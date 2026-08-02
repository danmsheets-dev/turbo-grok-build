# Hyper Developer Feedback Log

**Date:** 2026-08-02  
**Reporter:** End-to-end agent testing on Windows (`H:\Apps\testing`)  
**Binary under test:** `hyper 0.2.114-r9` (`6e495353f`) — `C:\Users\dan_m\.hyper\bin\hyper.exe`  
**Also exercised:** r8 (`0.2.114-r8`) worktree baseline; NVIDIA Integrate models  

**Related artifacts (testing workspace):**
- `H:\Apps\testing\results\HYPER_R9_RETEST_20260802.md` — full r9 retest
- `H:\Apps\testing\results\HYPER_WORKTREE_TEST_20260802.md` — worktree isolation (r8 era)
- `H:\Apps\testing\results\HYPER_RETEST_20260802.md` — Ultra + Minimax chat/tools
- `H:\Apps\testing\results\HYPER-DEV-REPORT-nemotron-ultra-subagents.md` — Ultra tool deser (pre-fix era)
- `H:\Apps\grok build\hyper-grok-build\docs\HYPER_DEVELOPER_FEEDBACK.md` — earlier 2026-08-01 feedback (this file supersedes for r9)

---

## 1. TL;DR for Hyper engineers

### What got better in r9 (keep)

| Area | Status |
|------|--------|
| Isolated subagent worktrees | **Solid** for Grok + Ultra |
| Agent-only baseline diffs | **Fixed** dirty-parent 235-file patches |
| `hyper subagent open/diff/land` | Usable recovery CLI |
| `open --restore` | One-command full tree restore |
| `worktree_state: preserved` + live path | Findable after complete |
| Parent write isolation | No pollution from child probes |
| Boot card injection on **new** sessions | Present in system prompt |
| Workflow multi-model (`model:` on `agent()` / `parallel`) | **Works** (Grok + Super + Ultra launched) |
| Fixed deep-audit Rhai (after trim fix) | Full investigate → verify → report |
| `--debug-file` | High-volume maintainer logs work |

### What is still broken / incomplete (fix)

| Pri | Issue | Impact |
|-----|--------|--------|
| **P0** | Built-in `/deepaudit` crashes on start: Rhai `trimmed()` returns `()` then `.to_lower()` fails | Slash command unusable out of the box |
| **P0** | `developer_log` tool **not exposed** to agents (parent headless or subagent) | Auto Developer Log cannot be filed; boot card **requires** it → contradiction |
| **P1** | Stock `/deepaudit` is single-model (session inherit); no multi-model pins | Cannot run Grok+Super+Ultra audit without custom workflow |
| **P1** | NVIDIA Super/Ultra finders **stall 180s** under workflow stall watchdog (nonzero tokens, no progress) | Multi-model audits degrade to Grok-only |
| **P1** | Boot card only on **new** sessions; long-lived/resume chats lack it | Agents in continued sessions miss recovery docs |
| **P1** | Headless `/deepaudit` exits immediately after start; no durable leader for background workflow | Unreliable harness launches |
| **P2** | Config key `ui.auto_interject_on_task_wait` rejected as unknown | Noise in debug logs |
| **P2** | Doctor still flags Ultra as not `agent_ready` while Ultra tool+worktree smokes pass | Misleading oracle pin guidance |
| **P2** | Project workflows need folder trust; filename must match `meta.name` | Friction for repo-local workflows |

---

## 2. Environment

| Item | Value |
|------|--------|
| OS | Windows |
| Test workspace | `H:\Apps\testing` (intentionally dirty git tree) |
| Hyper version | `0.2.114-r9` (`6e495353f`) |
| Parent session (main retest) | `019fc1d8-e387-75f0-8152-043f7bbd0f46` |
| NVIDIA route | Integrate (`platform/nvidia`, `integrate.api.nvidia.com`) |
| Models hit | `grok-4.5`, `nvidia/nvidia/nemotron-3-ultra-550b-a55b`, `nvidia/nvidia/nemotron-3-super-120b-a12b`, `nvidia/minimaxai/minimax-m3` |

---

## 3. Subagent isolated worktrees (r9)

### 3.1 Results

| Model | ID | Dur | Parent polluted | `worktree_state` |
|-------|-----|-----|-----------------|------------------|
| Grok 4.5 | `019fc242-776c-7541-9fdb-f159dfa721b6` | ~18s | **No** | **preserved** |
| Ultra 550B | `019fc242-776c-7541-9fdb-f163be736b62` | ~38s | **No** | **preserved** |

Live path pattern:

```text
C:\Users\dan_m\.grok\worktrees\apps-testing\subagent-<uuid>
```

### 3.2 Recovery surface (product wins)

`hyper subagent open <id>` now surfaces:

- `worktree_state` / `land_status`
- `baseline_ref` → `refs/grok/subagent-baselines/<id>`
- `snapshot_ref` → `refs/grok/subagents/<id>`
- **Agent-only diff:**  
  `git diff refs/grok/subagent-baselines/<id> refs/grok/subagents/<id>`
- `diffstat` / `changed_paths` (agent-authored only)
- `patch_path`
- **`hyper subagent open <id> --restore`** → `.grok-restore/<id>`

**Evidence:** 1-line probe on a dirty parent → `diffstat: 1 files, +1/-0` (was **235 files / ~1 MB patch** on r8 when diffing vs HEAD only).

### 3.3 Residual worktree notes

- Still clone-style sandboxes (not always listed in `git worktree list` while Hyper-managed).
- `hyper worktree list` needs `HOME`/`GROK_HOME` on Windows PowerShell or fails.
- Preserve-by-default is good for supervisors; document prune/land/discard lifecycle clearly in boot card (partially done).

### 3.4 Recommendation

Treat worktree isolation as **done for the RC7 “hard to find” complaint**. Remaining work is polish (UI for pending lands, agent completion payload always includes `snapshot_ref` + agent-only diffstat).

---

## 4. Boot card

### 4.1 What works

- New sessions inject `<hyper_boot_card version="1" mode="short">…</hyper_boot_card>` into system prompt.
- Content includes worktree recovery, `open --restore`, land/diff, and developer_log policy.
- Confirmed on disk for headless session `019fc243-8b31-7982-92a1-d2afa8c86045`.

### 4.2 Gaps

| Gap | Detail |
|-----|--------|
| Resume / long sessions | Boot card **not** re-injected; continued chats (this testing session) have **no** boot card |
| Contradictory instruction | Card **requires** `developer_log` but tool often **missing** from tool list |
| Visibility | Not a chat message (correct); hard for humans to know it fired without `system_prompt.txt` / inspect |

### 4.3 Spec already drafted

Agent-facing boot card product spec was authored in-session (short/full/off, ≤900 tokens short, subagent child stub, JIT completion hints). Recommend landing that as the source of truth under `docs/` if not already.

---

## 5. `/deepaudit` (Ultracode-style workflow)

### 5.1 Built-in slash — FAIL

```text
hyper -p "/deepaudit --size small tasks" -m grok-4.5 --always-approve
```

- Workflow `wf_019fc243087874828853b343c2d5c117` status **`failed`** in ~3 ms, `agents_used: 0`
- Error:

```text
Function not found: to_lower (()) (line 30, position 26)
in call to function 'normalize_focus' (line 58, position 21)
```

**Root cause (confirmed):**

```rhai
// BROKEN (shipped deep-audit)
fn trimmed(s) {
    if type_of(s) == "string" {
        return s.trim();  // mutator returns () in Rhai
    } else {
        ""
    }
}
// then: trimmed(raw).to_lower()  → to_lower(())
```

Documented pitfall in create-workflow skill; **stock deepaudit still ships the bug**.

**Fix:**

```rhai
fn trimmed(s) {
    if type_of(s) == "string" {
        s.trim();
        s
    } else {
        ""
    }
}
```

Verified: `~\.grok\workflows\deep-audit-fixed.rhai` completed successfully:

- Run `wf_019fc2465b817970b9e72bc215e7d105`
- ~3m 28s, 6 agents, **6/6 findings confirmed** on `H:\Apps\testing\tasks`

### 5.2 Multi-model support

| Question | Answer |
|----------|--------|
| Does stock `/deepaudit` multi-model? | **No** — children inherit session model |
| Can workflows multi-model? | **Yes** — `model:` on `agent()` / `parallel()` jobs |

Custom smoke `multi-model-audit-smoke.rhai`:

| Label | Model | Outcome |
|-------|-------|---------|
| find:grok | `grok-4.5` | **done**, 3 claims |
| find:super | Super 120B | **failed**, stall 180s, tokens≈50k |
| find:ultra | Ultra 550B | **failed**, stall 180s, tokens≈20k |
| verify ×3 | Grok | all 3 claims **confirmed** |

Run: `wf_019fc245b9617d938fafc95a2da9f351` (~3m 26s).

**Product ask:** `/deepaudit --models grok-4.5,nvidia/.../super,nvidia/.../ultra` or workflow args for finder/verifier model lists. Plumbing already exists.

### 5.3 Headless slash lifecycle

Headless `/deepaudit` returns `EndTurn` after “started in background”; process exits; `hyper leader list` empty. Long-lived parent sessions can host workflows to completion. Prefer: block until done in headless, or detach under durable leader.

---

## 6. Auto Developer Log

### 6.1 CLI — OK

```text
hyper issues path|list|show|export|resolve|ack|set-dir|clear-dir
→ root: ~/.grok/developer-log (enabled)
```

### 6.2 Agent tool — FAIL

| Surface | Result |
|---------|--------|
| Subagent tools | `developer_log` **missing** |
| Headless parent + boot card | Agent cannot invoke tool; max turns / cancelled |
| After attempt | No incidents; **developer-log dir never created** |

Binary contains `tool.developer_log` / `DeveloperLogInput` — implementation exists; **registration / toolset exposure** is the bug.

**P0:** Expose tool wherever boot card mandates filing, or stop requiring it until ready.

---

## 7. Debug logging

| Flag | Result |
|------|--------|
| `--debug` / `--debug-file <path>` | Works |
| Sample sizes | 2.9–3.3 MB for short headless runs |
| Content | startup timings, catalog inject, ACP prompts, roles/personas |

Also observed:

```text
config: ignored unrecognized ... path=ui.auto_interject_on_task_wait
```

---

## 8. NVIDIA / model notes (still relevant)

| Topic | Status on r9 |
|-------|----------------|
| Ultra chat + tools + worktree isolation | **PASS** (tool deser crash from earlier reports not reproduced in these smokes) |
| Ultra as workflow finder under 180s stall | **FAIL** (stall cancel) |
| Super same | **FAIL** (stall) |
| Minimax (earlier r8 session) | Chat/tools OK when not 429; concurrent bursts 429 |
| Oracle pin to Ultra | Doctor warns not `agent_ready` — reconcile with tool success |
| Strict Integrate fields | `supports_prompt_cache_key = false` still required |

Earlier P0 “null expected u32” on Ultra tool loop: **not seen** in r9 worktree/tool smokes; treat as fixed or rare — keep a regression test.

---

## 9. Prioritized fix list

### P0 — ship blockers

1. **Fix deep-audit `trimmed()`** so `/deepaudit` starts.  
2. **Wire `developer_log` into agent tool lists** (or gate boot card text).  
3. Unit test Rhai helpers for trim/to_lower; smoke `/deepaudit --size small` in CI.

### P1 — product completeness

4. Multi-model args for deepaudit finders/verifiers.  
5. Investigate Super/Ultra **workflow stall** (progress definition: tokens alone insufficient?).  
6. Boot card on resume optional; document new-session-only.  
7. Durable headless workflow host / wait-for-complete flag.  
8. Project workflow trust UX (clear error + how to trust).

### P2 — polish

9. Config schema for `ui.auto_interject_on_task_wait` or remove.  
10. Reconcile `agent_ready` flags with live Ultra tool success.  
11. Debug log levels / size caps for harness use.  
12. TUI: pending land list with open/diff/land/discard.

---

## 10. Evidence index

| Run / artifact | ID or path |
|----------------|------------|
| Grok worktree | `019fc242-776c-7541-9fdb-f159dfa721b6` |
| Ultra worktree | `019fc242-776c-7541-9fdb-f163be736b62` |
| Built-in deepaudit fail | `wf_019fc243087874828853b343c2d5c117` |
| Multi-model audit complete | `wf_019fc245b9617d938fafc95a2da9f351` |
| Fixed deep-audit complete | `wf_019fc2465b817970b9e72bc215e7d105` |
| Fixed script (user) | `%USERPROFILE%\.grok\workflows\deep-audit-fixed.rhai` |
| Multi-model script (user) | `%USERPROFILE%\.grok\workflows\multi-model-audit-smoke.rhai` |
| Full retest report | `H:\Apps\testing\results\HYPER_R9_RETEST_20260802.md` |
| Debug logs | `H:\Apps\testing\results\runs\*-debug.log` |

---

## 11. Verdict

**Worktrees and recovery:** r9 substantially delivers on the RC7 feedback (find, baseline-diff, restore, preserve).  

**Deepaudit:** architecture and fixed script are good; **stock slash is dead on arrival** from a one-line Rhai trim bug. Multi-model is available at the workflow API, not as stock UX.  

**Developer Log:** CLI shell is ready; **agent filing path is not**, which breaks the boot card contract.

Ship P0 items before marketing deepaudit + Auto Developer Log as ready.
