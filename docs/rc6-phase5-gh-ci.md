# RC6 Phase 5 — GitHub/CI Control Plane v1

## What shipped

Three thin tools in `crates/codegen/xai-grok-tools/src/implementations/grok_build/gh/mod.rs`
that wire the existing `gh` CLI — no new GitHub API client, no new dependencies.

### Tools

| Tool | Kind | Args | Reads stdout/stderr |
|------|------|------|-------------------|
| `gh_pr_status` | Read | `{pr?: string}` (default: detect current branch's PR) | `gh pr view --json number,title,state,statusCheckRollup,reviewDecision,url` |
| `gh_ci_status` | Read | `{run?: string}` (default: latest run on current branch) | `gh run list --limit 1 ...` then `gh run view <id>` |
| `gh_ci_rerun` | Execute | `{run: string, failed_only?: boolean}` | `gh run rerun <id> [--failed]` |

### Architecture

- **Spawn strategy**: shell-free `tokio::process::Command::new(gh_path)` with
  `--no-browser` flag, `Stdio::piped()` for stdout/stderr, 30s timeout via
  `tokio::time::timeout`.
- **Output capping**: captured stdout/stderr truncated to 60 KB (`GH_OUTPUT_CAP_BYTES`)
  with a `[truncated]` marker appended.
- **Error handling**: every error path returns
  `ToolError::custom(<code>, <actionable_message>)`. Missing `gh` → `gh_not_found`
  naming `gh auth login`. Auth failure → `gh_auth_failed` naming `gh auth login`.
  PR not found → `gh_pr_not_found`. Spawn failure → `gh_spawn_failed`. Timeout →
  `gh_timeout`.

### Integration points touched

1. **`implementations/grok_build/mod.rs`** — module declaration + pub use re-exports.
2. **`registry/types.rs::new()`** — three `b.register::<grok_build::XxxTool>()` calls
   following the `DeveloperLogTool` neighbor pattern.
3. **`types/tool_io.rs::ToolInput`** — three new variants:
   `GhPrStatus(GhPrStatusInput)`, `GhCiStatus(GhCiStatusInput)`, `GhCiRerun(GhCiRerunInput)`.
4. **`types/output.rs::ToolOutput`** — three new variants with `to_prompt_format` arms
   (delegating to `o.message`) and `is_error` arms (`!o.success`).
5. **`normalization.rs::canonical_input`** — three new variants added to the
   `_ => return None` catch-all (no canonical projection; falls to `None`).
6. **`reminders/task_completion.rs::consumed_completion_ids`** — three new output
   variants added to the `{}` no-op arm (no completion IDs to consume).

### Tool metadata

- Read tools: `ToolKind::Read`, `ToolNamespace::GrokBuild`, `tool_scope: Read`.
- Rerun tool: `ToolKind::Execute`, `ToolNamespace::GrokBuild`, `tool_scope: Write`,
  `is_read_only: false`.
- All use `#[tracing::instrument]` with fields matching the input.

## Deferred items (v1 slice)

- **Draft PR creation**: `gh pr create` integration (needs `--title`, `--body`,
  `--head`, `--base` args; draft support; base-branch detection).
- **GitLab pipeline_rerun**: `glab` CLI equivalent (same spawn pattern, different
  command surface).
- **Merge gating**: `gh pr merge --{merge,squash,rebase} --admin` with
  pre-flight checks (required reviews, passing CI, no conflicts).
- **Structured CI job list**: `gh run view --json jobs` to populate the
  `failing_jobs` field with real job-level data (v1 only surfaces check-run names
  from `statusCheckRollup` and optional `jobs`/`steps` arrays if present).

## Permission classification note

`From<&ToolInput> for AccessKind` lives in `xai-grok-workspace/src/permission/types.rs`
(outside this tool crate's allowed paths). Its match ends with:

```rust
_ => AccessKind::Read(None),
```

The three new `gh_*` input variants fall into this catch-all, so they are classified
as `AccessKind::Read(None)` — prompt-free, no permission gate. This means
`gh_ci_rerun` (`ToolKind::Execute`, mutating) will **not** prompt for bash-like
confirmation via the permission layer.

This is a **known limitation of v1** — the tool itself returns an error if `gh`
is missing/auth-broken, but the permission layer won't gate it. A follow-up PR
should add explicit arms in `From<&ToolInput> for AccessKind` to map `GhCiRerun`
to `AccessKind::Bash(...)` (or a new `AccessKind::GhRun` variant) so the
permission manager can prompt or deny rerun actions.

## Test evidence

```
running 34 tests
test implementations::grok_build::gh::tests::cap_bytes_no_truncation ... ok
test implementations::grok_build::gh::tests::cap_bytes_truncates ... ok
test implementations::grok_build::gh::tests::ci_status_message_formats_correctly ... ok
test implementations::grok_build::gh::tests::ci_status_no_failing_jobs_when_all_pass ... ok
test implementations::grok_build::gh::tests::ci_status_extracts_failing_jobs ... ok
test implementations::grok_build::gh::tests::gh_ci_rerun_command_exact_argv ... ok
test implementations::grok_build::gh::tests::gh_ci_rerun_command_uses_trimmed_run ... ok
test implementations::grok_build::gh::tests::gh_ci_rerun_command_without_failed_only ... ok
test implementations::grok_build::gh::tests::gh_ci_rerun_live ... ignored, requires gh CLI + GitHub auth + a real run ID
test implementations::grok_build::gh::tests::gh_ci_rerun_input_deserializes_failed_only_true ... ok
test implementations::grok_build::gh::tests::gh_ci_rerun_input_deserializes_failed_only_default_false ... ok
test implementations::grok_build::gh::tests::gh_ci_rerun_input_rejects_missing_run ... ok
test implementations::grok_build::gh::tests::gh_ci_status_command_with_run_id ... ok
test implementations::grok_build::gh::tests::gh_ci_status_command_without_run_id ... ok
test implementations::grok_build::gh::tests::gh_ci_status_live ... ignored, requires gh CLI + GitHub auth
test implementations::grok_build::gh::tests::gh_ci_status_input_deserializes_with_run ... ok
test implementations::grok_build::gh::tests::gh_ci_status_input_deserializes_without_run ... ok
test implementations::grok_build::gh::tests::gh_ci_status_tool_metadata ... ok
test implementations::grok_build::gh::tests::gh_pr_status_command_with_pr_number ... ok
test implementations::grok_build::gh::tests::gh_pr_status_command_without_pr_number ... ok
test implementations::grok_build::gh::tests::gh_pr_status_input_deserializes_serde_rename ... ok
test implementations::grok_build::gh::tests::gh_pr_status_live ... ignored, requires gh CLI + GitHub auth
test implementations::grok_build::gh::tests::gh_pr_status_input_deserializes_with_pr ... ok
test implementations::grok_build::gh::tests::gh_pr_status_input_deserializes_without_pr ... ok
test implementations::grok_build::gh::tests::gh_pr_status_tool_metadata ... ok
test implementations::grok_build::gh::tests::pr_status_handles_missing_fields ... ok
test implementations::grok_build::gh::tests::pr_status_handles_empty_checks ... ok
test implementations::grok_build::gh::tests::pr_status_message_formats_correctly ... ok
test implementations::grok_build::gh::tests::pr_status_message_includes_failing_checks ... ok
test implementations::grok_build::gh::tests::tally_checks_correctly ... ok
test implementations::grok_build::gh::tests::tally_checks_handles_none ... ok
test implementations::grok_build::gh::tests::truncate_for_display_truncates ... ok
test implementations::grok_build::gh::tests::truncate_for_display_within_limit ... ok

test result: ok. 31 passed; 0 failed; 3 ignored; 0 measured; 3022 filtered out; finished in 0.00s
```

`cargo check -p xai-grok-tools` also passes (Finished `dev` profile, no errors).
