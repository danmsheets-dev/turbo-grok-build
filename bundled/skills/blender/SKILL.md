---
name: blender
description: >
  Work on Blender projects and add-ons: honest headless invocation, extension
  manifests, bl_info vs blender_manifest.toml, .blend binary rules, and
  BLENDER_USER_SCRIPTS. Use when editing Blender add-ons, Python bpy scripts,
  .blend files, or when the user mentions Blender, bpy, or extensions.
metadata:
  short-description: "Blender add-on and headless competence"
---

# Blender

## Headless invocation (the only honest form)

```bash
blender --background --factory-startup --python-exit-code 1 --python path/to/script.py
```

Why each flag matters:

| Flag | Why |
|---|---|
| `--background` | No UI; required for CI |
| `--factory-startup` | Disables **every** installed add-on, including the one under test — the script must enable the module itself |
| `--python-exit-code 1` | Without this, a raising Python script still exits **0** |
| `--python <script>` | Entry point |

### Enabling the add-on under test

Because `--factory-startup` blanks add-ons, scripts must do:

```python
import addon_utils
addon_utils.enable("<module_name>", default_set=False, persistent=True)
```

Do not assume the add-on is already active.

## Add-on identity

- **Legacy:** `bl_info` dict in `__init__.py` (name, version, blender version, …).
- **Blender 4.2+ extensions:** `blender_manifest.toml` at the extension root.
- Validate a 4.2+ extension with:  
  `blender --command extension validate`

## Files

| Path | Kind | Agent rule |
|---|---|---|
| `*.blend` | Binary document | **Never** text-patch |
| `*.blend1`, `*.blend2` | Save backups | Leave alone / delete only intentionally |
| `*.blend@*` | Crash-save temp | Not a real document; recover in UI |
| `*.py` in the add-on | Source | Edit normally |
| `blender_manifest.toml` | Source (4.2+) | Edit carefully |

## Pointing Blender at a specific add-on tree

`BLENDER_USER_SCRIPTS` is the supported way to make Blender load scripts from a
chosen directory (e.g. a worktree’s `scripts/` layout). Prefer that over
copying into the user config dir mid-run.

## Agent checklist

1. Use the full headless flag set above for any CI/scripted run.
2. Enable the add-on inside the script after factory startup.
3. Never open or patch `.blend` as text — use `bpy.ops.wm.open_mainfile` /
   save operators inside a Python script.
4. Prefer extension validate for 4.2+ packaging errors over guessing.
