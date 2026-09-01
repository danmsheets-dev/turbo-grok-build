# Design: Workspace Tree (Tree Logging)

**Status:** Draft developer specification (world-class / not schedule-bound)  
**Product:** Turbo / Hyper Grok Build harness  
**Related FR:** `fr_019fc727f54a70419c015748386718ec`  
**Authoring context:** Cold-session path guesses (`scripts/ship/ship_roster.gd` vs real `scripts/core/`), large Godot/game repos, multi-worktree subagents  
**Last updated:** 2026-08-03

---

## 1. Problem statement

Agents open a workspace with almost no structural memory of the filesystem. They invent paths, call `list_dir`/`grep` serially, burn turns, and surface red â€œfile failedâ€ rows that look like product bugs.

**Tree Logging** gives every session a **maintained, queryable, token-budgeted map of the workspace layout** â€” not a knowledge graph of call edges (that is GitNexus), but a **authoritative atlas of where files and directories live**.

### 1.1 Primary user-visible outcomes

1. On folder open, the harness builds (or loads) a tree snapshot before the agent needs it.
2. The agent can resolve â€œwhere is `ship_roster`?â€ in one tool call without inventing folders.
3. Failed `read_file` / write targets get nearest-path suggestions from the atlas.
4. Subagents inherit or share the same atlas without each re-walking the disk.
5. The map stays correct as the tree changes (edit, land, commit, worktree).

### 1.2 Non-goals

- Not a substitute for `grep` content search.
- Not a substitute for GitNexus process/impact graphs.
- Not a full desktop file indexer (Spotlight/Everything) for the whole machine â€” **workspace-scoped**.
- Not embedding every binary asset path into the model context.
- Not requiring network, cloud, or user accounts.

---

## 2. Design principles

| Principle | Implication |
|---|---|
| **Atlas, not dump** | Full trees exist in storage; models see summaries + query results. |
| **Fail soft, stay fast** | Indexer never blocks first keystroke; partial maps are usable. |
| **Ignore by default** | `.gitignore` + hard excludes; binary/kit directories collapse to counts. |
| **One truth, many views** | Single store â†’ inject, tools, slash UI, hooks, miss recovery. |
| **Staleness is first-class** | Every response includes freshness; agents can trust or refresh. |
| **Worktree-aware** | Parent workspace + subagent worktrees are distinct indexes with optional inheritance. |
| **Config is progressive** | Zero-config works; power users can tune every budget and policy. |
| **Observable** | Timing, hit rates, miss-suggest accept rates land in developer log / metrics. |

---

## 3. Architecture overview

```
â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
â”‚  Session / Workspace lifecycle                                   â”‚
â”‚  open Â· trust Â· chdir Â· worktree spawn Â· land Â· commit Â· close   â”‚
â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
                             â”‚ events
                             â–¼
â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
â”‚  Tree Indexer Service (async, workspace-scoped)                  â”‚
â”‚  Â· Walker (gitignore, caps, heuristics)                          â”‚
â”‚  Â· Normalizer (paths, case, Windows)                             â”‚
â”‚  Â· Summarizer (collapse rules, role tags)                        â”‚
â”‚  Â· Incremental updater (git status / FS notify / tool hooks)     â”‚
â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
                â”‚                             â”‚
                â–¼                             â–¼
â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”   â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
â”‚  Durable Store            â”‚   â”‚  Hot Cache (session RAM)        â”‚
â”‚  ~/.grok/workspace-trees/ â”‚   â”‚  mmap / Arc<TreeIndex>          â”‚
â”‚  + optional SQLite        â”‚   â”‚  per workspace_id + worktree_id â”‚
â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜   â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
                â”‚                                 â”‚
                â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
                                 â–¼
        â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
        â”‚  Surfaces                                       â”‚
        â”‚  1. Session inject (budgeted summary card)      â”‚
        â”‚  2. Tools: workspace_tree / resolve_path        â”‚
        â”‚  3. Miss recovery on read_file / write failures â”‚
        â”‚  4. Hooks: SessionStart, PostToolUse, PostLand  â”‚
        â”‚  5. TUI: /tree, status chip, explorer pane      â”‚
        â”‚  6. Subagent bootstrap snapshot ref             â”‚
        â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
```

### 3.1 Component map (suggested crate / module split)

| Module | Responsibility |
|---|---|
| `xai-workspace-tree` (new crate) | Core types, walk, ignore, summarize, query, serialize |
| `turbo` session host | Lifecycle hooks, inject, tool registration |
| `xai-tool-runtime` | `workspace_tree`, `resolve_path` tool handlers |
| Store backend | File JSONL/JSON + optional SQLite FTS |
| TUI | `/tree`, progress, staleness chip |
| Telemetry | Index duration, query latency, suggest accept |

Keep the core pure (no TUI, no model) so unit tests and headless mode share one implementation.

---

## 4. Identity and scope

### 4.1 Workspace identity

```text
workspace_id = hash(canonical_absolute_path)
canonical_path = resolve symlinks + normalize case (Windows) + strip trailing sep
```

