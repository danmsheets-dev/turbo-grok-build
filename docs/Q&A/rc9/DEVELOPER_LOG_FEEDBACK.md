# Developer Logging Feedback (Companion Report)

**Date:** 2026-08-02  
**Hyper:** `0.2.114-r9`  
**Session:** `019fc26f-2601-73f2-8226-136bef094f36`  
**In-app pack:** `results/harness-qa-20260802/adl-export/`  
**Log root (after fix):** `C:\Users\dan_m\.grok\developer-log`  
**Primary harness report:** `HYPER_HARNESS_QA_REPORT.md`

This is the **second report** requested alongside the in-app Auto Developer Log (ADL) export: human evaluation of how well developer logging works as a product.

---

## 1. Executive scorecard

| Area | Score (1–5) | Notes |
|------|-------------|-------|
| Design intent (docs + boot card) | **5** | Clear taxonomy, export pack, dedup story |
| Tool availability to agents | **1** | Tool missing in parent, child, headless |
| CLI operator UX (`hyper issues *`) | **4** | list/show/export/path/set-dir solid once data exists |
| Default configuration safety | **2** | set-dir pointed at source repo; empty store looked “healthy” |
| Schema robustness for manual/edge writes | **3** | Strict enums; list can show index while show/export skip bad bodies |
| End-to-end “agent hits bug → maintainer pack” | **1** | Broken without tool; manual seed works with care |
| **Overall product fitness today** | **2 / 5** | Great design, incomplete wiring in this session |

---

## 2. What the product promises

From boot card + `docs/AUTO_DEVELOPER_LOG.md`:

1. Agents **must** call `developer_log` on product friction.
2. Store under configurable root with fingerprint dedup + redaction.
3. Humans review via `hyper issues list|show|export|path`.
4. Runtime auto-detectors for worktree dispose, isolation fallback, stalls.

That loop is the right architecture for field signal without chat archaeology.

---

## 3. What we observed live

### 3.1 Tool not in agent tool surface (blocker)

| Context | Result |
|---------|--------|
| Interactive parent (this session) | No `developer_log` in available tools; `search_tool` empty |
| Headless `hyper --prompt-file` | Model replied **`MISSING_DEVELOPER_LOG`** |
| Subagent general-purpose | **`MISSING_DEVELOPER_LOG`** (listed other tools) |

Boot card still **requires** the tool and tells agents not to skip it. That creates false compliance pressure and silent data loss: agents either invent workarounds or give up.

**Prior session title in history:** “R9 Headless Smoke developer_log Incident Filing” — strongly suggests this has been under test already.

### 3.2 Misconfigured log root

```text
hyper issues path  →  H:\Apps\grok build   (via developer-log.toml)
```

That path is the **product source tree**, not a log directory. Effects:

- No `incidents/` / `index.json` layout.
- `hyper issues list` → “No open incidents” (looked fine, was wrong place).
- Risk of co-mingling logs with code or confusing `set-dir` users.

After `hyper issues set-dir C:\Users\dan_m\.grok\developer-log`, path resolution was correct.

**UX fix:** `set-dir` should refuse non-empty unrelated trees or always use `<dir>/developer-log` subdirectory; `doctor` should warn when log root is a git repo with crates/src.

### 3.3 Manual seed path (what we had to do)

Because the tool was missing, we wrote incident JSON + `events.jsonl` + `index.json` by hand.

| Step | Result |
|------|--------|
| Write incident files only | `issues list` still empty (needs **index**) |
| UTF-8 **with BOM** (PowerShell `Set-Content`) | `json error: expected value at line 1 column 1` |
| Index without full incident schema | list works; show fails (`missing field reporter`) |
| Wrong `source.reporter` variant | show fails enum error |
| Fixed `source.reporter=human` | **show + export 6 incidents** |

**Positive:** once schema-valid, CLI list/show/export is excellent. Summary.md in export pack is maintainer-friendly.

**Negative:** there is **no** `hyper issues file|create` for humans/agents when the tool is down — only the tool path. Operators cannot recover without knowing the store schema.

### 3.4 Export pack quality

After fixes:

```text
Exported 6 incident(s) to …/adl-export
  summary.md · incidents.ndjson · manifest.json · evidence/
```

Summary severity table matched index (P0=1, P1=2, P2=3). Encoding glitch in summary title (`–` → ``) is minor Windows console/export cosmetic.

Interesting bug: when incident **bodies** failed to deserialize, export reported **“Exported 0 incident(s)”** but summary still listed them from the **index**. Maintainer would open empty ndjson while summary claims 6 — trust gap.

### 3.5 Runtime auto-detectors

