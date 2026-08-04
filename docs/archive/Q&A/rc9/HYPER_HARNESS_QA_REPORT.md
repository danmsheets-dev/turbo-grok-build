# Hyper Agent Harness Q&A Report

**Date:** 2026-08-02  
**Hyper:** `0.2.114-r9` (`6e495353f`)  
**Session:** `019fc26f-2601-73f2-8226-136bef094f36`  
**Workspace:** `H:\Apps\testing` (Windows, git dirty, **project untrusted**)  
**Parent model:** Grok 4.5 (`XAI_API_KEY`)  
**Scope:** Credit-light probes of harness control over Grok 4.5 + NVIDIA Nemotron agents; improve workflows; **ignore free-tier API timeouts**.  
**Artifacts:** `results/harness-qa-20260802/` · ADL pack `results/harness-qa-20260802/adl-export/`  
**Companion:** `DEVELOPER_LOG_FEEDBACK.md` (second report)

---

## 1. TL;DR for Hyper engineers

| Pri | Finding | Impact |
|-----|---------|--------|
| **P0** | `capability_mode=read-only` **does not block `write`** | Isolation/safety control is advisory only; children can mutate parent when `isolation=none` |
| **P1** | `developer_log` tool **not exposed** to parent, child, or headless agents | Boot card mandates filing; agents literally cannot comply |
| **P1** | `hyper subagent land` fails on dirty parents; `--mode overwrite` applies **inflated dirty-tree snapshots** (untracked bulk under `worktrees/`, `.grok-restore/`) | Multi-agent land unusable on real dirty eval repos |
| **P1** | `hyper subagent diff` errors: worktree path **outside repository** | Primary recovery CLI broken for clone-style isolation trees |
| **P2** | `allowed_paths` does **not** block child writes (only intended for land/diff; not enforced on land either when land fails first) | Supervisors over-trust allowlist |
| **P2** | Boot card **Model field blank**; oracle pin to Nemotron Ultra marked **not agent-ready** by doctor | Misleading session briefing / weak default for deep analysis |
| **P2** | `developer-log.toml` pointed at `H:\Apps\grok build` (source tree), not a log root | ADL appeared empty; co-mingled with code |
| **P3** | Workflow `validate_only` rejects wrong `agent()` shape / name rules without pointing to skill | Friction for first-time workflow authors |

**What worked well:** parallel `spawn_subagent`, worktree isolation for Grok + Nemotron (nano/super/9b/omni), soft-preserve + `retain_worktree`, baseline/snapshot refs, `hyper worktree list`, kill/cancel, resume_from, explore read-only role, plan/oracle roles, agent-only `git diff baseline..snapshot`.

---

## 2. Probe matrix (this session)

| ID (short) | Type / model | Isolation | Result | Notes |
|------------|--------------|-----------|--------|-------|
| `…9b2a7375e2e1` | explore / grok | worktree | completed | Correctly refused write (no write tools) |
| `…9b388d25d35c` | GP / grok-4.5 | worktree + retain | completed | Wrote probe; land blocked (dirty) |
| `…9b4aed14e5be` | GP / grok | **none** | completed | Shared parent write OK |
| `…9b5542bfe74f` | GP / **nemotron-3-nano** | worktree + retain | completed | Tool use OK (~78s) |
| `…9b65b94ace5d` | GP / **nemotron-3-super** | worktree + retain | completed | Tool use OK (~44s) |
| `…cb7068ad57c8` | oracle / **ultra** (pin) | none | completed | Simple read OK (~7s) |
| `…cb8ae7a7733d` | plan / grok | none | completed | Solid one-file plan |
| `…0e3bd26830f5` | GP / grok | worktree + **allowed_paths** | completed | Wrote **inside and outside** allowlist |
| `…0e465ade0ee9` | GP / **nano-9b** | worktree + retain | completed | Tool use OK (~52s) |
| `…21dfa8ed4198` | resume of grok WT | worktree | completed | Resume works; appends on live tree |
| `…4e39106a8f18` | GP read-only mode | none | completed | **WRITE SUCCEEDED** (P0) |
| `…b467ec53c125` | kill target | none | **cancelled** | Kill tool works |
| `…c29e9f35befc` | GP / grok | none | completed | Child also **MISSING_DEVELOPER_LOG** |
| `…040163f9` | GP / **omni-reasoning** | worktree + retain | completed | Tool use OK (~13s) |

