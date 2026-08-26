# 1.0.0-rc.12 Subagent Q&A

| Item | Content |
|------|---------|
| Product | **Turbo Build** (CLI `turbo`) |
| Wire | `1.0.0-rc.12` |
| Theme | Subagent hardening + display-name rename |
| Host under test | live pager session on Windows |
| Plan | `docs/superpowers/plans/2026-08-26-rc12-subagent-hardening.md` |

Fill **Result** as you run. P0 must be green before tagging.

## Results

| ID | Pri | Test | Pass criteria | Result |
|----|-----|------|---------------|--------|
| ISO-01 | P0 | Worktree write remaps | Marker only under `H:\t\w\{8hex}\subagent-{id}` | |
| ISO-09 | P0 | Completion tags | `isolation=worktree`, `worktree_path` set, `isolation_fallback` false/absent | |
| ISO-10 | P0 | Child boot card honesty | `Isolation claim: isolation=worktree` when Tool CWD is the short root | |
| ISO-08 | P0 | explore default | `isolation=none`, parent CWD, no worktree | |
| ISO-umbrella | P0 | Non-git `isolation=worktree` | Fail-closed; no silent share | |
| CARD-01 | P0 | Child token budget | Child card ≤320 tokens (unit test) | |
| CARD-02 | P1 | Parent short teaches `\t\w\` | Short card mentions `{drive}:\t\w\` or `/t/w/` | |
| CARD-03 | P1 | Parent short size | ≤1200 tokens (unit test; cap still 1650) | |
| BRAND-01 | P1 | Welcome / CLI about | User-visible "Turbo Build", not "Turbo Grok Build" | |
| BRAND-02 | P1 | `--version --json` | `"product": "turbo-grok-build"`, `"binary": "turbo"` | |
| LAND-skip | — | Land probe file | Do **not** land isolation probe files into parent | n/a |

## Evidence paths

- Parent session `subagents/<id>/meta.json` (`child_cwd`, `display_cwd`, `worktree_path`, `isolation_fallback`, `isolation_requested`)
- Child session `system_prompt.txt` `<turbo_boot_card mode="child">`
- Marker `H:\t\w\<hash>\subagent-<id>\ISOLATION_PROBE_RC12.txt`

## Loop

Fail → `developer_log` + fix on `rc12` → re-run the failed row. Stop when every P0 is Pass.
