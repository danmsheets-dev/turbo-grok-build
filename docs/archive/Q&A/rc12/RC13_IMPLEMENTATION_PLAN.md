# RC13 Implementation Plan

| Item | Content |
|------|---------|
| Product | Turbo Grok Build (`turbo`) |
| Baseline ship | **0.2.114-r12** |
| Target ship | **0.2.114-r13** |
| Date | 2026-08-03 |
| Sources | RC12 Q&A audit, live ADL/FRL, `docs/design-workspace-tree.md`, densify incidents, Game Mode visual pass lag |
| Artifacts | [`RC12_HARNESS_AUDIT_AND_QA_PLAN.md`](./RC12_HARNESS_AUDIT_AND_QA_PLAN.md), [`RC12_PHASE2_RESULTS.md`](./RC12_PHASE2_RESULTS.md) |

---

## 0. Goals (RC13 definition of done)

1. **Close all outstanding Q&A bugs** (P0–P2 real product defects; resolve smoke P3s).  
2. **Ship full Workspace Tree Phase‑1 MVP** from design (not tools-only): inject card, tools hardened, miss recovery, `/tree` + CLI, config, tests, docs.  
3. **Game Mode is smooth again** after RC12 visual edits (restore post-optimization feel; measure, don’t guess).  
4. **No regression** of RC12 isolation wins (ISO-01 abs-path remap, live marker, land fail-closed, ADL/FRL).  
5. Ship notes honest: residual shell ≠ OS jail remains accepted unless Wave A hardens further.

### Non-goals (defer past RC13 unless free)

- Full design Phase 3–4 (FS watch mode, explorer pane, adaptive collapse, multi-root).  
- SQLite FTS backend (Phase 2 optional; only if monorepo stress forces it).  
- OS sandbox (Landlock / AppContainer).  
- Fixing third-party MCP auth (sentry/sourcegraph/resend) unless config docs only.

---

## 1. What Q&A proved works (do not regress)

| Area | Evidence |
|------|----------|
| Isolation abs parent write remap (file tools) | ISO-01 PASS |
| `.grok-subagent-live` + real worktree path | ISO-09 PASS |
| ADL + FRL tools + CLI | Phase 2 PASS |
| Path miss similar-name hints | TREE-05 PASS |
| Coordinator concurrency queue | unit `spawn_queues_when_at_concurrency_limit` |
| `xai-workspace-tree` core walk/summary | unit crate green |
| MCP subset | blender, chrome-devtools, docs, gitnexus, godot-docs, react-docs, tasks |

---

## 2. Priority stack (implementation order)

| Wave | Pri | Theme | Items | Est. |
|------|-----|-------|-------|------|
| **A** | P0 | Worktree tombstone / densify safety | Write fail-closed when CWD gone; shell clear fail; prune never kills live; discard meta terminal | L |
| **B** | P1 | Tree tools correctness (partially done) | Ship Cwd `resolve_cwd` fix; rebuild; retest TREE-01–04 | S (code in tree) |
| **C** | P1 | **Workspace Tree full MVP** | Inject card, config, `/tree`, CLI, miss recovery, freshness honesty, worktree-aware, docs | **XL** |
| **D** | P1 | Isolation / land residual | Baseline soft-fail, resume fail-closed retest, land CLI parity, discard status bug | M |
| **E** | P1 | **Game Mode performance** | Profile → cache invalidation discipline → paint budget; playground bench | M |
| **F** | P2 | Docs + honesty | Ctrl+G, soft-preserve default, boot card tree tip, deep-audit status | S |
| **G** | P2 | Polish | Shell PowerShell multi-statement UX under confine; orchestrator tree tools optional | S–M |
| **H** | — | Ship | VERSION r13, CHANGELOG, package tests, release-dist, Q&A matrix green | M |

Ship order for humans: **A → B → C + E in parallel → D → F/G → H**.

---

## 3. Wave A — Worktree tombstone & densify safety (P0)

**Incidents:**  
`inc_019fc8a670fa77…` (write success + missing file),  
`inc_019fc8a2d77b7b…` (shell os error 267),  
`inc_019fc8b26b3976…` (supervisor prune race),  
`inc_019fc8bf3e2b7f…` (discard leaves running).

### A1. Write fail-closed when confine root / CWD is gone

