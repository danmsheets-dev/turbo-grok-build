# Feature Request Log (FRL)

Structured **product capability request** pipeline for Turbo agents and
maintainers — the twin of [Auto Developer Log](./AUTO_DEVELOPER_LOG.md).

When harness agents notice a **missing product surface** (no tool, no workflow
hook, no keep-N scheduler, no land merge helper), they file a **deduplicated,
redacted feature request** under the configured log root (default
`$GROK_HOME/feature-request-log/`). Product uses this as field demand signal —
not chat archaeology.

| Use | Tool | CLI |
|-----|------|-----|
| Bugs / friction / broken behavior | `developer_log` | `turbo issues …` |
| Missing capability agents need | `feature_request_log` | `turbo features …` |

## Quick start

```bash
# List open requests
turbo features list

# Show one request
turbo features show fr_<id>

# Export a maintainer pack
turbo features export
turbo features export --priority must_have --out ./fr-pack

# Paths / status
turbo features path
turbo features ack <id>
turbo features plan <id>
turbo features ship <id>
turbo features ship <id> --sha <gitsha> --note "landed in rc6"
turbo features decline <id>

# Configure storage
turbo features set-dir D:/HyperLogs/feature-request-log
turbo features clear-dir

# Opt-in GitHub Issues sync (never the default)
turbo features sync --repo owner/name
turbo features sync --push
turbo features sync --pull
```

Disable writes:

```bash
# Unix
export GROK_FEATURE_REQUEST_LOG=0

# Windows PowerShell
$env:GROK_FEATURE_REQUEST_LOG = "0"
```

### Configuring the log directory

| Precedence | Source |
|------------|--------|
| 1 (highest) | Process override after `turbo features set-dir` this session |
| 2 | Env `GROK_FEATURE_REQUEST_LOG_DIR` |
| 3 | File `$GROK_HOME/feature-request-log.toml` → `dir = "..."` |
| 4 (default) | `$GROK_HOME/feature-request-log` |

Example `~/.grok/feature-request-log.toml`:

```toml
dir = "D:\\HyperLogs\\feature-request-log"
# Opt-in GitHub Issues inbox (default off — no cloud upload)
# github_sync = "off" | "manual" | "on-file"
github_repo = "danmsheets-dev/turbo-field-logs"
github_sync = "off"
```

`set-dir` / `clear-dir` preserve `github_repo` and `github_sync`.

## GitHub Issues sync (opt-in)

Local JSON is the write-ahead log. `turbo features sync` mirrors one GitHub
issue per fingerprint (`type:feature`). Default is local-only. Auth is `gh`.
Agent `feature_request_log` never waits on the network; `github_sync = "on-file"`
is a fail-open background push after the local write.

Same private-repo flow as [Auto Developer Log](./AUTO_DEVELOPER_LOG.md): set
`github_repo`, run `gh auth login`, then `turbo features sync`. First private
sync warns. Local planned/ack stay open with labels; ship/decline close the
issue (`shipped` / `declined`). `turbo features ship <id> --sha` posts a
proving-commit comment. Pull maps those closes back onto local status.

## Storage layout

```text
<log-root>/                         # default: $GROK_HOME/feature-request-log
  index.json
  events.jsonl
  requests/
    YYYY-MM-DD/
      fr_<uuid>.json
  bundles/
    export-<timestamp>/
      summary.md
      requests.ndjson
      fingerprints.csv
      manifest.json
      evidence/
```

## Agent tool: `feature_request_log`

| Field | Required | Notes |
|-------|----------|--------|
| `title` | yes | Short capability title |
| `summary` | yes | 1–3 sentences |
| `request_class` | yes | Stable taxonomy (below) |
| `priority` | no | `must_have` … `exploratory` (defaults from class) |
| `use_case` | no | Concrete harness scenario |
| `current_workaround` | no | What agents do today |
| `proposed_behavior` | no | Desired product shape |
| `acceptance_criteria` | no | Bullets |
| `component` / `tags` | no | Product areas |
| `fingerprint` | no | Prefer auto-compute |

Rules:

- Call when work is blocked by a **missing** product surface (not a bug).
- One call per distinct request; store **dedups by fingerprint**.
- Never log secrets or full prompts.

## Request class taxonomy

| Class | Meaning | Default priority |
|-------|---------|------------------|
| `tool_surface` | Missing / incomplete agent tool | should_have |
| `workflow` | Workflow / orchestration | should_have |
| `subagent` | Spawn, resume, isolation, land | should_have |
| `ui_ux` | TUI / Game Mode / keys | nice_to_have |
| `provider_model` | Routing / catalog | should_have |
| `mcp_integration` | MCP product-side | should_have |
| `documentation` | Docs / boot card / CLI | nice_to_have |
| `performance` | Scale / concurrency | should_have |
| `api_surface` | Config / flags / API | nice_to_have |
| `scheduler` | Keep-N / automation loops | nice_to_have |
| `extensibility` | Plugins / skills / memory | nice_to_have |
| `other` | Catch-all | nice_to_have |

## Status lifecycle

`open` → `acknowledged` → `planned` → `shipped`  
or `declined`.

## Privacy

- Secrets and home-path segments are redacted (same sanitizers as ADL).
- **No cloud upload by default.** GitHub sync is opt-in. Missing `gh` or
  unauthenticated `gh` fails `turbo features sync`. Agent tools never block
  on the network.

## Related

- Auto Developer Log: `docs/AUTO_DEVELOPER_LOG.md`
- Boot card: `crates/codegen/xai-grok-agent/src/prompt/boot_card.rs`
