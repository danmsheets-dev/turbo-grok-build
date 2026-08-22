# RC6 Phase 4 — NVIDIA catalog / spawn compatibility

Gates NVIDIA Chat Completions models so spawn cannot pick hang/EOL/chat-only
rows as general-purpose agents, and so Integrate requests do not send Grok's
internal `model_id` field.

## What shipped

### `agent_ready` spawn gate (FR `fr_01a028cf0ade7e809fbc5601b0c92136`)

- Catalog `OpenAiCompletionsCompat.agent_ready` remains the source of truth.
  NVIDIA Chat Completions still defaults to `false` (platform override).
- Write-capable spawn (`general-purpose`, `xdotcom`, user-defined types, or
  explicit `capability_mode` of `read-write` / `execute` / `all`) is rejected
  with a message that lists agent-ready vs chat-only slugs.
- `explore` / `plan` / `oracle` and `capability_mode=read-only` may still pin
  chat-only NVIDIA models.
- Default `spawn_subagent` advertised list is agent-ready, except credentialed
  `[model.*]` entries are also surfaced for discoverability. Uncertified NVIDIA
  config entries fail closed as chat-only and are still rejected for write-capable
  spawn. Builtin hang / Ultra / small-Llama rows stay out of the advertised list.

### NVIDIA `model_id` extra_forbidden (inc `inc_01a028c941bc7133ae53d6e6ea1744b6`)

- New compat flag `supports_message_model_id` (default `true`).
- NVIDIA Integrate forces `false`. Sampler `patch_chat_request_body` strips
  top-level and per-message `model_id` before the request hits Integrate.
- Unit: `nvidia_chat_compat_strips_extra_model_id` in `xai-grok-sampler`.

### GLM-5.2 EOL 410 (inc `inc_01a028c99f9d7f61bc8a9b7b770fede0`)

- Historical snapshot row `nvidia/z-ai/glm-5.2` stays in `platform_catalog.json`
  with `"eol": true`. Not deleted.
- `catalog_available` / `picker_visible` are false. Spawn and slug validation
  return an HTTP 410-class error. OpenRouter `z-ai/glm-5.2` is not treated as
  NVIDIA EOL.

### Hang / Ultra / small Llamas (incs hang + Ultra 550B + small Llama tools)

Kept `agent_ready=false` (already the NVIDIA platform default):

| Catalog key | Why chat-only |
|---|---|
| `nvidia/meta/llama-3.3-70b-instruct` | Hang until spawn timeout, zero tools |
| `nvidia/openai/gpt-oss-120b` | Hang until spawn timeout, zero tools |
| `nvidia/nvidia/nemotron-3-ultra-550b-a55b` | Stream dies `internal_server_error` after 5+ min |
| `nvidia/meta/llama-3.1-8b-instruct` (and other small Llamas) | Complete without native `tool_calls` |

These are excluded from the **default** (agent-ready) spawn list and documented
as chat-only. They remain in the offline catalog for `/model` chat.

### config.toml NVIDIA extras (FR `fr_01a028c823647c20994104c402efd08f`)

`spawnable_task_model_catalog` includes `[model.*]` keys that have credentials
even when the builtin row was a hidden alias. Chat-only extras are included in
the advertised list so they are not silently dropped, but remain classified in
the catalog's `chat_only` set; write-capable spawn rejects them.

NVIDIA extras without `request_compat` fail closed (`agent_ready=false`).

### `openai/gpt-5.5` (inc `inc_01a025f200857c8088810134f4918b5e`)

Already working in rc5. `task_model_aliases` maps `openai/gpt-5.5` →
`openai-codex/gpt-5.5`. Additional unit test in
`crates/codegen/xai-grok-shell/src/agent/models/tests.rs`
(`gpt55_openai_slash_slug_aliases_to_openai_codex`). Do not regress.

## Tests

Cargo filters are **substrings**, not regex. Each command passed separately on
Windows with `--test-threads=2`:

```powershell
cargo test -p xai-grok-models --lib nvidia_glm -- --test-threads=2
cargo test -p xai-grok-models --lib nvidia_hang -- --test-threads=2
cargo test -p xai-grok-models --lib nvidia_fallback -- --test-threads=2
cargo test -p xai-grok-sampler --lib nvidia_chat_compat_strips -- --test-threads=2
cargo test -p xai-tool-types --lib spawn_requires_agent_ready -- --test-threads=2
cargo test -p xai-grok-tools --lib agent_ready_gate -- --test-threads=2
cargo test -p xai-grok-tools --lib invalid_model_returns -- --test-threads=2
cargo test -p xai-grok-shell --lib gpt55_openai_slash -- --test-threads=2
cargo test -p xai-grok-shell --lib glm_52_nvidia -- --test-threads=2
cargo test -p xai-grok-shell --lib agent_ready_false_rejected -- --test-threads=2
cargo test -p xai-grok-shell --lib config_nvidia_extras -- --test-threads=2
```

Result before the final catalog-classification cleanup: 11/11 commands passed,
13 tests passed, 0 failed. After that cleanup, `config_nvidia_extras` and
`agent_ready_false_rejected` were each rerun separately and passed (2 tests,
0 failed total).

Final compilation passed:

```powershell
cargo check -p xai-grok-models -p xai-grok-sampler -p xai-tool-types -p xai-grok-tools -p xai-grok-shell
```

Independent review was attempted three times but could not complete: two review
subagents became unqueryable and one stalled before producing output. These
harness failures are logged as `inc_01a0296ee6097532b321a0e24fc37eee` and
`inc_01a02972d18d7782890f15ef36039b9e`; workflow fallback is blocked for child
sessions (`fr_01a0297337c373719810c4e426e2a66b`).

## Allowed-paths residuals (this worktree)

Phase 4 land prefixes did not include `xai-grok-agent`,
`xai-grok-shell/src/session`, `xai-grok-shell/src/agent/config.rs`, or
`xai-grok-shell/src/agent/subagent`. Those stay parent-HEAD:

- Tool description has no separate **Chat-only:** section (hang models are
  simply omitted from the advertised list). Config extras still appear.
- `handle_request` slug check is EOL/unknown only; write-capable `agent_ready`
  is enforced at `TaskTool` (which sees `subagent_type`).
- `[model.*]` overlay does not auto-unhide alias rows in the picker; spawn
  catalog still surfaces credentialed config keys.

## Policy helpers

- `xai_tool_types::spawn_requires_agent_ready`
- `xai_tool_types::is_chat_only_agent_ready_error`
- `xai_grok_models::catalog_key_is_eol` / `is_nvidia_glm_52_eol_slug`
- `xai_grok_models::nvidia_integrate_chat_compat`
