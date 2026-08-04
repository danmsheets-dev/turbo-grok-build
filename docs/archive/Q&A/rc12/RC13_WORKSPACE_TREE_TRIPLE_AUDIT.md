# Workspace Tree — Triple Independent Audit (RC13)

| Item | Content |
|------|---------|
| Feature | **Workspace Tree** (directory atlas / Tree Logging) — not subagent git isolation worktrees |
| Date | 2026-08-03 |
| Tree | `H:\Apps\grok build\hyper-grok-build` (RC13 source land) |
| Auditors | Grok 4.5 · GPT-5.6-sol · Nemotron Ultra (2nd attempt; 1st timed out) |
| Mode | Independent explore, read-only |

---

## 1. Scorecard

| Auditor | Score | One-line |
|---------|-------|----------|
| **Grok 4.5** | **6.5 / 10** | Real Phase‑1 surface; uneven quality; ship-with-notes |
| **GPT-5.6-sol** | **4 / 10** | Core credible but **not production MVP**; compile blocker + Windows refresh + collapse indexing |
| **Nemotron Ultra** | **7.5 / 10** | Strong foundation/architecture; freshness + truncation + isolation edges weak |
| **Synthesized** | **~5.5–6 / 10** | Landed skeleton is right; **must-fix bugs block “MVP done” claim** |

---

## 2. Consensus: what works well

Agreement across all three:

1. **Crate boundary is clean** — walk / store / query / inject / tools / UI separated.  
2. **Non-blocking kickoff** on trusted open is the right session model.  
3. **Tools use `resolve_cwd`** (SharedResources fallback) — fixes the RC12 “requires session Cwd” class.  
4. **Surfaces exist** — tools, inject path, `/tree`, `turbo tree`, fixture tests.  
5. **Default / explore / plan / orchestrator** expose atlas tools.  
6. **Miss-recovery design** (when enabled) avoids building on the hot error path.

---

## 3. Completeness (design Phase 1 → reality)

| Design item | Consensus |
|-------------|-----------|
| Walker + gitignore + hard excludes | **Shipped** |
| Collapse rules | **Partial** (works; over-collapses large source dirs; sample paths wrong) |
| Durable store | **Partial** (JSON only; weak Windows replace; no lock/generation) |
| Async trusted kickoff | **Shipped** |
| Session inject card | **Broken / partial** — wiring present; **`tool_working_directory` missing on `PromptContext`** |
| `workspace_tree` / `resolve_path` tools | **Partial** (core actions yes; filters incomplete) |
| Miss suggestions | **Partial / off by default** |
| `/tree` + `turbo tree` | **Partial** (no search CLI; refresh cache bug; process CWD) |
| Config (TOML cascade) | **Mostly missing** (env only) |
| Honest freshness | **Missing** (always `Fresh`) |
| Worktree base+overlay | **Missing** (Phase 2) |
| Single-writer / generation | **Missing** |

---

## 4. Consensus bugs (merged severity)

### P0 — fix before calling RC13 tree “done”

| # | Defect | Who raised | Evidence / impact |
|---|--------|------------|-------------------|
| **F1** | **Compile break: inject reads nonexistent `PromptContext.tool_working_directory`** | GPT Sol (confirmed orchestrator grep: field does not exist; only `working_directory`) | Agent crate likely fails to build; inject broken for isolation CWD |
| **F2** | **Windows `write_atomically` = rename over existing file** | Grok + GPT Sol | Second `save`/`refresh` can fail on Windows; rebuild unreliable |
| **F3** | **CLI / slash rebuild does not update process cache** | Grok | `build_and_save` then `get_or_load` returns **stale Arc**; tools keep old atlas after “refresh” |
| **F4** | **Freshness always `Fresh` forever** | All three | Agents trust lying atlas after renames/commits until restart |

### P1 — high product impact

| # | Defect | Who raised | Notes |
|---|--------|------------|-------|
| **F5** | **Miss hints default OFF** (`path_not_found_hints: false`) | Grok | Docs advertise Did-you-mean; default local sessions never get it |
| **F6** | **Collapse destroys searchable inventory** | GPT Sol (+ Grok samples) | Name index built **after** collapse; large dirs (e.g. monorepo `crates/`) become nearly unresolvable |
| **F7** | **Collapsed samples invent paths** | Grok + GPT Sol | Basename-only samples re-joined as `collapsed/basename` → false paths |
| **F8** | **No single-writer / dual full walks** | All | Kickoff + first tool race; last writer wins |
| **F9** | **Trust gate only on kickoff** | Grok + GPT Sol | Tools can walk/write store for untrusted CWDs |
| **F10** | **Inject `truncate` can panic on UTF-8 boundary** | Grok + GPT Sol | Prompt build crash under budget |
| **F11** | **`/tree` uses process CWD**, not session tool CWD | All | Wrong root after chdir / multi-root |
| **F12** | **Symlink follow without cycle guard** | Nemotron | Potential hang/OOM |

### P2 — polish / correctness

| # | Defect |
|---|--------|
| F13 | Case-sensitive `find_node` on Windows |
| F14 | Search collects from HashMap then truncates → nondeterministic top-N |
| F15 | `ignored_dirs` never incremented; silent walk error drop |
| F16 | Mojibake in inject strings (`Â·` / `â€¦`) |
| F17 | Concise/hashline toolsets omit atlas tools while boot card advertises them |
| F18 | No store prune / size cap → disk growth |
| F19 | Linked worktree git HEAD/common_dir incomplete |
| F20 | Inject-preview can show “building…” when mode is `off` |

---

## 5. Suggested fixes (prioritized backlog)