| | |
|--|--|
| **Problem** | Mid-run worktree delete (prune race or external rm) leaves tools reporting success while files never land. |
| **Fix** | Before write: require real CWD + ConfineRoot (if set) exist and are directories. After write: exist+size verify (LocalFs already partial) — **fail the tool** if verify fails. Never return success for a path that does not exist. |
| **Primary files** | `xai-grok-tools` LocalFs / ConfinedFs / `enforce_write_path`; write + search_replace paths; optional session health probe on tool start |
| **Acceptance** | Unit: delete root mid-flight → write returns error, no parent pollution. Live densify repro: no false success. |

### A2. Shell on dead worktree

| | |
|--|--|
| **Problem** | `os error 267` / invalid directory after tombstone; agent loops. |
| **Fix** | Pre-flight shell: if session CWD missing, fail with explicit `worktree_tombstone` / `cwd_missing` and surface completion cancel. Optional: auto-file ADL once per session. |
| **Acceptance** | Clear error string; no silent hang; coordinator can mark child failed. |

### A3. Prune never deletes RUNNING + live marker

| | |
|--|--|
| **Problem** | Keep-N / supervisor bulk delete races densify spawns. |
| **Fix** | (1) Product prune paths (`prune_soft_preserved`, disk-guard prune, any CLI) **must** skip dirs with fresh `.grok-subagent-live` and meta status running. (2) Docs: never `Remove-Item` bulk worktrees. (3) Optional: `turbo subagent prune` only. |
| **Acceptance** | Spawn ≥ keep-N+2 concurrent isolation children; all live trees survive keep-N pass; unit pin. |

### A4. Discard meta always terminal

| | |
|--|--|
| **Problem** | Discard can leave `status=running` / incomplete flags. |
| **Fix** | Re-audit `discard.rs` + CLI discard; force meta to discarded/cleaned even if remove fails; set `snapshot_dropped` honestly. |
| **Acceptance** | Discard after soft-preserve and after path already gone → meta terminal both ways. |

---

## 4. Wave B — Tree tools Cwd fix (ship + close incidents)

**Already implemented in source (RC12 tree dirty):**  
`workspace_tree` + `resolve_path` use `resolve_cwd` + `shared_resources`.  
Tests: `*_uses_resources_cwd_without_extension_override` **PASS**.

| Task | Done? |
|------|-------|
| Code + unit tests | **Yes (source)** |
| Rebuild/install turbo binary | **RC13 ship** |
| Live TREE-01–04 on isolation=none and worktree children | Retest |
| Resolve ADL `inc_019fc8c12e8a79…`, `inc_019fc8c29cde72…` | After retest green |

---

## 5. Wave C — Full Workspace Tree MVP (headline RC13 feature)

**Design authority:** [`docs/design-workspace-tree.md`](../../design-workspace-tree.md) §8, §11, §18 Phase 1, §20 acceptance.  
**FRs:** `fr_019fc8c0c9e67c…` (inject card), `fr_019fc8c177ee7e…` (boot layout).  
**RC12 shipped:** crate, tools (partial), kickoff load, partial miss hints.  
**RC12 missing:** inject, slash, CLI, config host bridge, freshness honesty, full miss recovery, worktree overlay, docs.

### C0. Scope cut for “full” RC13

Implement **design Phase 1 MVP + the Turbo-critical pieces of Phase 2** that densify needs:

| Design Phase 1 item | RC13 |
|---------------------|------|
| Durable store | Yes (JSON ok; bin optional stretch) |
| Async build on open | Yes (kickoff already; harden) |
| Inject `standard` card | **Yes — must** |
| Tools workspace_tree / resolve_path | Yes + Cwd fix + schema gaps (glob/ext if cheap) |
| Miss suggestions on read_file | Yes — warm cache always; expand write/search_replace |
| `/tree` + `turbo tree …` | **Yes** |
| Config subset | **Yes** |
| Tests + acceptance | **Yes** |
| Worktree base+overlay (Phase 2) | **Yes (minimal)** — child index root = worktree path; parent refresh on land optional |
| PreCompact re-inject (Phase 2) | **Yes if inject path exists** |
| Status chip / SQLite FTS | No (defer) unless free |

### C1. Session inject — Workspace Tree Card