Optional secondary keys:

- `git_common_dir` (so bare worktrees of the same repo can share a **base** index)
- `git_worktree_path` (each worktree has a **delta** layer)

### 4.2 Scopes

| Scope | Root | Use |
|---|---|---|
| `workspace` | User-opened folder | Primary session |
| `worktree` | Subagent isolation path (`~/.grok/worktrees/...`) | Child agent |
| `overlay` | Explicit multi-root (`--add-dir` / ACP `additionalDirectories`) | Phase 1 |

### 4.3 Trust boundary

Only index **trusted** workspace roots (existing `trusted_folders.toml`). Untrusted / outside-workspace paths are never walked by default.

---

## 5. Data model

### 5.1 On-disk layout (recommended default)

```text
~/.grok/workspace-trees/
  index.json                          # registry of workspaces
  <workspace_id>/
    meta.json                         # identity, versions, stats, freshness
    tree.v1.bin                       # compact binary (preferred) OR tree.v1.json
    tree.v1.json                      # human-debug export (optional, on demand)
    names.fts.sqlite                  # optional FTS for name search (optional backend)
    log.jsonl                         # optional event log (rebuilds, errors)
```

**Do not** store under the project tree by default (avoids dirty git status).  
Optional project-local cache: `.grok/workspace-tree/` when `tree.store.project_local = true` (must be gitignored by installer or documented).

### 5.2 `meta.json` (schema sketch)

```json
{
  "schema_version": 1,
  "workspace_id": "w_â€¦",
  "root": "H:\\Pirates",
  "canonical_root": "H:\\Pirates",
  "git": {
    "present": true,
    "head": "a1daf51câ€¦",
    "branch": "main",
    "common_dir": "H:\\Pirates\\.git"
  },
  "created_at": "2026-08-03T12:00:00Z",
  "updated_at": "2026-08-03T12:00:04Z",
  "build": {
    "mode": "full",
    "duration_ms": 1840,
    "walker": "gitignore_v1",
    "app_version": "0.2.x"
  },
  "stats": {
    "dirs": 842,
    "files": 12540,
    "ignored_dirs": 120,
    "collapsed_dirs": 48,
    "bytes_seen": 0,
    "truncated": false
  },
  "freshness": {
    "state": "fresh",
    "basis": "git_head+mtime_sample",
    "dirty_paths": 0
  },
  "budgets": { "â€¦copy of effective configâ€¦" }
}
```

### 5.3 Tree node model

Logical model (language-agnostic):

```text
TreeNode {
  rel_path: string          # POSIX-style relative, no leading ./
  kind: file | dir | collapsed_dir | symlink
  name: string
  ext: string?              # lowercased, no dot; files only
  children?: TreeNode[]     # dirs only; absent if collapsed or leaf-summary
  file_count?: u32          # dir aggregate (recursive, non-ignored)
  dir_count?: u32
  size_bytes?: u64          # optional; expensive â€” config gated
  mtime_ms?: u64            # optional
  role_tags?: string[]      # heuristic: source, test, docs, asset, build, config, vendor
  lang_hint?: string        # gdscript, rust, python, â€¦
  flags?: bitset            # executable, symlink, large, generated
}
```

**Collapsed directory:** a dir that is too large/noisy to expand. Stores counts + optional sample names (top N by role priority), not full children.

### 5.4 Compact binary format (world-class option)

Prefer a compact binary for large repos:

- Path dictionary (interned segments)
- Flat node array + child index ranges
- Optional roaring bitmap of â€œsource-likeâ€ nodes for fast filters

JSON remains a **debug/export** format (`turbo tree export --format json`).

### 5.5 Name index

Secondary structure for O(log n) / FTS name lookup:

| Backend | Pros | Cons |
|---|---|---|
| **In-memory hashmap** `basename â†’ [node_ids]` | Simple, fast enough <100k files | RAM |
| **SQLite FTS5** | Scales, fuzzy `MATCH`, durable | Extra file, Windows care |
| **fst / tantivy** | Excellent prefix/fuzzy | Heavier dep |

**Recommendation:** in-memory basenames + path-segment inverted index for MVP+; SQLite FTS as config `tree.search.backend = "sqlite"` for huge monorepos.

---

## 6. Scanning and ignore policy

### 6.1 Walker algorithm

1. Start at workspace root (or worktree root).
2. Load ignore stack:
   - Hard excludes (always)
   - `.gitignore` (git hierarchy)
   - `.git/info/exclude`
   - Global gitexcludes if present
   - Turbo defaults
   - User/project config globs
3. BFS or iterative DFS with explicit stack (no recursion depth risk).
4. For each dir:
   - If ignored â†’ skip (record in stats only if `tree.stats.count_ignored`)
   - If collapse rule matches â†’ emit `collapsed_dir`, do not descend (optional shallow sample)
   - Else emit `dir`, descend until depth/budget hit
5. For each file: apply size, extension, and name filters; tag role/lang.

### 6.2 Hard excludes (defaults)

