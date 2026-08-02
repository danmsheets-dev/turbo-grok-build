# Streaming JSON protocol (`--output-format streaming-json`)

Hyper emits **newline-delimited JSON** (NDJSON). Each line is one event object
with a `type` field.

## Contract for consumers (harnesses / bridges)

1. **Pin on `schemaVersion`.** The `start` and `end` events carry
   `schemaVersion` (currently **2**). New fields may be added under the same
   version; breaking renames bump the version.
2. **Unknown event types are ignorable.** A consumer MUST NOT fail the run when
   it sees a `type` it does not recognize. Collect them (e.g. `unknownTypes`)
   for diagnostics; continue parsing known types on subsequent lines.
3. **Terminal events always include `usage`.** Either a usage object or
   `null` when the session ledger never started. Partial ledgers may set
   `"usageIsIncomplete": true`.
4. **Tool progress is first-class.** `tool_call` / `tool_call_update` /
   `tool_result` are the authority for “what the agent is doing.” Do not
   treat a long silence as a hung model when a prior `tool_call` has not yet
   received a terminal `tool_result`.

See also `docs/streaming-json.schema.json` for a JSON Schema draft of the event
union.

## Guaranteed events

| Event | When | Guaranteed |
|---|---|---|
| `start` | First line of every streaming-json run | **Yes** |
| `end` | Successful terminal path | **Yes** (or `error` / `max_turns_reached`) |
| `error` | Auth / session / transport failure | When that path is taken |
| `max_turns_reached` | Turn budget exhausted | When that path is taken |
| `text` | Agent text chunks | Optional (model-dependent) |
| `thought` | Reasoning chunks | Optional |
| `tool_call` | Tool invocation observed | When the agent calls a tool |
| `tool_call_update` | In-progress tool status / partial I/O | Optional (streaming tools) |
| `tool_result` | Tool reached Completed/Failed | When a tool finishes |
| `tool_denied` | Headless permission refusal | When a tool is refused |
| `confine_violation` | Path outside `--confine` root | When confinement blocks |
| `subagent_spawned` | Background/foreground subagent start | When a subagent is spawned |
| `subagent_finished` | Subagent terminal status | When a subagent ends |
| `question_suppressed` | Interactive question blocked headlessly | When `ask_user_question` fires |
| `warning` | Non-fatal advisory (trust, rules re-sync, …) | When something was degraded |
| `model_resolved` | Served model differs from requested | When known after start |
| `auto_continue` | HYPER-1 question recovery | At most once per run |
| `auto_compact_*` | Compaction lifecycle | When compaction runs |

Every **terminal** event (`end`, `error`, `max_turns_reached`) includes a
`usage` key: either a usage object, or `null` when the session ledger never
started. Partial ledgers set `"usageIsIncomplete": true`.

## `start` (schemaVersion 2)

```json
{
  "type": "start",
  "schemaVersion": 2,
  "sessionId": "…",
  "cwd": "…",
  "sessionCwd": "…",
  "originalCwd": "…|omitted",
  "confineRoot": "…|null",
  "confineInherited": false,
  "confineShellEnforcement": "…|null",
  "requestedModel": "grok-4.5",
  "servedModel": "grok-4.5-build|null",
  "permissionMode": "plan|default|auto|bypassPermissions",
  "sandbox": "read-only|null",
  "binary": "turbo",
  "version": "0.2.114-r5",
  "alwaysApprove": true,
  "rulesApplied": true,
  "folderTrust": {
    "trusted": false,
    "key": "…",
    "reason": "store|no-configs|unrecordable-root|untrusted-headless|feature-off|inert-build",
    "droppedMcpServers": ["blender"],
    "droppedHooks": 0,
    "droppedPlugins": 0,
    "droppedAgents": 0,
    "configKinds": ["mcp", "hooks"]
  }
}
```

| Field | Meaning |
|---|---|
| `cwd` | Process cwd at launch (where the binary was started) |
| `sessionCwd` | Directory the ACP session actually uses for relative paths (may differ on cross-dir `--resume`) |
| `originalCwd` | Session's recorded origin when known (resume/fork) |
| `rulesApplied` | Whether `--rules` / `--append-system-prompt` was provided (applied on new and re-synced on resume) |
| `folderTrust` | Trust verdict and names of project-scoped capabilities dropped when untrusted |
| `confineRoot` | Absolute path root of `--confine`, or null when unconfined |
| `confineInherited` | True when this process inherited `GROK_CONFINE` from a parent |
| `confineShellEnforcement` | Under confine: `fail-closed` (default — unknown programs denied) or `operand-scan` (legacy write-operand allowlist only). **Not an OS sandbox** — path-prefix + classifier only; full Landlock/AppContainer is out of scope |

## `tool_call` / `tool_call_update` / `tool_result`