Not heavily exercised this session (no dispose-without-snapshot on our retain_worktree probes). Land/diff failures did **not** auto-file incidents — expected if only agent/tool/detectors write, and agent cannot.

---

## 4. Incidents filed (in-app after manual seed)

| Sev | Class | Title |
|-----|-------|-------|
| P0 | `tool_schema` | capability_mode=read-only does not block write tools |
| P1 | `land_conflict` | subagent land fails closed on dirty parent unrelated paths |
| P1 | `feature_gap` | developer_log tool not exposed to parent agent session |
| P2 | `feature_gap` | allowed_paths does not restrict child writes |
| P2 | `docs_gap` | developer-log.toml set-dir pointed at source tree not log root |
| P2 | `feature_gap` | hyper subagent diff fails for clone-style isolation worktrees |

Full export: `results/harness-qa-20260802/adl-export/`.

---

## 5. Feedback: what works well in ADL design

1. **Stable error_class taxonomy** — good for dedup and triage dashboards.  
2. **Fingerprint + occurrence_count** — right model for field spam.  
3. **Local-only store + explicit export** — privacy-appropriate.  
4. **CLI surface** (`list/show/export/ack/resolve/path/set-dir`) — complete enough for maintainers.  
5. **Boot card education** — if the tool existed, agents would know when/how to file.  
6. **Separation from `/feedback` sentiment** — correct product split.

---

## 6. Feedback: what to improve

### P0 / P1 product gaps
1. **Register `developer_log` in every agent tool schema** when ADL enabled (parent, child, headless, roles that can write logs).  
2. **Doctor check:** “ADL enabled but tool not in registry” → fail/warn.  
3. **Headless smoke** in CI: agent turn must be able to call developer_log successfully.

### Operator UX
4. Add **`hyper issues file`** (or `report`) CLI with same fields as the tool — human + script fallback.  
5. **set-dir validation:** create layout; warn if target looks like an application repo.  
6. Export should **not** claim incidents in summary if bodies fail to load; or fail the export non-zero with clear “N index entries unreadable”.  
7. Document **minimum valid Incident JSON** for emergency manual filing.  
8. Avoid UTF-8 BOM footguns in any sample scripts (document PowerShell `[UTF8Encoding]::new($false)`).

### Agent UX
9. If tool call fails (disabled/missing), return a structured error with `hyper issues path` and enablement env vars — not silence.  
10. Subagent boot blurb currently says “call developer_log” even when tool absent — gate the reminder on tool registration.  
11. Optional: parent auto-file on subagent `failed` with `error_class` from meta (runtime detector).

### Schema
12. Publish JSON Schema for Incident / IndexEntry next to AUTO_DEVELOPER_LOG.md.  
13. Softer parsing for `source` (default reporter) so partial files still export.

---

## 7. How well developer logging “works” for this harness QA

| Question | Answer |
|----------|--------|
| Did the boot card make us try to use it? | **Yes** — clear and forceful |
| Could we file from the agent? | **No** |
| Could we list/export after manual seed? | **Yes** (after schema/BOM fixes) |
| Would maintainers get signal from a normal agent session? | **No** — zero tool-originated incidents |
| Is the export pack good once data exists? | **Yes** |
| Net for “close the loop on product friction”? | **Not yet** — design > implementation in this build |

---

## 8. Recommended acceptance tests for rN

1. Interactive session: agent calls `developer_log` → `issues list` shows new open incident.  
2. Headless: same.  
3. Subagent: same (or explicit policy that only parent files).  
4. `GROK_DEVELOPER_LOG=0` → tool returns disabled cleanly.  
5. Bad set-dir → doctor warning.  
6. Export with corrupt body → non-zero exit + clear error.  
7. Dedup: second identical class+components increments occurrence_count.

---

## 9. Bottom line

Auto Developer Log is **well designed and almost operator-ready**, but in this Hyper session the **agent write path is missing**, so the mandatory boot-card workflow cannot run. CLI review/export is good once incidents exist. Until `developer_log` is always registered (plus a human CLI fallback), field signal will keep dying in chat transcripts — exactly what ADL was built to avoid.

---

## 10. Phase 2 note

Additional incidents seeded manually (tool still missing):

- `--require-changes` ignores `write` creates (P1)
- Scheduler snapshots dirty parent bulk (P1)
- `--no-subagents` partial tool strip (P3)

Re-export: `results/harness-qa-20260802/adl-export/` (9 incidents when index healthy).

**Unchanged blocker:** agents still cannot call `developer_log` natively.

*Companion to harness QA; manual seed used only because the in-app tool was unavailable.*