| | |
|--|--|
| **Behavior** | On trusted session start (and boot card short path), inject budgeted card from `inject_card()` when index ready; if still building, inject one-line “Workspace tree: building… tools available” or skip per config. |
| **Modes** | `tree.inject.mode = off \| minimal \| standard \| rich` (default **standard**). Env override e.g. `GROK_WORKSPACE_TREE_INJECT=off`. |
| **Budget** | `tree.inject.max_tokens` default ~2500; never dump full tree. |
| **Children** | Subagents: `minimal` by default (`tree.subagent.inject`) so densify children get orientation without huge prompt. |
| **Primary files** | `xai-workspace-tree` inject; `xai-grok-agent` prompt/context + boot_card; `mvp_agent` / session spawn kickoff wait-or-partial |
| **Acceptance** | New session system context contains `## Workspace tree` (or equivalent) with root + top dirs; token estimate ≤ budget; inject off removes it. Cold monorepo / Pirates: agent can name real top-level dirs without list_dir first. |

### C2. Tools hardening

| Item | Work |
|------|------|
| Cwd | Wave B |
| Actions | summary/list/search/stats/refresh (+ subtree→list already) |
| Toolsets | Keep default/explore/plan; **add to orchestrator** (research agents need atlas) |
| Errors | `disabled`, `not_ready`, `outside_workspace` shapes |
| Worktree param | Prefer real tool CWD; optional `worktree=auto` later |

### C3. Miss recovery

| | |
|--|--|
| **Today** | Similar entries when parent dir exists (TREE-05 PASS). |
| **RC13** | On read/write/search_replace path miss, if atlas warm: suggest resolve_path hits + nearest names. Never block on build. |
| **Files** | `path_suggestions.rs`, read_file / write / search_replace error paths |

### C4. Slash + CLI

| Surface | Commands |
|---------|----------|
| TUI | `/tree` (summary), `/tree off\|on`, `/tree refresh`, `/tree search <q>` |
| CLI | `turbo tree status \| build \| search \| resolve \| doctor \| inject-preview` |
| Doctor | Store path, last build error, enabled, inject mode, watcher n/a |

### C5. Config host bridge

Wire progressive config (defaults work without file):

```toml
# ~/.grok/config.toml or project .grok/config subset
[workspace_tree]
enabled = true
# inject.mode = "standard"
# inject.max_tokens = 2500
```

Env: `GROK_WORKSPACE_TREE=0` disables entirely.

### C6. Freshness honesty

| Today | Always stamp Fresh / full_walk |
|-------|--------------------------------|
| RC13 | At minimum: record `built_at`, `git_head` if available; mark `stale` if HEAD moved or mtime trigger; `refresh` action rebuilds. Incremental walker optional stretch. |

### C7. Worktree-aware (minimal Phase 2)

| | |
|--|--|
| Child isolation=worktree | Index/kickoff against **worktree path** (not parent only). |
| Land | Optional parent index invalidate or path_patch for landed files. |
| Acceptance | Child resolves file it created under worktree; after land, parent resolve finds it (refresh ok). |

### C8. Docs + boot tip

- User-guide page: Workspace Tree (tools, inject, `/tree`, disable).  
- Boot card short line: use `workspace_tree` / `resolve_path`; inject when enabled.  
- Close FRs after inject ships.  
- Resolve `inc_019fc8bf3e1f71…` (boot omits layout) via inject, not prose-only.

### C9. Workspace Tree acceptance matrix (must all pass)

1. Cold session: inject card present (mode standard).  
2. `resolve_path ship_roster` (or monorepo basename) without prior list_dir.  
3. Wrong path → miss suggestions.  
4. Inject ≤ token budget; no full tree in prompt.  
5. Warm open tool serve fast (cache).  
6. `target/` / `node_modules` collapsed / not expanded.  
7. Untrusted folder → no index / clear error.  
8. Headless `-p` tools work.  
9. Subagent isolation=worktree: tools work (Cwd fix).  
10. Unit + fixture tests green.  
11. `/tree` + `turbo tree doctor` usable.  

---

## 6. Wave D — Isolation / land residual (P1–P2)

| ID | Item | Acceptance |
|----|------|------------|
| D1 | Baseline capture soft-fail → fail closed or hard retry | Spawn without baseline refuses land without force; spawn should succeed with baseline when disk ok |
| D2 | ISO-04 resume non-worktree + isolation=worktree | Fail closed without ALLOW_SHARED; opt-in honest tags |
| D3 | Land CLI vs tool parity | CLI documents force/allowlist; prefer tool; add only_missing if missing |
| D4 | Write-time allowed_paths retest | OOB write denied at write, not only land |
| D5 | ISO-02 shell abs write to parent | Denied or confined; parent clean |
| D6 | Completion isolation fields | Retest tags after densify |