Always ignore unless user forces `tree.ignore.allow`:

```
.git/
.grok/worktrees/
.godot/
node_modules/
target/
dist/
build/
.venv/
__pycache__/
.pytest_cache/
.mypy_cache/
.tox/
.idea/
.vs/
.vscode/.browse*
*.pyc
*.pdb
*.dll
*.exe          # optional: still show as collapsed â€œbinaries presentâ€
```

### 6.3 Collapse rules (defaults)

Collapse (counts only) when **any** match:

| Rule | Example |
|---|---|
| Dir named in collapse list | `assets/models/**` kit folders with hundreds of LODs |
| File count under dir > `N` | `tree.collapse.max_files_per_dir = 80` |
| Depth > `max_expand_depth` | deep generated trees |
| Extension histogram dominated by binaries | `*.glb`, `*.png`, `*.wav` > 90% of children |
| Path matches asset glob | `**/*_LOD[0-9].glb` parent |

Collapsed node records:

```json
{
  "kind": "collapsed_dir",
  "rel_path": "assets/models/hull",
  "file_count": 42,
  "sample": ["hull_sloop_LOD0.glb", "hull_sloop_LOD1.glb"],
  "ext_histogram": { "glb": 40, "import": 40, "md": 2 },
  "role_tags": ["asset"]
}
```

### 6.4 Role tagging heuristics

Cheap path/name heuristics (no content parse required for MVP):

| Tag | Signals |
|---|---|
| `source` | `scripts/`, `src/`, `crates/`, `*.gd`, `*.rs`, `*.ts`, `*.py` |
| `test` | `tests/`, `*_test.*`, `test_*.gd` |
| `docs` | `docs/`, `*.md` (non-node_modules) |
| `scene` | `*.tscn`, `scenes/` |
| `asset` | `assets/`, `*.glb`, `*.png`, `*.wav` |
| `config` | `project.godot`, `Cargo.toml`, `package.json`, `*.toml` |
| `tool` | `tools/`, `scripts/tools` |
| `vendor` | `addons/` third-party, `third_party/`, `vendor/` |
| `generated` | `.uid`, `.import`, `*.generated.*` |

Tags power summary cards (â€œsource layoutâ€) without dumping assets.

### 6.5 Language / stack detection (workspace card)

From top-level markers, set `workspace_profile`:

- Godot: `project.godot`
- Rust: `Cargo.toml`
- Node: `package.json`
- Python: `pyproject.toml` / `requirements.txt`
- Mixed: list all

Profile selects **default expand roots** (e.g. Godot â†’ expand `scripts/`, `scenes/`, `docs/`; collapse `assets/models/**`).

---

## 7. Freshness and incremental updates

### 7.1 Freshness states

| State | Meaning | Agent guidance |
|---|---|---|
| `fresh` | Matches head/mtime basis | Trust |
| `likely_fresh` | Minor dirty files outside source roots | Trust for path lookup |
| `stale` | Head changed or many dirty paths | Prefer refresh before structural claims |
| `building` | Index in progress | Partial results OK |
| `error` | Last build failed | Fall back to live `list_dir` |
| `missing` | Never built | Build now |

### 7.2 Invalidation triggers

| Trigger | Action |
|---|---|
| Session open | Load cache; if missing/stale â†’ async rebuild |
| `git commit` / merge / rebase / checkout (PostToolUse shell or git hooks) | Mark stale or incremental update |
| Subagent **land** into parent | Parent incremental refresh on landed paths |
| N seconds after burst of write tools | Debounced refresh of touched subtrees |
| Manual `/tree refresh` or tool `refresh: true` | Full or incremental rebuild |
| Config change of ignore/collapse | Full rebuild |
| FS watcher events (optional) | Debounced path-level patch |

### 7.3 Incremental strategies (options)

| Mode | Description | When |
|---|---|---|
| `full` | Rewalk entire tree | First build, force, ignore-rule change |
| `git_status` | Apply `git status --porcelain` + untracked | Best default for git repos |
| `path_patch` | Rewalk only listed roots | After land / write batch |
| `mtime_sample` | Re-stat sample of dirs; if drift â†’ full | Non-git folders |
| `watch` | OS notify (ReadDirectoryChangesW / inotify / FSEvents) | Power mode; off by default on Windows network drives |

**Recommended default:** `git_status` incremental when `.git` present; else `mtime_sample` + periodic full.

### 7.4 Worktree layering (optimal for Turbo)

```
Base index (git common_dir @ commit SHA)
   + Worktree overlay (sparse dirty paths for that worktree)
   = Effective tree for agent cwd
```

Subagent with `isolation=worktree`:

1. Inherit parent base snapshot by reference (no full rewalk).
2. Build small overlay for the worktree path.
3. On land: parent applies path_patch for landed files.

---

## 8. Agent surfaces

### 8.1 Session inject â€” â€œWorkspace Tree Cardâ€

At `SessionStart` / `before_agent_start` (same path as AGENTS.md / boot card):

Inject a **budgeted** card, never the full tree.

