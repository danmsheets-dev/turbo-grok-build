# Auto Developer Log (ADL)

Structured product-issue pipeline for Turbo agents and maintainers.

When agents or the runtime hit product friction (worktrees deleted without
recovery artifacts, provider deser failures, isolation fallback, stalls),
Turbo records a **deduplicated, redacted incident** under the configured log
root (default `$GROK_HOME/developer-log/`). The Turbo Development Team exports
a pack and triages from real field signal — not chat archaeology.

The **Agent Boot Card** instructs agents that `developer_log` is **required**
for product friction (not optional).

## Quick start

```bash
# List open incidents
turbo issues list

# Show one incident
turbo issues show inc_<id>

# Export a maintainer pack (summary.md + incidents.ndjson + evidence/)
turbo issues export
turbo issues export --severity p0 --severity p1 --out ./hyper-issues-pack

# Paths / status
turbo issues path
turbo issues path --json
turbo issues ack <id>
turbo issues resolve <id>
turbo issues resolve <id> --sha <gitsha> --note "landed keep-N PID/retain"

# Set where logs are stored (writes ~/.grok/developer-log.toml)
turbo issues set-dir D:/HyperLogs/developer-log
turbo issues set-dir ~/Projects/hyper-field-logs
turbo issues clear-dir

# Opt-in GitHub Issues sync (never the default)
turbo issues sync --repo owner/name
turbo issues sync --push
turbo issues sync --pull
```

Disable writes:

```bash
# Unix
export GROK_DEVELOPER_LOG=0

# Windows PowerShell
$env:GROK_DEVELOPER_LOG = "0"
```

### Configuring the log directory

| Precedence | Source |
|------------|--------|
| 1 (highest) | Process override after `turbo issues set-dir` this session |
| 2 | Env `GROK_DEVELOPER_LOG_DIR=/absolute/or/~/path` |
| 3 | File `$GROK_HOME/developer-log.toml` → `dir = "..."` |
| 4 (default) | `$GROK_HOME/developer-log` |

Example `~/.grok/developer-log.toml`:

```toml
# Turbo Auto Developer Log root (managed by `turbo issues set-dir`)
dir = "D:\\HyperLogs\\developer-log"

# Opt-in GitHub Issues inbox (default off — no cloud upload)
# github_sync = "off" | "manual" | "on-file"
github_repo = "danmsheets-dev/turbo-field-logs"
github_sync = "off"
```

`set-dir` / `clear-dir` preserve `github_repo` and `github_sync`.

## Storage layout

```text
<log-root>/                    # default: $GROK_HOME/developer-log
  index.json                   # fast list / fingerprint index
  events.jsonl                 # append-only create/increment/status events
  incidents/
    YYYY-MM-DD/
      inc_<uuid>.json          # canonical incident document
  bundles/
    export-<timestamp>/        # turbo issues export output
      summary.md
      incidents.ndjson
      fingerprints.csv
      manifest.json
      evidence/
        inc_*.json
```

Default `$GROK_HOME` is `~/.grok` (shared with official `grok`).

## Agent tool: `developer_log`

**Required** for Turbo product friction. Agents file issues with the built-in tool:

| Field | Required | Notes |
|-------|----------|--------|
| `title` | yes | Short product title |
| `summary` | yes | 1–3 sentences |
| `error_class` | yes | Stable taxonomy (below) |
| `kind` | no | `bug`, `product_friction`, `feature_gap`, … |
| `severity` | no | `p0`–`p3` (defaults from class) |
| `component` | no | e.g. `["subagent","worktree"]` |
| `repro_steps` / `expected` / `actual` | no | Repro block |
| `suggested_fix` | no | Maintainer hint |
| `provider` / `model` / `subagent_id` | no | Environment |
| `fingerprint` | no | Prefer auto-compute |

Rules for agents:

- **Always** call this tool when Turbo product behavior blocks work — do not rely on chat alone.
- Prefer a stable `error_class` over essays.
- One call per distinct product issue; the store **dedups by fingerprint** and increments `occurrence_count`.
- Never log secrets, tokens, or full unredacted prompts (fields are redacted, but do not rely on that alone).

## Error class taxonomy

