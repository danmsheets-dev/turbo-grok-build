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
  "usage": { "…": "…" } | null,
  "filesChanged": {
    "count": 3,
    "paths": ["src/a.rs", "src/b.rs"],
    "truncated": false
  }
}
```

`filesChanged` lists paths the agent **edited/created/deleted through tools**
(ACP Edit-kind tool calls). Build outputs are not included. `paths` is capped
(200 entries / ~32 KB); `count` is the full unique set and `truncated` is true
when the list is a prefix.

With `--require-changes`, a successful run that changed nothing sets
`stopReason` to `NoChanges` and exits non-zero.

### Headless question recovery (HYPER-1)

A headless run (`-p` / `--prompt-file` / non-`plain` `--output-format`) injects
a non-negotiable system-prompt clause declaring there is **no interactive
user**. If the first turn still ends as a pure question (normal `EndTurn`,
**zero tool calls**, assistant text that reads as a question), Hyper:

1. Emits `{"type":"auto_continue","reason":"headless_question","attempt":1}`
2. Injects one internal nudge (“assume no to optional features; proceed”)
3. Bounds the recovery to **one** auto-continue — never a loop

If the second turn also makes no tool calls, the terminal event uses
`stopReason: "AwaitingUserInput"` (not `EndTurn`) with `filesChanged.count: 0`,
and the process exits non-zero. Harnesses that key `completed-blind` off a
clean `EndTurn` will correctly treat a question-only run as a failure.

Opt out only with `--allow-interactive-questions` (rare).

## `auto_continue`

```json
{
  "type": "auto_continue",
  "reason": "headless_question",
  "attempt": 1
}
```

Emitted at most once per run when the headless question-recovery path fires.

## `tool_denied`

```json
{
  "type": "tool_denied",
  "tool": "grep",
  "reason": "no approval is possible in headless mode (permission mode: plan). …"
}
```

## `confine_violation` (schemaVersion 1)

Emitted when `--confine` / `--workspace-root` blocks a write (Edit tool or
shell write/`cd` operand) that resolves outside the root. Harnesses count
these to detect attempted escapes without diffing the filesystem afterwards.

```json
{
  "type": "confine_violation",
  "tool": "write",
  "path": "C:/…/Main Repo/d.txt",
  "resolvedPath": "C:/…/Main Repo/d.txt",
  "root": "C:/…/wt",
  "schemaVersion": 1
}
```

| Field | Meaning |
|---|---|
| `tool` | Tool name that requested the write (e.g. `write`, `search_replace`, `bash`) |
| `path` | Operand as supplied by the model / shell |
| `resolvedPath` | Canonical form after ancestor walk (8.3, `..`, symlinks) |
| `root` | Canonical confine root |
| `schemaVersion` | `1` |

See also `docs/streaming-json.schema.json` for a JSON Schema draft of the event
union.