#### Default card contents (~1.5â€“4k tokens, configurable)

```markdown
## Workspace tree (fresh Â· 1.8s Â· 12,540 files)

Root: H:\Pirates
Stack: Godot 4 (project.godot) Â· git main@a1daf51c
Source map:
  scripts/core/     62 *.gd   (rules, pure)
  scripts/ui/       14 *.gd
  scripts/ship/     11 *.gd
  scripts/world/    22 *.gd
  scenes/ui/        12 *.tscn
  docs/             â€¦ (briefs, orientation)
  tools/            â€¦
Assets: assets/models/** collapsed (kits; use resolve_path for stems)
Ignore: .godot, *.import noise collapsed

Tools: workspace_tree, resolve_path
Tip: resolve_path before inventing folders.
```

#### Inject modes (`tree.inject.mode`)

| Mode | Behavior |
|---|---|
| `off` | No inject; tools only |
| `minimal` | Root + stack + top-level dirs only |
| `standard` (default) | Source map + collapsed asset note + freshness |
| `rich` | + role histograms, recent dirty paths, key entry files |
| `profile:<name>` | Named profiles (godot, rust_workspace, monorepo) |

#### Token budget knobs

- `tree.inject.max_tokens` (default 2500)
- `tree.inject.max_top_dirs` (default 24)
- `tree.inject.expand_globs` (force-expand always, e.g. `scripts/**`, `src/**`)
- `tree.inject.collapse_globs`
- `tree.inject.include_entrypoints` (detect `main.gd`, `project.godot`, `Cargo.toml` paths)

### 8.2 Tools

#### 8.2.1 `workspace_tree` (primary)

**Purpose:** Query the atlas without walking disk.

```json
{
  "action": "summary | list | search | subtree | stats | refresh",
  "path": "scripts/ui",
  "query": "ship_roster",
  "glob": "**/*upgrade*",
  "ext": ["gd", "tscn"],
  "role": ["source", "test"],
  "depth": 2,
  "limit": 50,
  "include_collapsed": false,
  "refresh": false,
  "worktree": "auto | parent | <id>"
}
```

| action | Returns |
|---|---|
| `summary` | Inject-equivalent card (freshness + source map) |
| `list` | Children of `path` (one level or `depth`) |
| `search` | Basename / path substring / fuzzy hits ranked |
| `subtree` | Nested view under `path` with collapse respected |
| `stats` | Counts, histograms, build timing |
| `refresh` | Rebuild (full/incremental per flags) then summary |

**Response always includes:**

```json
{
  "freshness": "fresh",
  "root": "H:\\Pirates",
  "result": { },
  "truncated": false,
  "hints": ["Use resolve_path for unique basenames"]
}
```

#### 8.2.2 `resolve_path` (path intelligence)

**Purpose:** â€œWhat did I mean?â€ â€” the tool that kills invented folders.

```json
{
  "name": "ship_roster",
  "hint_path": "scripts/ship/ship_roster.gd",
  "ext": "gd",
  "role": "source",
  "limit": 8
}
```

Ranking signals (weighted):

1. Exact basename match
2. Stem match (`ship_roster` â†’ `ship_roster.gd`)
3. Path similarity to `hint_path` (edit distance on segments)
4. Role/tag preference (`source` > `generated`)
5. Recency (if mtimes enabled)
6. Git path history (optional: `git log --follow` / renames â€” advanced)

Returns:

```json
{
  "matches": [
    {
      "path": "scripts/core/ship_roster.gd",
      "score": 0.96,
      "why": ["exact_stem", "role:source", "hint_segment_overlap:scripts"]
    }
  ],
  "best": "scripts/core/ship_roster.gd"
}
```

#### 8.2.3 Optional: `workspace_glob`

Thin alias over `workspace_tree` with `action=search` + glob only â€” if you want tool parity with classic â€œGlobâ€ tools agents already know.

### 8.3 Miss recovery (world-class default)

When `read_file` / write tools fail with not-found **inside workspace**:

1. Call internal `resolve_path` with the failed path as `hint_path`.
2. If top score â‰¥ threshold, attach to tool error:

```text
File not found: scripts/ship/ship_roster.gd
Did you mean:
  1. scripts/core/ship_roster.gd  (score 0.96)
  2. scripts/core/ship_roster.gd.uid
```

3. Config:
   - `tree.miss_suggest.enabled` (default true)
   - `tree.miss_suggest.max` (default 5)
   - `tree.miss_suggest.min_score` (default 0.55)
   - `tree.miss_suggest.auto_retry` (default **false** â€” never silent rewrite of agent intent)

**Do not** auto-read the suggestion without the agent choosing (safety + predictability).

### 8.4 Hooks integration

| Event | Behavior |
|---|---|
| `SessionStart` | Ensure index started; inject card when ready or inject â€œbuildingâ€¦â€ + tools available on partial |
| `PostToolUse` (write/edit) | Mark path dirty; debounce patch |
| `PostToolUse` (git commit/land) | Stale or incremental |
| `PreCompact` | Re-inject mini summary so compaction doesnâ€™t lose map |
| `Stop` | Optional: flush dirty patches |