| Class | Meaning | Default sev |
|-------|---------|-------------|
| `worktree_tombstone` | Path advertised after delete | P0 |
| `work_lost_risk` | Worktree removed without snapshot/patch | P0 |
| `protocol_deser` | Provider stream/tool parse failure | P0 |
| `isolation_fallback` | Sandbox failed open to shared cwd | P1 |
| `subagent_stall` | No progress / timeout | P1 |
| `provider_400` | Request rejected | P1 |
| `provider_auth` | Auth/refresh failure | P1 |
| `land_conflict` | Land/merge failed | P1 |
| `tool_schema` | Tool args invalid/ignored | P2 |
| `mcp_connect` | MCP handshake fail | P2 |
| `catalog_stale` | EOL/404 models | P2 |
| `feature_gap` | Missing product affordance | P2 |
| `perf_regression` | Extreme latency/cost | P2 |
| `docs_gap` | Missing documentation | P3 |
| `unknown` | Needs human triage | P3 |

## Auto detectors (runtime)

| Detector | Trigger |
|----------|---------|
| `worktree_dispose` | Worktree removed **without** `snapshot_ref` and **without** `changes.patch` |
| `isolation_fallback` | Child fell back to shared parent cwd |
| `subagent_stall` | `error_class` is `stall` / `timeout` / `budget` |

Healthy RC8 dispose (snapshot + patch before delete) does **not** create noise.

## Incident document (schema v1)

Key fields:

- `incident_id`, `fingerprint`, `occurrence_count`
- `severity`, `status`, `kind`, `error_class`, `component`
- `title`, `summary`, `suggested_fix`
- `environment` (version, OS, provider, model, session/subagent ids)
- `repro` (steps, expected, actual, confidence)
- `evidence` (meta_path, snapshot_ref, patch_path, related_events)
- `source` (`agent` | `runtime` | `human` | `cli`, `auto`, `detector`)

## Export pack for the Turbo team

`turbo issues export` produces a reviewable pack:

1. **summary.md** — human TL;DR with top incidents
2. **incidents.ndjson** — one JSON object per line
3. **fingerprints.csv** — counts for prioritization
4. **evidence/** — full incident JSON copies
5. **manifest.json** — version, OS, counts, redaction policy

Hand the directory (or zip it) to the Turbo Development Team.

## Privacy

- Secrets and common credential shapes are redacted via `xai-grok-secrets`.
- User home path segments are scrubbed.
- Free-form fields are length-capped.
- **No cloud upload by default** — local JSON is the write-ahead log. GitHub
  sync is opt-in (`github_repo` + `turbo issues sync` or `github_sync = "manual"` /
  `"on-file"`).
- Agent `developer_log` never waits on the network. `on-file` is a fail-open
  background push after the local write.
- Sync refuses to upload a document that still matches a known secret shape
  after sanitizers. Unauthenticated or missing `gh` fails the CLI sync.

## GitHub Issues sync (opt-in)

Approach: one GitHub issue per fingerprint, labeled `type:incident`. Auth is
the existing `gh` CLI (`gh auth login`). Point at a **private** repo:

```bash
# 1. Create a private repo (example)
gh repo create danmsheets-dev/turbo-field-logs --private --clone=false

# 2. Configure (~/.grok/developer-log.toml)
#    github_repo = "danmsheets-dev/turbo-field-logs"
#    github_sync = "manual"

# 3. Sync
turbo issues sync --repo danmsheets-dev/turbo-field-logs
```

The first sync to a private repo prints a warning. Local open → GH open;
ack → labels + open; resolve / wontdo → close + `resolved` / `declined`.
`turbo issues resolve <id> --sha <gitsha>` is posted as a proving-commit
comment. Pull maps GH close + those labels back onto local status.

Risks: GitHub search can lag (upsert also uses `fp:` labels + a local
`github-index.json`); `gh` secondary rate limits abort the rest of a CLI
run. `on-file` is fail-open.

## Related

- Live audit that motivated worktree recovery: [HYPER_DEVELOPER_FEEDBACK.md](./HYPER_DEVELOPER_FEEDBACK.md)
- Known issues / RC8 fixes: [KNOWN_ISSUES.md](./KNOWN_ISSUES.md)
- Human feedback channel remains `/feedback` (sentiment); ADL is for **engineering defects**.