---

## 7. Wave E — Game Mode performance (RC13 release list)

**Symptom:** RC11/early RC12 Game Mode was optimized to feel fast (~10–12 Hz Slow tick + sprite cache + frame fingerprint). After RC12 **visual/hover/SNES floor** edits it feels laggy again.

### E1. Measure first

| Tool | Action |
|------|--------|
| Playground | `cargo run -p xai-grok-pager --bin game-mode-playground` |
| Instrumentation | Time: bg scale, `compose_cell_frame`, halfblock paint, full paint path; log p50/p95 ms per frame |
| Fingerprint | Confirm cache hits when tick-only animation; misses on resize/hover/state |

### E2. Likely fix areas (from code shape)

| Hotspot | Path | Risk after visual pass |
|---------|------|------------------------|
| Full recompose every Slow tick | `compose.rs` `compose_cell_frame` | Hover/expanded hit targets dirty too often |
| BG rescaling | `compose.rs` / `state.rs` | Floor mask / SNES pass invalidates cache more than needed |
| Sprite cache thrash | thread-local cache keys | New palette/skin/hover keys explode cache or miss |
| Halfblock downsample | render paint | Larger PIXEL_SCALE × bigger blit |
| Hover popups | `render.rs` | Per-frame hit test + popup paint without dirty region |
| Tick demand | `app_view` TickDemand::Slow | Accidentally Fast while Game Mode open |

### E3. Required optimizations (implement as needed from profile)

1. **Strict dirty flags:** only recompose when fingerprint inputs change (tick frame bucket, desk states, hover target, size).  
2. **Separate layers:** static BG (cached) + sprites (cached) + hover overlay (cheap); don’t rebuild BG for blink.  
3. **Hover:** recompute hit targets on move only; popup paint without full office recompose.  
4. **Cap PIXEL_SCALE / canvas size** on small terminals; keep SNES look at mid sizes.  
5. **Throttle:** keep Game Mode on `TickDemand::Slow` (~8–12 Hz), never Fast unless user setting.  
6. **Regression test:** playground or unit bench asserts compose under budget on fixed size (e.g. p95 &lt; N ms on CI machine — document N).  

### E4. Acceptance

| Criterion | Pass |
|-----------|------|
| Subjective | Office view feels as smooth as post-first-optimization RC11 |
| Objective | Profiled compose+paint p95 under agreed budget at 120×40 / 200×50 |
| Idle | No Fast tick when only Game Mode animating |
| Functional | Hover expansion, Ctrl+G, SNES floor still correct |
| Tests | `cargo test -p xai-grok-pager --lib views::game_mode` green |

### E5. Files

- `crates/codegen/xai-grok-pager/src/views/game_mode/{compose,render,state,mod,sprites_pixel}.rs`  
- `app_view.rs` tick demand wiring  
- `bin/game_mode_playground.rs` optional FPS overlay for local QA  

---

## 8. Wave F — Docs & honesty (P2)

| Item | Change |
|------|--------|
| User-guide keyboard | Ctrl+G = Game Mode; Ctrl+Shift+G = tasks; Shift+G = GotoBottom |
| `16-subagents.md` | Soft-preserve default (not “removed by default”) |
| design-game-mode | Ctrl+G not Shift+G |
| KNOWN_ISSUES | RC13 tombstone + tree inject status |
| DEEP_AUDIT doc | Header: tool jail shipped; shell residual |
| CHANGELOG r13 | Full entry |

---

## 9. Wave G — Polish (P2, as time allows)

| Item | Note |
|------|------|
| PowerShell multi-statement under shell confine | Reduce false `shell-unparseable` or better error |
| Tree tools on concise/hashline | Optional; orchestrator yes |
| Resolve smoke ADL incidents | ISO-01 / ADL smoke P3 → resolve |

---

## 10. Wave H — Ship checklist

```text
[ ] VERSION → 0.2.114-r13
[ ] xai-grok-version / lockstep
[ ] CHANGELOG [0.2.114-r13]
[ ] Free disk ≥ 40 GB before package tests; ≥ 60 GB before release-dist
[ ] cargo test -p xai-grok-tools --lib -- --test-threads=4
[ ] cargo test -p xai-grok-shell --lib -- --test-threads=4
[ ] cargo test -p xai-workspace-tree --lib --tests
[ ] cargo test -p xai-grok-pager --lib views::game_mode -- --test-threads=4
[ ] Live Q&A matrix: ISO-01, TREE inject, TREE tools, tombstone A1–A3, GM perf
[ ] turbo issues: resolve fixed; export pack for remaining deferred
[ ] turbo features: ship/ack inject FRs
[ ] release-dist build: cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
[ ] Install smoke: turbo --version shows r13
```

