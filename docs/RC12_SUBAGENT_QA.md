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
| ISO-01 | P0 | Worktree write remaps | Marker only under `H:\t\w\{8hex}\subagent-{id}` | **Pass** `01a03eea-4411-71e0-a536-73085d7d8815` (`H:\t\w\a86e802e\…`, parent marker false) |
| ISO-09 | P0 | Completion tags | `isolation=worktree`, `worktree_path` set, `isolation_fallback` false/absent | **Pass** host tags match |
| ISO-10 | P0 | Child boot card honesty | `Isolation claim: isolation=worktree` when Tool CWD is the short root | **Pass (source)** `cargo test -p xai-grok-agent --lib -- boot_card` 18/18 including `child_card_windows_short_root_says_worktree`. **Live pager still 1.0.0-rc.11.1** so this session's children still print `isolation=none` until a rebuilt `turbo` is launched. |
| ISO-08 | P0 | explore default | `isolation=none`, parent CWD, no worktree | **Pass** (rc.11.1 session `01a03ec1-e2df-75c1-90f3-719226fe0c19`) |
| ISO-umbrella | P0 | Non-git `isolation=worktree` | Fail-closed; no silent share | **Pass** (rc.11.1 session `01a03ec5-4a4a-7ff3-86ce-803bbcfcedf7`) |
| CARD-01 | P0 | Child token budget | Child card ≤320 tokens (unit test) | **Pass** measured 146 / Windows 152 |
| CARD-02 | P1 | Parent short teaches `\t\w\` | Short card mentions `{drive}:\t\w\` or `/t/w/` | **Pass** `short_card_mentions_windows_short_root` |
| CARD-03 | P1 | Parent short size | ≤1200 tokens (unit test; cap still 1650) | **Pass** measured 1168 |
| BRAND-01 | P1 | Welcome / CLI about | User-visible "Turbo Build", not "Turbo Grok Build" | **Pass (source)** landed on `rc12`; live TUI still 11.1 until rebuild |
| BRAND-02 | P1 | `--version --json` | `"product": "turbo-grok-build"`, `"binary": "turbo"` | **Pass** assert kept at pager-bin `version_json_payload` |
| LAND-skip | — | Land probe file | Do **not** land isolation probe files into parent | **Pass** not landed |

## Evidence paths

- Parent session `subagents/<id>/meta.json` (`child_cwd`, `display_cwd`, `worktree_path`, `isolation_fallback`, `isolation_requested`)
- Child session `system_prompt.txt` `<turbo_boot_card mode="child">`
- Marker `H:\t\w\<hash>\subagent-<id>\ISOLATION_PROBE_RC12.txt`

## Loop

Fail → `developer_log` + fix on `rc12` → re-run the failed row. Stop when every P0 is Pass.
