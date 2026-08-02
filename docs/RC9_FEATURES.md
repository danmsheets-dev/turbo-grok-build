# RC9 features (dev branch)

Not yet released as a version tag. Implements RC8 worktree feedback + agent productivity.

## 1. Isolated worktree trust

| Item | Behavior |
|------|----------|
| Spawn baseline | After worktree create: `refs/grok/subagent-baselines/<id>` |
| Agent-only patch | `changes.patch` = `baseline..snapshot` (not `HEAD..snapshot`) |
| Soft preserve | Live tree kept by default for review; `GROK_SUBAGENT_SOFT_PRESERVE=0` restores immediate delete |
| Land guard | Refuse land if agent delta > 50 files unless `force=true` |
| Restore | `hyper subagent open <id> --restore [--restore-dir PATH]` |
| Clean seed | `GROK_SUBAGENT_WORKTREE_SEED=clean` → HEAD-only (no parent dirty copy) |
| Windows HOME | `USERPROFILE` / `home_dir` fallback when `HOME` unset |

## 2. Agent Boot Card

Injected into the system prompt on new primary sessions (and a tiny child stub for subagents).

```text
GROK_BOOT_CARD=off|short|full   # default short
```

Content: session facts, tools map, subagent lifecycle, recovery CLI, dirty-diff footgun, git safety.

## 3. Copy icon on completed messages

`appearance.scrollback.display.selection_buttons` defaults to **true**. Select a completed message to show the copy (and view) icon on the bottom-right of the selection frame; click copies full message text.

## 4. Auto Developer Log (from earlier RC9 work)

See [AUTO_DEVELOPER_LOG.md](./AUTO_DEVELOPER_LOG.md).
