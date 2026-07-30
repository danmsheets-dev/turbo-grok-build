# Reasoning effort ladder (`--reasoning-effort` / `--effort`)

Hyper accepts an explicit enumerated effort ladder on `--reasoning-effort`
(alias `--effort`). A typo fails fast with the full accepted set in the error
message (HYPER-2 — harnesses previously had no way to learn the ceiling and
capped themselves at `low|medium|high`).

## Ladder

| Token | Notes |
|---|---|
| `none` | No reasoning |
| `minimal` | Lowest non-zero tier |
| `low` | |
| `medium` | Default when a model supports effort and none is set |
| `high` | |
| `xhigh` | Canonical wire/id is `xhigh` |
| `max` | First-class max tier (Responses API / Codex) |
| `ultra` | Codex app-server tier that enables automatic subagent delegation |

## Wire mapping (do not change)

On Chat Completions requests, `xhigh` / `max` / `ultra` all serialize as
`"max"` so Moonshot/Kimi K3 (and OpenAI-style aliases) accept the field. The
enum’s global serialize form used for conversation persistence keeps the
distinct tokens. See `ReasoningEffort` in
`crates/codegen/xai-grok-sampling-types/src/types.rs`.

## Per-model support

Not every model accepts every tier. `hyper models --json` surfaces:

```json
{
  "id": "openai-codex/gpt-5.6-terra",
  "supportsReasoningEffort": true,
  "supportedEfforts": ["low", "medium", "high", "xhigh", "max"]
}
```

- `supportsReasoningEffort` — boolean from the platform registry
- `supportedEfforts` — ordered list of canonical tokens the model’s menu
  advertises (may be empty when the boolean is false)

Validate against this list before spending a run at a tier the route will
reject or silently ignore.

## Related

- Streaming JSON events: [`streaming-json-schema.md`](./streaming-json-schema.md)
- JSON Schema draft: [`streaming-json.schema.json`](./streaming-json.schema.json)