Headless (`hyper --prompt-file …`): reply **`MISSING_DEVELOPER_LOG`**.

---

## 3. What worked (keep / double down)

### 3.1 Subagent spawn + concurrency
- Five parallel children (Grok + Nemotron) launched cleanly; results returned via `get_command_or_subagent_output` wait_all.
- Meta includes `effective_model_id`, duration, tool_calls, worktree path, snapshot/baseline refs.

### 3.2 Worktree isolation correctness
- Worktree agents wrote under `~\.grok\worktrees\apps-testing\subagent-<id>\` only.
- Parent pollution for intentional isolation=none was clear and expected.
- **Soft preserve + `retain_worktree=true`** left trees live for inspection (`worktree_state: preserved`).
- Agent-only diff via refs works:
  ```text
  git diff refs/grok/subagent-baselines/<id> refs/grok/subagents/<id>
  ```

### 3.3 NVIDIA Nemotron (credit-light tool probes)
Ignoring free-tier timeouts (none hit this session): **nano-30b, super-120b, nano-9b, omni-reasoning** all completed tiny list_dir/write tasks. Harness **can** drive these models for simple tool loops today.

### 3.4 Role agents
- **explore:** no write tools (capability real at agent-type level).
- **plan / oracle:** structured useful outputs; oracle pin resolved to Ultra and still succeeded on a 1-tool read.

### 3.5 Lifecycle controls
- **kill_command_or_subagent** → `status: cancelled`.
- **discard** removes live tree, keeps snapshot_ref.
- **hyper worktree list** shows subagent trees (better than raw `git worktree list`, which only showed main + one restore).
- **resume_from** reuses conversation + worktree quickly.

### 3.6 Workflow validate path
- After fixing `agent(prompt, opts)` shape and hyphenated `meta.name`, `validate_only` smoke check passed (canned host path).

---

## 4. What failed / friction (improve)

### 4.1 P0 — `capability_mode` not enforced
**Repro:** `spawn_subagent(capability_mode="read-only", isolation="none")` → child called `write` → file `should_not_write.txt` created on parent.

**Expected:** write/edit/shell-mutate tools absent or denied.  
**Actual:** full write succeeded.  
**Note:** `explore` agent type *does* strip writes; capability_mode does not. Inconsistent security story.

**Fix:** Enforce mode at tool registration + dispatch; fail closed. Add integration test.

### 4.2 P1 — Land unusable on dirty parents
```text
merge land would conflict (nothing applied):
error: tasks/debugging/json_path.py: does not match index
```
Agent only added `results/.../probe_*.txt`. Unrelated dirty tracked file blocks entire land.

`--mode overwrite` then tries to apply a **huge** snapshot including pre-existing untracked `worktrees/**` and `.grok-restore/**` → hundreds of “already exists” errors. Boot card FOOTGUN is real and worse on eval repos.

**Fix ideas:**
1. Default land = **agent-only** (`baseline_ref..snapshot_ref`), not full tree vs dirty parent index.
2. Three-way merge only for overlapping paths.
3. Auto-exclude known bulk dirs (or only land `changed_paths` from meta).
4. Surface `changed_paths` + one-click “copy these files” in CLI/UI.
5. Align boot card: there is no `--force`; modes are `merge|overwrite`.

### 4.3 P1 — `hyper subagent diff` broken for isolation trees
```text
'…\subagent-…' is outside repository at 'H:/Apps/testing'
```
`open` already has correct recover recipes; **diff should use the same refs**.

### 4.4 P1 — developer_log invisible (see companion report)
Boot card + docs require it; parent/child/headless all report missing. Manual seed + index can make `hyper issues list` work, but agents cannot file.

### 4.5 P2 — allowed_paths semantics
Child freely wrote `scripts/allowed_paths_leak.txt` despite `allowed_paths=["results/harness-qa-20260802/"]`. Snapshot `changed_paths` includes both files. Docs say land/diff only — product should either enforce at write time **or** filter land/diff/changed_paths hard and say so in boot card.

### 4.6 P2 — Boot card / doctor
- Boot card `Model:` empty while default is grok-4.5.
- Doctor: Oracle pin Ultra not agent-ready — still used Ultra for oracle; worked for trivial read, risk for real investigation.
- Project **untrusted** — MCP/hooks/plugins may drop silently; harnesses should prefer explicit trust for worktrees.

### 4.7 P2 — Workflow authoring
- Underscores in `meta.name` rejected.
- Map-first `agent(#{...})` fails: need `agent(prompt, opts)`.
- Skill documents this; tool error messages should cite the skill/API one-liner.

### 4.8 Operational
- Shell note “`&&` not supported” is inaccurate for `cmd /c` on Windows; PowerShell uses `;`.
- Concurrent spawn of 4–5 worked; no stall this session.
- Soft-preserve keeps many worktrees on disk — good for recovery, needs prune UX (list is long: 70+ historical subagents).

### 4.9 Monitor tool + Windows quoting
`monitor` with a nested `powershell -Command "… Write-Output \"tick $_ …\""` exited **code 1 in ~3.7s with empty stdout**. Nested quote/`$_` stripping is a common PowerShell-through-PowerShell footgun. Agents will mis-author monitor commands often on Windows.

**Fix ideas:** document Windows-safe monitor examples; prefer `cmd /c` or a temp `.ps1` path; surface stderr from monitor failures in the tool result (empty output makes diagnosis hard).

---

## 5. Grok 4.5 vs Nemotron (harness control)

| Dimension | Grok 4.5 | Nemotron (nano/super/9b/omni) |
|-----------|----------|-------------------------------|
| Spawn via harness | Excellent | Excellent (when model set on spawn) |
| Tiny tool loops | Excellent | **Succeeded** this session |
| Role agents | explore/plan solid | Oracle pin Ultra did simple tools |
| Isolation | Worktree correct | Same |
| Land into dirty parent | Blocked (harness) | Same (land is parent-side) |
| Credit efficiency | Fast (~11–43s probes) | 13–78s for tiny probes |

**Conclusion:** For credit-light tool probes, harness control of NVIDIA models looks healthy today. Remaining blockers are **parent-side land/diff/capability/devlog**, not “Nemotron can’t tool.”

---

## 6. Recommended harness workflow improvements

1. **Land agent-only by default** (baseline→snapshot); optional “include dirty parent” advanced mode.
2. **Diff via refs**, never parent-repo path into external clone tree.
3. **Enforce capability_mode** like explore does for tools.
4. **Always register `developer_log`** when ADL enabled; doctor checks tool presence.
5. **allowed_paths:** write-time deny or land filter; document in boot card in one sentence.
6. **Boot card:** fill Model; link land modes; warn when parent dirty will block land.
7. **Trust:** headless/harness flag `--require-trust` + auto-trust for created worktrees.
8. **Supervisor helpers:** `hyper subagent extract <id> --to <dir>` copying only `changed_paths`.
9. **Prune UX:** filter `subagent list` by session / land_status pending; warn when >N preserved trees.
10. **Workflow validate errors:** print canonical `agent("…", #{…})` example on Function not found.

---

## 7. Recovery recipes that *do* work today

```bash
# Meta + paths
hyper subagent open <id>

# Agent-only changes
git diff refs/grok/subagent-baselines/<id> refs/grok/subagents/<id>
git show refs/grok/subagents/<id>:<path>

# Extract single file to parent
git show refs/grok/subagents/<id>:results/foo.txt > results/foo.txt

# Full restore
hyper subagent open <id> --restore

# Discard live tree keep snapshot
hyper subagent discard <id>
```

Manual extract used successfully for nano/super/9b probes when land failed.

---

## 8. ADL filing this session

| Source | Result |
|--------|--------|
| `developer_log` tool | **Unavailable** (parent/child/headless) |
| Manual incident JSON + index | **`hyper issues list` shows 6** after schema fixes |
| `hyper issues export` | Pack at `results/harness-qa-20260802/adl-export/` (6 incidents) |

See companion report for developer logging UX.

---

## 9. Test inventory (harness surfaces touched)

- [x] Boot card presence / contents  
- [x] Shell + file tools (parent)  
- [x] spawn_subagent: explore, plan, oracle, general-purpose  
- [x] isolation worktree / none / retain_worktree  
- [x] allowed_paths  
- [x] capability_mode (failed enforcement)  
- [x] resume_from  
- [x] kill/cancel  
- [x] monitor (short stream) — **failed** exit 1, empty output (Windows PowerShell nested-quote command; see §4.9)  
- [x] workflow validate_only  
- [x] land merge / overwrite  
- [x] diff CLI (failed)  
- [x] open / discard / worktree list / sessions list  
- [x] doctor / inspect / models  
- [x] issues path / set-dir / list / show / export  
- [x] NVIDIA nano, super, 9b, omni, ultra-oracle  
- [x] Headless prompt-file  
- [ ] Full live workflow run (validate only)  
- [ ] scheduler_create (not needed for core harness)  
- [ ] MCP deep (several servers timed out at session start)  

---

## 10. Bottom line

The Hyper harness **successfully steers Grok 4.5 and Nemotron** on small isolated tasks with good meta, snapshots, and soft-preserve recovery. The highest-value fixes are **parent-side**: enforce capability modes, make land/diff agent-only-first, and **actually expose `developer_log`** so the boot card’s mandatory feedback loop can close.

---

## 11. Phase 2 expansion (continued testing)

User correctly noted phase 1 was a dense first pass, not exhaustive. Phase 2 added:

### 11.1 Additional results matrix

| Test | Result |
|------|--------|
| `timeout_ms=15000` long sleep child | **Cancelled** at budget (~15.05s), clear message |
| Invalid model slug on spawn | **Fail closed** with valid slug list (excellent) |
| `nvidia/.../nemotron-3-ultra` as **GP** write | **Success** (~29s) — Ultra can do simple tools |
| Live **workflow** `harness-qa-live` | **complete** ~2s, result `WORKFLOW_OK`, 1 agent |
| `--require-changes` + `write` tool | **BUG**: file created, but `stopReason=NoChanges`, `filesChanged.count=0`, exit 1 |
| `--require-subagent-success` (no children) | exit 0 (vacuous pass) |
| `--max-turns 1` | works (`HI`) |
| `hyper subagent prune --older-than 1h` dry-run | lists 73 old dirs (safe default) |
| `hyper sessions search harness` | works |
| `hyper export <session>` | MD export works (~41KB; some mojibake on Windows) |
| `capability_mode=execute` shell | **works** (`shell-ok`) |
| `capability_mode=read-write` write | **works** |
| `capability_mode=read-only` write | **still broken** (phase 1) |
| `cwd` + `isolation=worktree` | **reject** mutual exclusive (good error) |
| `cwd` + `isolation=none` into mini-repo | **works** (child-ok) |
| Land on dirty parent | still **blocked** |
| Nested headless spawn→land in clean repo | **max turns** — agent thrashed invoking spawn_subagent |
| **6 concurrent** explore agents | all completed ~4–6s (queue OK) |
| `monitor` via `.ps1` file | preferred over nested quotes |
| `--confine` | **blocks** outside path (`path-outside-root`); inside write OK |
| `--no-subagents` | removes `spawn_subagent`; leaves kill/get/workflow |
| `scheduler_create` 60s fire_immediately | fires loop children; **one snapshot showed 507 files** dirty inflation; cancelled after probe |
| MCP `tasks__list` | empty `{}` (no automations) |
| Session `plugin list` | many plugins installed |

### 11.2 New P1: `--require-changes` false negative

Streaming-json end event from a run that **did** create `require_changes_marker.txt`:

```json
"stopReason": "NoChanges",
"filesChanged": { "count": 0, "paths": [] },
"toolCalls": 1
```

File on disk: `marker-1`. **Harnesses relying on `--require-changes` will false-fail successful write-tool runs.** Likely only tracks certain edit paths, not `write` creates.

### 11.3 New P1: scheduler + dirty tree snapshots

Scheduler loop subagent meta showed `diffstat: 507 files, +62303/-67` including `.grok-restore/**` and historical `worktrees/**` despite prompt “do not use tools.” Confirms snapshot baseline is “dirty parent copy,” not agent-only edits — catastrophic for scheduled tasks on eval repos.

### 11.4 Headless orchestration fragility

Clean mini-repo headless agent with 12 turns failed to successfully call `spawn_subagent` (max turns). Tool may be present but call protocol / discovery is hard without interactive tool schemas. Document headless spawn examples.

### 11.5 Still not fully covered (honest backlog)

- Hooks PreToolUse/PostToolUse  
- Plan mode enter/exit UX  
- Memory experimental  
- `hyper agent stdio|serve|leader`  
- Trace upload  
- MCP depth (gitnexus/godot timeouts)  
- Land **success** path on a truly clean git parent with worktree child (dirty-tree blocked all lands here; clean nested headless failed to spawn)  
- Multi-phase live workflow with parallel()  
- Permission modes (`dontAsk`, sandbox profiles)  
- Image / video tools  
- Oracle hard investigation (only trivial read)  

---

*Phase 1 + phase 2 live harness QA, 2026-08-02.*