### 8.5 Subagent contract

- Parent passes `workspace_tree_ref` (workspace_id + generation) in subagent bootstrap.
- Explore/plan agents get tools `workspace_tree` + `resolve_path` in read-only mode.
- Worktree agents see **effective tree** = parent base + worktree overlay.
- Land pipeline: after land, parent `path_patch` for landed paths.

---

## 9. Configuration reference (all useful options)

Location cascade (later wins):

1. Built-in defaults  
2. `~/.grok/config.toml` `[workspace_tree]`  
3. Project `.grok/tree.toml` or `[workspace_tree]` in project config  
4. CLI flags / env for one session  
5. Tool call overrides (refresh, depth) where safe  

### 9.1 Master

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.enabled` | bool | `true` | Master switch |
| `tree.auto_build_on_open` | bool | `true` | Start index when workspace opens |
| `tree.block_session_until_ready` | bool | `false` | **Avoid true** except CI; prefer async |
| `tree.ready_timeout_ms` | u64 | `0` | If blocking, max wait (0 = no block) |
| `tree.background_priority` | enum | `low` | `low \| normal` OS priority for walker |

### 9.2 Storage

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.store.backend` | enum | `file` | `file \| sqlite \| hybrid` |
| `tree.store.dir` | path | `~/.grok/workspace-trees` | Global store root |
| `tree.store.project_local` | bool | `false` | Also write `.grok/workspace-tree/` |
| `tree.store.format` | enum | `bin` | `bin \| json` |
| `tree.store.keep_json_debug` | bool | `false` | Dual-write JSON for humans |
| `tree.store.max_cached_workspaces` | u32 | `32` | LRU eviction of cold indexes |
| `tree.store.compress` | bool | `true` | zstd for bin payloads |

### 9.3 Walk budgets

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.walk.max_files` | u32 | `250000` | Hard stop; mark truncated |
| `tree.walk.max_dirs` | u32 | `100000` | Hard stop |
| `tree.walk.max_depth` | u32 | `32` | Absolute max depth |
| `tree.walk.max_expand_depth` | u32 | `8` | Depth before forced collapse |
| `tree.walk.max_duration_ms` | u64 | `15000` | Soft deadline; return partial |
| `tree.walk.max_file_size_bytes` | u64 | `0` | 0 = unlimited for *listing*; size still optional |
| `tree.walk.follow_symlinks` | bool | `false` | Dangerous; off by default |
| `tree.walk.cross_device` | bool | `false` | Donâ€™t leave volume |
| `tree.walk.threads` | u32 | `0` | 0 = min(4, cores) for parallel readdir batches |
| `tree.walk.collect_mtime` | bool | `true` | Needed for freshness samples |
| `tree.walk.collect_size` | bool | `false` | Extra stat cost |
| `tree.walk.case_sensitive` | enum | `auto` | `auto \| true \| false` |

### 9.4 Ignore / collapse

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.ignore.use_gitignore` | bool | `true` | |
| `tree.ignore.use_global_gitignore` | bool | `true` | |
| `tree.ignore.extra` | string[] | `[]` | Additional globs |
| `tree.ignore.allow` | string[] | `[]` | Force-include even if gitignored (careful) |
| `tree.ignore.hard` | string[] | (built-in) | Always exclude |
| `tree.collapse.globs` | string[] | asset defaults | |
| `tree.collapse.names` | string[] | `node_modules`, `target`, â€¦ | |
| `tree.collapse.max_files_per_dir` | u32 | `80` | |
| `tree.collapse.binary_ext_ratio` | f32 | `0.9` | |
| `tree.collapse.sample_names` | u32 | `5` | Samples kept on collapsed nodes |
| `tree.collapse.asset_exts` | string[] | glb,png,wav,â€¦ | |

### 9.5 Inject

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.inject.mode` | enum | `standard` | See Â§8.1 |
| `tree.inject.max_tokens` | u32 | `2500` | |
| `tree.inject.max_top_dirs` | u32 | `24` | |
| `tree.inject.expand_globs` | string[] | stack-default | |
| `tree.inject.collapse_globs` | string[] | stack-default | |
| `tree.inject.include_entrypoints` | bool | `true` | |
| `tree.inject.include_dirty` | bool | `true` | Show dirty source paths count |
| `tree.inject.on_partial` | enum | `card` | `card \| skip \| building_notice` |
| `tree.inject.on_precompact` | bool | `true` | Re-inject mini card |

### 9.6 Tools / miss recovery

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.tools.enabled` | bool | `true` | |
| `tree.tools.expose_workspace_tree` | bool | `true` | |
| `tree.tools.expose_resolve_path` | bool | `true` | |
| `tree.tools.default_limit` | u32 | `50` | |
| `tree.tools.max_limit` | u32 | `500` | |
| `tree.miss_suggest.enabled` | bool | `true` | |
| `tree.miss_suggest.max` | u32 | `5` | |
| `tree.miss_suggest.min_score` | f32 | `0.55` | |
| `tree.miss_suggest.auto_retry` | bool | `false` | Never default true |
| `tree.search.backend` | enum | `memory` | `memory \| sqlite` |
| `tree.search.fuzzy` | bool | `true` | Typo-tolerant basename |
| `tree.search.fuzzy_max_distance` | u32 | `2` | |

