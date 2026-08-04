# RC10 Harness Implementation Record

**Plan:** `RC10_HARNESS_FIX_PLAN.md` (Grok 4.5 explore audits 2026-08-02)  
**Version:** `0.2.114-r10`  
**Approach:** Linear implementation after plan subagents (shared workspace; land/diff + capability touch same modules).

## Shipped in this commit

| Wave | Item | Status | Primary touch |
|------|------|--------|---------------|
| A P0 | capability_mode stamp + no write re-inject | Done | `handle_request.rs`, `builder.rs` |
| A P1 | agent-only land/diff (baseline..snapshot) | Done | `subagent_worktree/{land,diff}.rs`, `subagent_cmd.rs` |
| B P1 | `--require-changes` harvest Edit start locs | Done | `headless.rs` |
| B P1 | ADL set-dir nest + layout, export honesty | Done | `developer-log/store.rs`, `export.rs` |
| B P1 | `turbo issues file` | Done | `issues_cmd.rs` |
| B | developer_log on default toolsets | Done | `agent/config.rs` |
| C P2 | Boot card Model field | Done | `PromptContext.model`, `AgentBuilder::with_session_model`, `agent_rebuild.rs` |
| C P2 | allowed_paths boot-card note (land/diff) | Done | `boot_card.rs` |

## Explicitly deferred (Wave C/D / follow-up)

- Write-time `allowed_paths` deny (product choice: document land/only for now)
- Doctor / catalog Ultra `agent_ready` reconcile
- Workflow validate Rhai hints, monitor Windows examples, prune UX
- Scheduler dirty-seed hardening beyond land/diff prefer-snapshot

## Acceptance retest (post-build)

See plan §9 matrix. User will build/install separately.

## Audit subagent IDs

See plan §11.
