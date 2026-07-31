---
name: godot
description: >
  Work on Godot Engine projects: project.godot layout, .tscn/.tres/.uid source
  rules, headless import/run, GUT/gdUnit4 tests, and what must never be
  text-patched or deleted. Use when editing Godot games, scenes, scripts, or
  addons, or when the user mentions Godot, GDScript, .tscn, .tres, or GUT.
metadata:
  short-description: "Godot Engine project competence"
---

# Godot Engine

## Project layout

- Root marker: `project.godot`. Read `config_version` — it maps to the engine
  major the project was last saved with (treat mismatches as upgrade work).
- Source (commit these):
  - `.tscn` / `.tres` — **structured text** scenes/resources
  - `.gd` / `.cs` — scripts
  - `*.uid` companions — stable `uid://…` identifiers
  - `*.import` next to imported assets — import settings (source, not cache)
  - `addons/`, `project.godot`, `export_presets.cfg`
- Regenerable caches (do **not** commit; do **not** hand-edit):
  - `.godot/` (including `.godot/imported/…`)
  - some projects also use a top-level `.import/` — still cache

## UID references

- Scenes/resources reference dependencies as `ext_resource … uid="uid://…"`.
- Those uids live in sibling `*.uid` files. **Deleting a `.uid` regenerates a
  new random uid** and breaks every `ext_resource` that pointed at the old one.
- Treat `.uid` and asset-side `.import` as **source**. Treat `.godot/` as trash
  that reappears on next editor/import run.

## Headless CLI

```bash
# Reimport assets (writes into .godot/)
godot --headless --path . --import

# Smoke-open project then quit
godot --headless --path . --quit-after 1

# Parse a script without running it
godot --headless --path . --check-only --script res://path/to/script.gd
```

**Exit codes lie.** Both `--import` and `--quit-after 1` often exit `0` while
printing `SCRIPT ERROR:` lines. Always read stdout/stderr; never treat exit 0
as “no script errors”.

## Tests

- **GUT:** common CLI shape  
  `godot --headless --path . -s addons/gut/gut_cmdln.gd -gexit`
- **gdUnit4:** the other common runner — check the project's `addons/gdUnit4`
  docs for the exact command; same rule: inspect output, not just exit code.

## Editor lock

The editor holds a lock on `.godot/`. A shared CI cache + a developer’s open
editor will fight (import thrash, missing uids, “file in use”). Prefer a
per-worktree `.godot/` and never share that directory across concurrent runs.

## Editing rules for agents

- **Edit freely:** `.tscn`, `.tres`, `.gd`, `.uid`, asset `.import` companions.
- **Never text-patch:** `.pck`, binary `.res`/`.scn`, anything under `.godot/`.
- Prefer structured text resources over binary ones when creating new content.