### 9.7 Freshness / watch

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.freshness.mode` | enum | `git_status` | `git_status \| mtime_sample \| watch \| manual` |
| `tree.freshness.auto_refresh` | bool | `true` | |
| `tree.freshness.debounce_ms` | u64 | `800` | |
| `tree.freshness.refresh_on_commit` | bool | `true` | |
| `tree.freshness.refresh_on_land` | bool | `true` | |
| `tree.freshness.stale_after_ms` | u64 | `0` | 0 = only event-based |
| `tree.watch.enabled` | bool | `false` | Opt-in FS events |
| `tree.watch.exclude_globs` | string[] | assets heavy | |

### 9.8 Worktrees / multi-agent

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.worktree.inherit_base` | bool | `true` | Share parent base index |
| `tree.worktree.overlay` | bool | `true` | Track worktree deltas |
| `tree.worktree.rebuild_on_spawn` | bool | `false` | Usually unnecessary |
| `tree.subagent.inject` | enum | `minimal` | Card size for children |
| `tree.subagent.tools` | bool | `true` | |

### 9.9 Privacy / safety

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.privacy.skip_secret_names` | bool | `true` | Still list `.env` as name-only; never read contents |
| `tree.privacy.redact_home_in_inject` | bool | `false` | Show `~/â€¦` form |
| `tree.security.trusted_only` | bool | `true` | |
| `tree.security.allow_outside_workspace_query` | bool | `false` | |

### 9.10 Telemetry

| Key | Type | Default | Description |
|---|---|---|---|
| `tree.telemetry.enabled` | bool | `true` | Local metrics only |
| `tree.telemetry.log_slow_ms` | u64 | `3000` | developer_log when build slow |
| `tree.telemetry.record_miss_suggest` | bool | `true` | Counters, no path contents if privacy on |

### 9.11 Environment overrides

```text
TURBO_TREE=0|1
TURBO_TREE_INJECT=off|minimal|standard|rich
TURBO_TREE_STORE_DIR=...
TURBO_TREE_FORCE_FULL=1
TURBO_TREE_WATCH=1
```

---

## 10. CLI and TUI

### 10.1 CLI (`turbo tree` / `grok tree`)

```text
turbo tree status              # freshness, stats, path to store
turbo tree build [--full]      # rebuild
turbo tree show [path]         # print subtree
turbo tree search <query>      # basename/path search
turbo tree resolve <name>      # same as resolve_path
turbo tree export [--json|--dot|--markdown] [--out file]
turbo tree inject-preview      # print the exact card the agent would see
turbo tree clean [--all]       # drop cache for workspace or LRU
turbo tree doctor              # ignore parse errors, watch support, timings
```

### 10.2 Slash commands

| Command | Action |
|---|---|
| `/tree` | Show summary card in TUI |
| `/tree scripts/ui` | Subtree |
| `/tree search roster` | Search |
| `/tree refresh` | Rebuild |
| `/tree off` / `/tree on` | Session toggle inject |

### 10.3 Status bar chip

`Tree Â· fresh Â· 12k` / `Tree Â· building 62%` / `Tree Â· stale` with click â†’ `/tree`.

### 10.4 Optional explorer pane

Read-only tree browser bound to the same store (not a second walker). Useful for humans; agents still use tools.

---

## 11. Performance budgets (acceptance targets)

| Scenario | Target |
|---|---|
| Warm open (cache hit, same HEAD) | **&lt; 50 ms** to serve tools + inject |
| Cold open, mid repo (10â€“30k files, gitignore ok) | **&lt; 3 s** full build |
| Cold open, large monorepo (100k+ files) | **&lt; 15 s** partial-usable; complete under 60 s with collapse |
| `resolve_path` | **&lt; 5 ms** p95 in-memory |
| `workspace_tree search` | **&lt; 20 ms** p95 for 50 results |
| Inject card | **â‰¤ configured max_tokens** always |
| Session start blocked | **0 ms** default (async) |

Partial maps must answer `resolve_path` for already-walked roots while build continues.

---

## 12. Concurrency and consistency

- Single writer per `workspace_id` (mutex / file lock).
- Readers use generation counter: `generation` bumps on swap.
- Tools never block on full rebuild longer than `tree.tools.max_wait_ms` (default 100); return partial + `freshness=building`.
- Atomic replace: write `tree.v1.bin.tmp` â†’ fsync â†’ rename.

---

## 13. Windows / cross-platform notes

- Normalize to stored **relative POSIX** paths; accept Windows inputs on tools.
- Respect `\\?\` long paths when needed.
- Default `follow_symlinks=false` (junction loops).
- Case: on case-insensitive FS, store actual disk casing from readdir; match case-insensitively in `resolve_path`.
- Defender / AV: batch readdir; avoid statting every file if `collect_size=false`.
- Network drives: disable `watch` by default; increase `max_duration_ms`.

---

## 14. Interaction with existing systems

| System | Relationship |
|---|---|
| `list_dir` | Live truth for one directory; tree is atlas. Prefer tree for â€œwhere is Xâ€, list_dir for â€œwhatâ€™s here right now in this folderâ€. |
| `grep` | Content search; tree filters paths first (`role=source`). |
| AGENTS.md / project rules | Rules stay normative; tree is descriptive layout. |
| Boot card | Tree card is a sibling inject, not a replacement. |
| GitNexus | Optional companion: tree for paths, GitNexus for call graphs. If GitNexus indexed, inject one line â€œGitNexus: availableâ€. |
| Subagent worktrees | Overlay model Â§7.4 |
| Permissions | Tree tools require same workspace trust as `list_dir` |

### 14.1 Optional GitNexus bridge

`tree.bridge.gitnexus = true`:

- After tree build, if `gitnexus status` shows indexed, tools can return `gitnexus_repo` name.
- Never require GitNexus for tree to function.

---

## 15. Security and privacy

- Do not embed file **contents** in the tree store.
- Listing `.env` name is OK; miss-suggest must not open it.
- Redact absolute home prefixes in shared logs if telemetry on.
- Untrusted folders: no index.
- Symlink escape: refuse targets outside workspace when resolving.

---

## 16. Observability

### 16.1 Local metrics (session)

- build_ms, file_count, collapsed_dirs  
- query_ms histogram  
- miss_suggest_shown / (optional) agent followed suggestion (heuristic: next read matches suggestion)

### 16.2 Developer log events

Emit `developer_log` when:

- Build exceeds budget (`perf_regression` / tree component)
- Ignore parser failure storm
- Truncation hits max_files (docs_gap? or feature signal)

Do not log full path lists.

---

## 17. Testing plan

### 17.1 Unit (`xai-workspace-tree`)

- Ignore stack matches git check-ignore fixtures  
- Collapse rules  
- Path normalization Windows/Unix  
- Ranking: exact stem > wrong folder guess  
- Incremental git_status patch correctness  
- Token budget never exceeded for inject renderer  
- Atomic write / generation swap  

### 17.2 Integration (Turbo)

- Open Pirates-sized fixture â†’ card + resolve_path finds `scripts/core/ship_roster.gd` from wrong hint  
- `read_file` miss attaches suggestions  
- Subagent worktree overlay after create file  
- Land refreshes parent  
- Compaction re-injects mini card  

### 17.3 Golden fixtures

Commit a small synthetic repo:

```text
fixture_tree_basic/
  src/foo.rs
  src/bar/baz.rs
  node_modules/huge/...
  assets/many_bins/...