### Must-do (next patch / RC13.1)

1. **F1 — Wire real tool CWD into inject**  
   - Add `tool_working_directory: Option<String>` on `PromptContext` (or stop reading it and use a field that shell already fills for isolation).  
   - Populate from session tool CWD / worktree path for subagents.  
   - Unit-test struct construction + inject root for isolation.

2. **F2 — Windows-safe atomic replace**  
   - On Windows: remove dest or use replace API before rename; keep tmp + fsync.  
   - Test: save → save again → refresh twice on Windows.

3. **F3 — Rebuild → cache insert**  
   - CLI + slash must call `workspace_tree_cache::refresh` (or insert built index), never bare `get_or_load` after rebuild.

4. **F4 — Honest freshness (minimal)**  
   - On load: compare stored git HEAD (if any) to current; if mismatch → `stale` or auto-rebuild.  
   - Never claim `fresh` for an unchecked durable snapshot.

5. **F5 — Default-on miss hints**  
   - `path_not_found_hints: true` by default (remote settings can still override).

### Should-do (same RC or next)

6. **F6/F7 — Index full paths, collapse only for display**  
   - Build `name_index` from **pre-collapse** inventory (or compact path list).  
   - Samples store full rel-paths under collapsed nodes, not basenames alone.  
   - Source-aware collapse: do not collapse `src/`, `scripts/`, `crates/*` package roots solely for file count.

7. **F8 — Singleflight per workspace_id**  
   - In-process mutex + shared build future; optional lockfile for CLI/session races.

8. **F9 — Trust check in tool handlers**  
   - Same `project_scope_allowed` as kickoff before `get_or_load` walk.

9. **F10 — Safe inject budget**  
   - Truncate at UTF-8 char boundary; reserve wrapper tag size; prefer project token estimator.

10. **F11 — Session CWD for slash/CLI**  
    - Pass agent session CWD into slash exec context; default CLI `--root` to session/workspace.

11. **F12 — Symlink cycles**  
    - Disable follow by default **or** track visited inodes.

### Nice-to-have

12. Deterministic search heap; case-insensitive Windows path walk.  
13. Store generation directory + pointer swap; meta/payload ID check.  
14. Atlas tools on concise toolset **or** remove from boot card for that mode.  
15. `turbo tree search`; prune/doctor disk usage; fix mojibake.

---

## 6. Suggested feature improvements

### P0 product value

| Feature | Rationale |
|---------|-----------|
| **Auto-stale + opportunistic refresh** | HEAD change → mark stale; first tool call may rebuild |
| **Default-on miss recovery** | Core acceptance story (wrong path → real path) |
| **Compile-safe inject with real worktree CWD** | Subagents must see **their** atlas |

### P1

| Feature | Rationale |
|---------|-----------|
| **Searchable inventory ≠ display collapse** | Authoritative resolve without dumping tree into prompt |
| **Git-cheap incremental** | `git status --porcelain` path patch after full base |
| **Worktree base share + overlay** | Design Phase 2: stop full rewalk per isolation child |
| **Land / PostToolUse invalidation** | Parent atlas updates after densify land |
| **PreCompact mini re-inject** | Compaction must not erase layout memory |
| **Status chip** | `Tree · fresh · 12k` / building / stale |
| **TOML config cascade** | Project `[workspace_tree]` + global + env |
| **Subagent pre-warm** | Kickoff worktree path at spawn |

### P2

| Feature | Rationale |
|---------|-----------|
| Compact `tree.v1.bin` / compression | Large monorepo store size |
| Fuzzy typo resolve | Agent inventiveness |
| Profile packs (godot/rust/node) | Expand/collapse defaults |
| Watch mode | Live atlas |
| Telemetry | build_ms, miss accept rate |
| Store prune / LRU | Disk hygiene |
| Privacy redaction in inject | Sensitive path names |

---

## 7. Implementation status (pre-ship RC13)

| Wave | Items | Status |
|------|--------|--------|
| **P0 hotfixes** | F1–F4 | **Landed** |
| **P1 correctness** | F5–F12 | **Landed** (see CHANGELOG RC13) |
| **Feature slice** | Pre-collapse name index + HEAD-stale rebuild | **Landed** as part of P0/P1 |
| **P2 polish** | F13–F20 (case-insensitive list, deterministic search, prune, mojibake, concise tools, git worktree HEAD, inject-preview honesty) | **Landed** |
| **Still open (later)** | TOML config cascade, worktree base+overlay, binary store, FS watch, fuzzy typo resolve | Not started |

---

## 8. Residual risks (all auditors)

- Agents **over-trust** a permanent “fresh” card.  
- Collapse can **increase** path invention via confident wrong samples.  
- Cold open dual-walk + Windows AV thrash.  
- Store under `~/.grok` grows unbounded; path names can leak layout secrets.  
- Installed binary may still be **r12** until rebuild (CHANGELOG already notes this).

---

## 9. Raw auditor summaries

### Grok 4.5 — 6.5/10
Ship-with-notes. Best structural overview. Strongest callouts: refresh/cache desync, miss hints off, collapse sample paths, UTF-8 panic, trust incomplete.

### GPT-5.6-sol — 4/10
Harshest. Lead finding: **`tool_working_directory` does not exist on `PromptContext`**. Windows rename, collapse destroys index, permanent fresh, no single-writer, trust incomplete. Demands expanded acceptance tests on Windows.

### Nemotron Ultra — 7.5/10
Most favorable on architecture. Aligns on permanent Fresh, truncation silence, races, symlink cycles, CLI worktree targeting, path-similarity weighting. Adds disk growth / schema migration residual risks.

---

_End of triple audit synthesis._