---

## 11. Incident / FR map → wave

| ID | Sev | Title | Wave |
|----|-----|-------|------|
| inc_019fc8a670fa77… | P0 | Write success + tombstoned CWD | **A1** |
| inc_019fc8a2d77b7b… | P1 | Shell os error 267 | **A2** |
| inc_019fc8b26b3976… | P2 | Prune raced live worktrees | **A3** |
| inc_019fc8bf3e2b7f… | P2 | Discard leaves running | **A4** |
| inc_019fc8c12e8a79… | P1 | workspace_tree Cwd | **B** (fixed source) |
| inc_019fc8c29cde72… | P2 | resolve_path Cwd worktree | **B** |
| inc_019fc8bf3e1f71… | P2 | Boot omits layout | **C1** |
| fr_019fc8c0c9e67c… | FR | Tree inject card | **C1** |
| fr_019fc8c177ee7e… | FR | Boot layout inject | **C1** |
| — | — | Full Workspace Tree MVP | **C** |
| — | — | Game Mode lag regression | **E** |
| — | — | Docs Ctrl+G / soft-preserve | **F** |
| P3 smoke ×2 | P3 | Q&A probes | **G** resolve |

---

## 12. Suggested implementation parallelism

| Lane | Owner shape | Work |
|------|-------------|------|
| Lane 1 | Shell/tools | Wave A tombstone + Wave B retest + Wave D |
| Lane 2 | Tree | Wave C full MVP |
| Lane 3 | Pager UI | Wave E Game Mode perf + Wave F docs keyboard |
| Lane 4 | Integration | Live Q&A matrix + ship H |

Max **4 Grok 4.5 subagents** (isolation=worktree for code lanes). Orchestrator lands with `land_subagent` only.

---

## 13. Risk register

| Risk | Mitigation |
|------|------------|
| Tree inject blows context budget | Hard max_tokens; standard mode collapses assets; mode off escape hatch |
| Large monorepo cold index slow | Async kickoff; partial inject “building…”; never block first keystroke |
| Game Mode optim breaks SNES art | Visual golden/playground screenshots before/after |
| Tombstone fix false-negatives writes | Only fail when root missing or post-write verify fails; retry once |
| Disk pressure on Windows tests | Package-scoped tests; clean target/debug PDBs per AGENTS.md |
| Scope creep to design Phase 3–4 | Cut line §C0; explorer/FS watch explicitly out |

---

## 14. Success criteria (RC13 release gate)

| Gate | Criterion |
|------|-----------|
| **G1** | No open P0/P1 tombstone or tree-Cwd incidents after retest |
| **G2** | Workspace Tree inject visible on new trusted session (default mode) |
| **G3** | `resolve_path` + inject + `/tree` + `turbo tree doctor` documented and working |
| **G4** | Game Mode subjective + objective perf gate met |
| **G5** | ISO-01 still PASS; ADL/FRL still work |
| **G6** | CHANGELOG + VERSION r13; release-dist turbo runs |

---

## 15. Status

| Phase | State |
|-------|-------|
| Plan | **This document** |
| Implementation | **Source landed 2026-08-03** (Waves A–H). **No compile/install** per operator request. |
| Next action | Package tests + `release-dist` when ready; live Q&A; resolve ADL after retest |

### Source land checklist (no binary yet)

| Wave | In tree? |
|------|----------|
| A tombstone write/shell/discard | Yes |
| B tree Cwd `resolve_cwd` | Yes |
| C inject + `/tree` + `turbo tree` + docs | Yes |
| D baseline fail-closed | Yes (`GROK_SUBAGENT_ALLOW_BASELINE_SOFT_FAIL`) |
| E Game Mode fingerprint/hover perf | Yes |
| F docs Ctrl+G / soft-preserve | Yes |
| G boot card atlas tip | Yes |
| H VERSION/CHANGELOG r13 | Yes (binary still r12 until rebuild) |

---

_End of RC13 plan. Update checkboxes and link commits/PRs as waves land._