```

Golden JSON for summary + search results.

### 17.4 Perf benches

- 1k / 10k / 100k synthetic files  
- Regression gate in CI for build_ms and resolve_ms  

---

## 18. Phased delivery (recommended)

World-class does not mean big-bang. Ship depth over time without abandoning quality of v1.

### Phase 0 â€” Spike (1â€“2 days)

- Walker + gitignore + in-memory tree  
- `resolve_path` CLI only  
- Measure Pirates cold build  

### Phase 1 â€” MVP (production quality)

- Durable file store (`meta.json` + `tree.v1.bin` or JSON)  
- Async build on open  
- Inject `standard` card  
- Tools: `workspace_tree`, `resolve_path`  
- Miss suggestions on `read_file`  
- `/tree` + `turbo tree status|build|search|resolve`  
- Config subset: enable, inject mode, ignore extra, collapse globs  
- Tests + Pirates acceptance  

**Wave C RC13 (implemented):** crate + tools + session inject + miss recovery + `/tree` + `tree_cmd` library.  
Env: `GROK_WORKSPACE_TREE`, `GROK_WORKSPACE_TREE_INJECT` (+ `TURBO_TREE*` aliases).  
Freshness: `updated_at` = built_at; `basis` includes git HEAD. Subagent inject prefers minimal; root is tool CWD.  
CLI top-level `turbo tree` needs pager-bin `Command::Tree` wire on land — see `docs/_PATCH_PAGER_BIN_TREE.md` and `docs/workspace-tree.md`.

### Phase 2 â€” Turbo-native depth

- Worktree base+overlay  
- Land/commit invalidation  
- PreCompact re-inject  
- Status chip  
- Telemetry counters  
- SQLite FTS backend option  

### Phase 3 â€” World-class polish

- FS watch mode  
- Fuzzy search + rename awareness (`git log --name-status` optional)  
- Profile packs (godot, rust, node, monorepo)  
- Explorer pane  
- GitNexus bridge line  
- Export markdown map for humans  
- Adaptive collapse from past miss patterns (local only)  

### Phase 4 â€” Intelligence (optional, careful)

- Light content peek for entrypoint detection only (first N bytes of known manifests)  
- Learning: boost paths the user/agent successfully opens (privacy-preserving local weights)  
- Multi-root workspaces  

---

## 19. API sketches (tools)

### 19.1 Tool descriptions (agent-facing)

**workspace_tree**  
*Query the maintained workspace directory atlas (not live disk by default). Use for layout, listing known paths, and searching names. Prefer resolve_path when you have a basename or a wrong guessed path. Check freshness in the response.*

**resolve_path**  
*Map a free-form name or guessed path to real workspace paths ranked by confidence. Call this instead of inventing directories. Does not read file contents.*

### 19.2 Error shapes

Align with existing tool protocol:

- `not_ready` + partial  
- `truncated`  
- `outside_workspace`  
- `disabled`  

---

## 20. Acceptance criteria (definition of done for â€œworld classâ€)

1. **Cold Pirates session:** agent finds `ship_roster` via `resolve_path` without prior `list_dir`, &lt; 1 turn.  
2. **Wrong path:** `read_file scripts/ship/ship_roster.gd` error lists `scripts/core/ship_roster.gd`.  
3. **No HUD regression:** inject â‰¤ token budget; full tree never dumped into system prompt.  
4. **Warm open** &lt; 50 ms tool serve from cache.  
5. **Ignore correctness:** `.godot/` and `node_modules/` absent from expanded nodes.  
6. **Assets collapsed:** `assets/models/**` not expanded file-by-file in inject.  
7. **Worktree:** child agent resolves new file it created; parent updates after land.  
8. **Headless:** same tools work in `-p` mode.  
9. **Trust:** untrusted folder â†’ no index, tools explain why.  
10. **Doctor:** `turbo tree doctor` explains watcher support, last build error, store path.  
11. **Tests:** unit + integration + golden fixture green in CI.  
12. **Docs:** user-guide page + AGENTS tip line for projects that opt into rich inject.

---

## 21. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Stale paths after rename | git_status incremental + miss_suggest |
| Huge monorepo RAM | collapse + bin format + file cap + sqlite backend |
| Agent ignores tools | inject tip + miss recovery + explore-agent prompt bake-in |
| Double source of truth with list_dir | docs: tree=atlas, list_dir=live |
| Windows AV slowdown | fewer stats, parallel readdir, deadline partial |
| Privacy (.env names) | list-only; never content; optional hide secret globs entirely |
| Over-engineering Phase 1 | stick to Phase 1 cut line; keep crate boundaries for later |

---

## 22. Suggested defaults (opinionated â€œoptimalâ€)

For Turbo out of the box:

```toml
[workspace_tree]
enabled = true
auto_build_on_open = true
block_session_until_ready = false

[workspace_tree.store]
backend = "file"
format = "bin"
project_local = false

[workspace_tree.walk]
max_files = 250000
max_expand_depth = 8
max_duration_ms = 15000
collect_mtime = true
collect_size = false
follow_symlinks = false

[workspace_tree.inject]
mode = "standard"
max_tokens = 2500

[workspace_tree.miss_suggest]
enabled = true
auto_retry = false

[workspace_tree.freshness]
mode = "git_status"
refresh_on_land = true
refresh_on_commit = true

[workspace_tree.watch]
enabled = false

[workspace_tree.worktree]
inherit_base = true
overlay = true
```

Godot profile auto when `project.godot` exists:

```toml
expand_globs = ["scripts/**", "scenes/**", "docs/**", "tools/**", "resources/**"]
collapse_globs = ["assets/models/**", "assets/terrain/**", "**/*.import", ".godot/**"]
```

---

## 23. Documentation deliverables

1. This design doc (implementation source of truth)  
2. `docs/user-guide/NN-workspace-tree.md` â€” user facing  
3. Changelog entry when shipping  
4. Update explore/general agent tool lists and â€œwhen to useâ€ blurb  
5. Optional: project template snippet for `.grok/tree.toml`

---

## 24. Open questions (decide during Phase 0/1)

1. Binary format home-grown vs reuse an existing crate (rkyv, postcard, capnp)?  
2. Should inject live in boot card merge vs separate system-reminder? (Prefer **system-reminder / before_agent inject** so it can refresh without rewriting durable system.)  
3. Multi-root workspaces: in-scope for v1 or Phase 4?  
4. Is `workspace_glob` a separate tool or only an action? (Prefer single tool with actions.)  
5. Store schema migrations: support n-1 only or full versioned readers?

---

## 25. Summary

**Optimal Turbo design:** an async, gitignore-aware, collapse-heavy **workspace atlas** stored under `~/.grok/workspace-trees/`, with:

- **Budgeted inject** on session start  
- **Query tools** (`workspace_tree`, `resolve_path`)  
- **Miss recovery** on failed reads  
- **Git-based incremental freshness**  
- **Worktree base+overlay** for subagents  
- **Deep config** for power users without burdening defaults  

This is the harness-native answer to â€œwhere is everything?â€ â€” complementary to GitNexusâ€™s â€œhow does it connect?â€, and the right foundation to implement without hurry to world-class quality.