```json
{
  "type": "tool_call",
  "schemaVersion": 2,
  "toolCallId": "call_…",
  "name": "bash",
  "kind": "execute",
  "status": "in_progress",
  "title": "Bash: cargo test",
  "locations": [{ "path": "…", "line": 1 }],
  "rawInput": { "…": "…" },
  "rawInputTruncated": false,
  "elapsedMs": 0
}
```

```json
{
  "type": "tool_result",
  "schemaVersion": 2,
  "toolCallId": "call_…",
  "status": "completed",
  "elapsedMs": 1234,
  "rawOutput": { "…": "…" },
  "rawOutputTruncated": true
}
```

- `tool_call` is emitted when the invocation is first observed.
- `tool_call_update` is emitted for non-terminal status/I/O updates.
- `tool_result` is emitted when status is `completed` or `failed`.

Raw I/O is gated by `--stream-tool-io=none|truncated|full` (default `truncated`,
~2 KB). When truncated, `rawInputTruncated` / `rawOutputTruncated` is `true`.

## `subagent_spawned` / `subagent_finished`

```json
{
  "type": "subagent_spawned",
  "schemaVersion": 2,
  "subagentId": "…",
  "childSessionId": "…",
  "subagentType": "explore",
  "description": "…",
  "model": "…",
  "capabilityMode": "read-only"
}
```

```json
{
  "type": "subagent_finished",
  "schemaVersion": 2,
  "subagentId": "…",
  "childSessionId": "…",
  "status": "completed|failed|cancelled",
  "error": "…|omitted",
  "terminationReason": "max_tool_calls|…|omitted",
  "usage": { "…": "…" },
  "toolCalls": 12,
  "turns": 3,
  "durationMs": 45000,
  "tokensUsed": 8000
}
```

## `question_suppressed`

Emitted when the model attempts `ask_user_question` (or similar) in headless
mode. The reverse-request is answered immediately with `Cancelled` so the run
cannot block for up to 30 minutes.

```json
{
  "type": "question_suppressed",
  "schemaVersion": 2,
  "toolCallId": "…",
  "reason": "headless: ask_user_question is disabled; no interactive user"
}
```

## `warning`

Non-fatal advisory. Codes currently include:

| Code | Meaning |
|---|---|
| `rules_resynced_on_resume` | `--rules` re-applied on resume/continue |
| `folder_trust_untrusted` | Project capabilities dropped (untrusted folder) |
| `plan_approval_suppressed` | `exit_plan_mode` cancelled headlessly |

```json
{
  "type": "warning",
  "schemaVersion": 2,
  "code": "folder_trust_untrusted",
  "message": "…"
}
```

## `end`

```json
{
  "type": "end",
  "schemaVersion": 2,
  "stopReason": "…",
  "sessionId": "…",
  "requestId": "…",
  "usage": { "…": "…" } | null,
  "toolCalls": 12,
  "filesChanged": {
    "count": 3,
    "paths": ["src/a.rs", "src/b.rs"],
    "truncated": false
  },
  "subagents": {
    "spawned": 2,
    "completed": 1,
    "failed": 1,
    "cancelled": 0
  }
}
```

| Field | Meaning |
|---|---|
| `toolCalls` | Total tool invocations observed this run (not reset on auto-continue) |
| `subagents` | Rollup of subagent lifecycle outcomes |
| `filesChanged` | Paths the agent **edited/created/deleted through tools** (ACP Edit-kind). Build outputs are not included. `paths` is capped (200 entries / ~32 KB); `count` is the full unique set and `truncated` is true when the list is a prefix. |

### Stop reasons harnesses should key on

| `stopReason` | Meaning |
|---|---|
| `EndTurn` | Normal completion |
| `NoChanges` | `--require-changes` and zero agent edits (non-zero exit) |
| `SubagentFailure` | `--require-subagent-success` and at least one failed subagent (non-zero exit) |
| `AwaitingUserInput` | Question-only turn after one auto-continue (non-zero exit) |

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

Opt out only with `--allow-interactive-questions` (rare). That also re-enables
the `ask_user_question` tool; without it the tool is default-disabled and any
ext-method is answered with `question_suppressed` + Cancelled.

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

## CLI flags that affect the stream

| Flag | Effect |
|---|---|
| `--stream-tool-io=none\|truncated\|full` | Raw tool I/O on the stream (default `truncated`) |
| `--require-changes` | `stopReason: NoChanges` + non-zero if no edits |
| `--require-subagent-success` | `stopReason: SubagentFailure` + non-zero if any subagent failed |
| `--require-trust` | Fail before prompt if workspace untrusted |
| `--trust` | Grant folder trust for this cwd (project MCP/hooks/… allowed) |
| `--rules` / `--append-system-prompt` | Applied on new sessions; re-synced on resume |
| `--worktree` | **Rejected in headless** with a clear error (create worktree + `--confine` instead) |
| `--allow-interactive-questions` | Opt out of no-questions clause; re-enable ask tool |

See also `docs/streaming-json.schema.json` for a JSON Schema draft of the event
union.
