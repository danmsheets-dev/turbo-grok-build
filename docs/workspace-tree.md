# Workspace Tree (directory atlas)

**Status:** Phase 1 MVP (Wave C RC13)  
**Design:** [design-workspace-tree.md](./design-workspace-tree.md)

## What it is

A **token-budgeted map of the workspace layout** — not a content search index and not a call graph (GitNexus). Agents use it to stop inventing paths and to recover from `read_file` misses.

## Surfaces

| Surface | How |
|--------|-----|
| **Session inject** | Budgeted card in the system prompt after boot card (`<workspace_tree_card>`) |
| **Tools** | `workspace_tree`, `resolve_path` |
| **Miss recovery** | `read_file` / similar “does not exist” errors attach atlas “Did you mean?” when warm |
| **Slash** | `/tree`, `/tree doctor`, `/tree inject-preview`, `/tree refresh`, `/tree resolve <name>`, `/tree search <q>` |
| **CLI** | `turbo tree status\|doctor\|inject-preview\|build\|resolve\|search\|prune` |

## Config / env

| Env | Effect |
|-----|--------|
| `GROK_WORKSPACE_TREE` / `TURBO_TREE` | `0` / `false` / `off` disables; `1` / `on` enables |
| `GROK_WORKSPACE_TREE_INJECT` / `TURBO_TREE_INJECT` | `off` \| `minimal` \| `standard` \| `rich` |
| `GROK_WORKSPACE_TREE_STORE_DIR` / `TURBO_TREE_STORE_DIR` | Override durable store root (default `~/.grok/workspace-trees`) |

Defaults: **enabled**, inject **standard** for primary sessions, **minimal** preferred for subagents (unless inject env is set explicitly).

## Lifecycle

1. Trusted session open → fire-and-forget `kickoff_load` on the **real tool CWD** (worktree for isolated children).
2. Prompt build → inject card from process cache / durable store (never blocks on a walk). If not ready: short “building…” notice.
3. Tools → `get_or_load` (cache → store → build).
4. Miss recovery → `try_get` / `try_load_cached` only (never builds).

## Freshness (Phase 1)

Full walks stamp `freshness.state = fresh`. Meta records:

- `updated_at` — build timestamp (`built_at`)
- `freshness.basis` — `full_walk+built_at=<rfc3339>[+git_head=<short>@branch]`
- `git.head` / `git.branch` — best-effort from `.git/HEAD` (including linked worktrees)

True incremental invalidation is Phase 2.

## Agent tips

- Prefer `resolve_path` for basenames (`ship_roster`) before inventing folders.
- Use `workspace_tree` `summary` / `search` / `list` for layout; do **not** dump the full tree into context.
- Subagents see their **tool CWD** atlas (worktree), not a hardcoded parent path.

## Related

- Feature request: `fr_019fc727f54a70419c015748386718ec`
- Crate: `crates/codegen/xai-workspace-tree`
