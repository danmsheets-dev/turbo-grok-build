# Streaming JSON protocol (`--output-format streaming-json`)

Hyper emits **newline-delimited JSON** (NDJSON). Each line is one event object
with a `type` field. Harnesses should treat unknown types as ignorable for
forward compatibility.

## Guaranteed events

| Event | When | Guaranteed |
|---|---|---|
| `start` | First line of every streaming-json run | **Yes** |
| `end` | Successful terminal path | **Yes** (or `error` / `max_turns_reached`) |
| `error` | Auth / session / transport failure | When that path is taken |
| `max_turns_reached` | Turn budget exhausted | When that path is taken |
| `text` | Agent text chunks | Optional (model-dependent) |
| `thought` | Reasoning chunks | Optional |
| `tool_denied` | Headless permission refusal | When a tool is refused |
| `confine_violation` | Path outside `--confine` root | When confinement blocks |
| `model_resolved` | Served model differs from requested | When known after start |

Every **terminal** event (`end`, `error`, `max_turns_reached`) includes a
`usage` key: either a usage object, or `null` when the session ledger never
started. Partial ledgers set `"usageIsIncomplete": true`.

## `start` (schemaVersion 1)

```json
{
  "type": "start",
  "schemaVersion": 1,
  "sessionId": "…",
  "cwd": "…",
  "confineRoot": "…|null",
  "requestedModel": "grok-4.5",
  "servedModel": "grok-4.5-build|null",
  "permissionMode": "plan|default|auto|bypassPermissions",
  "sandbox": "read-only|null",
  "binary": "hyper",
  "version": "0.2.114-r5",
  "alwaysApprove": true
}
```

## `end`

```json
{
  "type": "end",
  "schemaVersion": 1,
  "stopReason": "…",
  "sessionId": "…",
  "requestId": "…",
  "usage": { "…": "…" } | null
}
```

## `tool_denied`

```json
{
  "type": "tool_denied",
  "tool": "grep",
  "reason": "no approval is possible in headless mode (permission mode: plan). …"
}
```

## `confine_violation`

```json
{
  "type": "confine_violation",
  "tool": "search_replace",
  "path": "C:\\outside\\file.rs",
  "root": "C:\\worktrees\\feat"
}
```

See also `docs/streaming-json.schema.json` for a JSON Schema draft of the event
union.
