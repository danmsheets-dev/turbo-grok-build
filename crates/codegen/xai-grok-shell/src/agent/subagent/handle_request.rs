use super::*;
use crate::upload::trace::GCS_SCHEMA_VERSION;
use xai_grok_sampling_types::ReasoningEffort;
use xai_grok_tools::implementations::{grok_build, opencode};
pub(super) fn canonical_total_tokens(totals: &xai_chat_state::UsageTotals) -> u64 {
    totals.total_tokens()
}
pub(super) fn usage_is_incomplete(
    ledger_incomplete: bool,
    cancellation_may_hide_usage: bool,
    _known_total_tokens: u64,
    _has_usage_entries: bool,
) -> bool {
    ledger_incomplete || cancellation_may_hide_usage
}
pub(super) async fn record_subagent_usage(
    parent_cmd_tx: Option<&mpsc::UnboundedSender<SessionCommand>>,
    by_model: Option<Vec<(String, xai_chat_state::UsageTotals)>>,
    parent_prompt_id: Option<String>,
    incomplete: bool,
) -> bool {
    match by_model {
        None => false,
        Some(by_model) if by_model.is_empty() && !incomplete => true,
        Some(by_model) => {
            let Some(cmd_tx) = parent_cmd_tx else {
                return false;
            };
            let (respond_to, ack) = oneshot::channel();
            if cmd_tx
                .send(SessionCommand::RecordSubagentUsage {
                    by_model,
                    parent_prompt_id,
                    incomplete,
                    respond_to,
                })
                .is_err()
            {
                return false;
            }
            ack.await.is_ok()
        }
    }
}
pub(super) fn task_model_override_error(
    requested: Option<&str>,
    provenance: ModelOverrideProvenance,
    is_resume: bool,
    available: &indexmap::IndexMap<String, crate::agent::config::ModelEntry>,
    is_session_auth: bool,
) -> Option<String> {
    if provenance != ModelOverrideProvenance::Tool || is_resume {
        return None;
    }
    let requested = requested?;
    crate::agent::models::task_model_error_for_catalog(requested, available, is_session_auth)
}

/// Pin cargo's native target dir to the real worktree path.
///
/// Inherited `CARGO_TARGET_DIR` (or DisplayCwd remapping onto the parent
/// `H:\…\target`) mixes CMake's worktree path with MSBuild FileTracker tlogs
/// under the parent target and fails FTK1011 on Windows. Always overwrite —
/// the child must not share the parent's target.
pub(super) fn inject_worktree_cargo_env(
    env: &mut std::collections::HashMap<String, String>,
    worktree: &std::path::Path,
) {
    let target = worktree.join("target");
    let target_s = target.to_string_lossy().into_owned();
    env.insert("CARGO_TARGET_DIR".into(), target_s.clone());
    env.insert("GROK_WORKTREE_CARGO_TARGET".into(), target_s);
}

/// Ensure densify engines (Blender / Godot) resolve on Windows for confined
/// worktree children. Does not override values already present in the env map
/// or process environment (parent wins).
pub(super) fn inject_densify_engine_env(env: &mut std::collections::HashMap<String, String>) {
    const BLENDER_KEYS: &[&str] = &["BLENDER_PATH", "GROK_BLENDER", "GROK_BLENDER_PATH"];
    const GODOT_KEYS: &[&str] = &["GODOT_PATH", "GROK_GODOT", "GROK_GODOT_PATH"];

    if !env_map_or_process_has(env, BLENDER_KEYS) {
        if let Some(path) = discover_blender_exe() {
            env.insert("BLENDER_PATH".into(), path.clone());
            env.insert("GROK_BLENDER".into(), path);
        }
    }
    if !env_map_or_process_has(env, GODOT_KEYS) {
        if let Some(path) = discover_godot_exe() {
            env.insert("GODOT_PATH".into(), path.clone());
            env.insert("GROK_GODOT".into(), path);
        }
    }
}

fn env_map_or_process_has(env: &std::collections::HashMap<String, String>, keys: &[&str]) -> bool {
    keys.iter().any(|k| {
        env.get(*k).is_some_and(|v| !v.trim().is_empty())
            || std::env::var(k).is_ok_and(|v| !v.trim().is_empty())
    })
}

fn discover_blender_exe() -> Option<String> {
    // Explicit env already handled by caller; probe well-known install paths.
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let base = std::path::PathBuf::from(pf).join("Blender Foundation");
        // Prefer newest version directory when present.
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut versions: Vec<std::path::PathBuf> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            versions.sort();
            versions.reverse();
            for dir in versions {
                candidates.push(dir.join("blender.exe"));
                candidates.push(dir.join("blender"));
            }
        }
    }
    // Common fixed paths (Windows densify machines).
    candidates.push(std::path::PathBuf::from(
        r"C:\Program Files\Blender Foundation\Blender 5.2\blender.exe",
    ));
    candidates.push(std::path::PathBuf::from(
        r"C:\Program Files\Blender Foundation\Blender 4.5\blender.exe",
    ));
    candidates.push(std::path::PathBuf::from(
        r"C:\Program Files\Blender Foundation\Blender 4.2\blender.exe",
    ));
    // PATH lookup
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            candidates.push(dir.join("blender.exe"));
            candidates.push(dir.join("blender"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
}

fn discover_godot_exe() -> Option<String> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let base = std::path::PathBuf::from(pf).join("Godot");
        candidates.push(base.join("Godot_v4.exe"));
        candidates.push(base.join("godot.exe"));
        candidates.push(base.join("Godot_console.exe"));
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            candidates.push(dir.join("godot.exe"));
            candidates.push(dir.join("godot"));
            candidates.push(dir.join("Godot_console.exe"));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
}
/// Runtime adapter for one shell child. Shared lifecycle state is owned by the
/// `xai-grok-tools` coordinator actor and reached only through `reporter`.
#[tracing::instrument(
    name = "subagent.handle_request",
    skip_all,
    fields(
        subagent_id = %run.request.id,
        parent_session_id = %ctx.parent_session_id,
        subagent_type = %run.request.subagent_type,
    )
)]
pub(crate) async fn run_shell_child(
    run: grok_build::task::coordinator::ChildRunRequest<ShellChildRuntime>,
    mut ctx: SubagentSpawnContext,
    gateway: &GatewaySender,
) -> ChildRunOutput<ShellCompletionData> {
    let grok_build::task::coordinator::ChildRunRequest {
        mut request,
        cancellation: cancel_token,
        reporter,
        queued_for,
        session_running,
    } = run;
    let start = std::time::Instant::now();
    let mut completion_data = ShellCompletionData::from_context(&ctx);
    // Defense in depth: `request.id` is joined into worktree paths,
    // `subagents/<id>/meta.json`, and session dirs. The Task tool gates
    // model `task_id`, but every join site must fail closed in case another
    // constructor or peer path supplies a hostile id.
    if !xai_tool_types::is_safe_task_id(&request.id) {
        let msg = format!(
            "subagent id `{}` is not a valid path segment (letters, digits, `-`, `_`, `.` only; \
             no separators, drive prefixes, Windows device names, or trailing dot/space)",
            request.id
        );
        return child_run_output(failure_result(&request, &msg), completion_data, None);
    }
    if request.owner.is_workflow() && cancel_token.is_cancelled() {
        return child_run_output(
            cancelled_result(&request, "Subagent was cancelled"),
            completion_data,
            None,
        );
    }
    let Some(mut definition) = resolve_agent_definition(&request.subagent_type, &ctx) else {
        let msg = format!("Unknown subagent type: {}", request.subagent_type);
        return child_run_output(failure_result(&request, &msg), completion_data, None);
    };
    match gate_subagent_type(&request.subagent_type, &ctx) {
        SubagentValidateTypeOutcome::Disabled => {
            let msg = format!(
                "Subagent '{}' is disabled via [subagents.toggle] in config.toml",
                request.subagent_type
            );
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
        SubagentValidateTypeOutcome::NotAllowed { allowed } => {
            let msg = format!(
                "agent can only spawn: {}; '{}' not allowed",
                allowed.join(", "),
                request.subagent_type
            );
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
        SubagentValidateTypeOutcome::Unknown { .. }
        | SubagentValidateTypeOutcome::ValidationUnavailable => {
            let msg = format!("Cannot validate subagent '{}'", request.subagent_type);
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
        SubagentValidateTypeOutcome::Ok => {}
        _ => {
            let msg = format!("Cannot validate subagent '{}'", request.subagent_type);
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
    }
    resolve_subagent_toolset(
        &request.subagent_type,
        request.runtime_overrides.harness_agent_type.as_deref(),
        &ctx,
        &mut definition,
    );
    let cwd = ctx
        .parent_session_info
        .as_ref()
        .map(|i| std::path::Path::new(&i.cwd));
    let config_effort_pin = ctx
        .subagent_effort_overrides
        .get(&request.subagent_type)
        .and_then(|raw| {
            raw.parse()
                .map_err(|err| {
                    tracing::warn!(
                        value = %raw,
                        error = %err,
                        "[subagents.effort] pin has an invalid value, ignoring"
                    );
                })
                .ok()
        });
    let mut effective_runtime = xai_grok_subagent_resolution::resolve_runtime_config(
        &request.subagent_type,
        &request.runtime_overrides,
        &ctx.subagent_roles,
        &ctx.subagent_personas,
        cwd,
        &definition,
        config_effort_pin,
    );
    let prompt = request.prompt.clone();
    if let Some(ref err) = effective_runtime.persona_error {
        tracing::error!(
            subagent_id = %request.id,
            error = err,
            "Persona resolution failed, aborting subagent spawn"
        );
        return child_run_output(failure_result(&request, err), completion_data, None);
    }
    if let Some(ref warn) = effective_runtime.role_prompt_warning {
        tracing::warn!(
            subagent_id = %request.id,
            warning = warn,
            "Role prompt_file degraded, continuing without role prompt"
        );
    }
    let resume_source = if let Some(resume_id) = request
        .resume_from
        .as_deref()
        .filter(|s| is_valid_resume_id(s))
    {
        match reporter
            .resume_source(resume_id, &ctx.parent_session_id)
            .await
        {
            SubagentResumeLookup::Active => {
                let msg = format!(
                    "Cannot resume from subagent '{resume_id}': it is still running. \
                     Wait for it to complete before resuming."
                );
                return child_run_output(failure_result(&request, &msg), completion_data, None);
            }
            SubagentResumeLookup::Completed(info) => Some(ResumeSourceData {
                subagent_id: info.subagent_id,
                child_session_id: info.child_session_id,
                child_cwd: info.child_cwd,
                worktree_path: info.worktree_path.map(PathBuf::from),
                snapshot_ref: info.snapshot_ref,
                subagent_type: info.subagent_type,
                persona: info.persona,
                model_id: info.model_id,
            }),
            SubagentResumeLookup::Missing => {
                match durable_resume_source_for(resume_id, &ctx.parent_session_id, &ctx.parent_cwd)
                {
                    Some(info) => Some(info),
                    None => {
                        let msg = format!(
                            "Cannot resume from subagent '{resume_id}': not found. \
                             The subagent may have been evicted or the ID is invalid."
                        );
                        return child_run_output(
                            failure_result(&request, &msg),
                            completion_data,
                            None,
                        );
                    }
                }
            }
        }
    } else {
        None
    };
    if let Some(ref source) = resume_source {
        if request.runtime_overrides.model.is_some() {
            tracing::debug!(
                subagent_id = %request.id,
                "Ignoring caller model override on resume; source model will be pinned"
            );
        }
        effective_runtime.model = None;
        if let Err(e) = xai_grok_subagent_resolution::validate_resume_identity(
            &request.subagent_type,
            request.runtime_overrides.persona.as_deref(),
            source,
        ) {
            return child_run_output(
                failure_result(&request, &e.to_string()),
                completion_data,
                None,
            );
        }
    }
    if let Some(error) = task_model_override_error(
        request.runtime_overrides.model.as_deref(),
        request.runtime_overrides.model_override_provenance,
        resume_source.is_some(),
        &ctx.available_models,
        ctx.auth_manager
            .current_or_expired()
            .is_some_and(|a| a.is_session_auth()),
    ) {
        return child_run_output(failure_result(&request, &error), completion_data, None);
    }
    // R6-10: isolation is fail-closed. When isolation=worktree was requested and
    // the worktree cannot be created/rehydrated, the subagent does NOT start in
    // the shared workspace unless the operator explicitly opts in via
    // GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 (which also sets isolation_fallback
    // on the result so harnesses refuse to report the run as isolated).
    let isolation_requested =
        effective_runtime.isolation != xai_tool_types::SubagentIsolationMode::None;
    let allow_shared_fallback = isolation_shared_fallback_allowed();
    let mut isolation_fallback = false;
    // Seed mode for honesty tags (clean = HEAD-only; dirty = parent WIP).
    // Set on fresh worktree create; resume inherits source meta when present.
    let mut worktree_seed: Option<&'static str> = None;
    // Spawn-time worktree baseline ref (agent-only diffs). Set after create.
    let mut spawn_baseline_ref: Option<String> = None;
    let worktree_path = if let Some(ref source) = resume_source {
        // Deep-audit C1: resume of non-worktree source must not fail-open when
        // isolation=worktree was requested.
        match resume_isolation_gate(
            isolation_requested,
            source.worktree_path.is_some(),
            allow_shared_fallback,
        ) {
            ResumeIsolationGate::Proceed => {}
            ResumeIsolationGate::SharedFallback => {
                isolation_fallback = true;
                tracing::warn!(
                    subagent_id = %request.id,
                    isolation_fallback = true,
                    "Resumed source had no worktree; shared-workspace fallback (opt-in ALLOW_SHARED_FALLBACK)"
                );
            }
            ResumeIsolationGate::Refuse => {
                let msg = format!(
                    "Cannot resume subagent `{}` with isolation=worktree: source had no \
                     worktree_path (ran shared or pre-isolation). Set \
                     GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 to opt into shared-workspace \
                     resume (emits isolation_fallback; the run is NOT isolated), or \
                     spawn a fresh isolation=worktree child without resume_from.",
                    source.subagent_id
                );
                tracing::error!(
                    subagent_id = %request.id,
                    source_id = %source.subagent_id,
                    "Resume isolation=worktree of non-worktree source refused (fail-closed)"
                );
                return child_run_output(failure_result(&request, &msg), completion_data, None);
            }
        }
        match source.worktree_path.as_deref() {
            None => None,
            Some(dest) => {
                match resume_worktree_action(dest.is_dir(), source.snapshot_ref.as_deref()) {
                    ResumeWorktreeAction::Reuse => {
                        if let Err(health) = validate_subagent_worktree_materialized(dest) {
                            if isolation_requested && !allow_shared_fallback {
                                let msg = format!(
                                    "Resume worktree at {} is empty or incomplete ({health}). \
                                     Rehydrate from snapshot or spawn fresh isolation=worktree.",
                                    dest.display()
                                );
                                return child_run_output(
                                    failure_result(&request, &msg),
                                    completion_data,
                                    None,
                                );
                            }
                            isolation_fallback = isolation_requested;
                            None
                        } else {
                            // Protect reused tree from keep-N prune while this child runs.
                            if let Err(e) = write_live_worktree_marker(dest) {
                                if isolation_requested && !allow_shared_fallback {
                                    return child_run_output(
                                        failure_result(&request, &e),
                                        completion_data,
                                        None,
                                    );
                                }
                                tracing::warn!(
                                    subagent_id = %request.id,
                                    error = %e,
                                    "live worktree marker write failed; prune may reclaim tree"
                                );
                            }
                            Some(dest.to_path_buf())
                        }
                    }
                    ResumeWorktreeAction::Rehydrate => {
                        let snapshot_ref = source.snapshot_ref.clone().unwrap_or_default();
                        let source_repo =
                            resolve_subagent_source_repo(&ctx, request.cwd.as_deref());
                        match crate::session::worktree::rehydrate_subagent_worktree(
                            dest,
                            &source_repo,
                            &snapshot_ref,
                            Some(source.subagent_id.as_str()),
                        )
                        .await
                        {
                            Ok(path) => {
                                tracing::info!(
                                    subagent_id = %request.id,
                                    worktree_path = %path.display(),
                                    snapshot_ref = %snapshot_ref,
                                    "Rehydrated subagent worktree from snapshot for resume"
                                );
                                if let Err(health) = validate_subagent_worktree_materialized(&path)
                                {
                                    if isolation_requested && !allow_shared_fallback {
                                        let msg = format!(
                                            "Rehydrated worktree at {} is empty or incomplete ({health}).",
                                            path.display()
                                        );
                                        return child_run_output(
                                            failure_result(&request, &msg),
                                            completion_data,
                                            None,
                                        );
                                    }
                                    isolation_fallback = isolation_requested;
                                    None
                                } else {
                                    if let Err(e) = write_live_worktree_marker(&path) {
                                        if isolation_requested && !allow_shared_fallback {
                                            return child_run_output(
                                                failure_result(&request, &e),
                                                completion_data,
                                                None,
                                            );
                                        }
                                        tracing::warn!(
                                            subagent_id = %request.id,
                                            error = %e,
                                            "live worktree marker write failed; prune may reclaim tree"
                                        );
                                    }
                                    Some(path)
                                }
                            }
                            Err(e) => {
                                if isolation_requested && !allow_shared_fallback {
                                    let msg = format!(
                                        "Failed to rehydrate isolated worktree for subagent \
                                         (isolation required): {e}. Set \
                                         GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 to opt into \
                                         shared-workspace fallback (emits isolation_fallback; \
                                         the run is NOT isolated)."
                                    );
                                    tracing::error!(
                                        subagent_id = %request.id,
                                        error = %e,
                                        "Worktree rehydrate failed; refusing shared-workspace fallback"
                                    );
                                    return child_run_output(
                                        failure_result(&request, &msg),
                                        completion_data,
                                        None,
                                    );
                                }
                                isolation_fallback = isolation_requested;
                                tracing::warn!(
                                    subagent_id = %request.id,
                                    error = %e,
                                    isolation_fallback,
                                    "Failed to rehydrate subagent worktree; shared-workspace fallback (opt-in)"
                                );
                                None
                            }
                        }
                    }
                    ResumeWorktreeAction::Shared => {
                        if isolation_requested && !allow_shared_fallback {
                            let msg = format!(
                                "Resumed subagent worktree dir missing with no snapshot \
                                 (isolation required): {}. Set \
                                 GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 to opt into \
                                 shared-workspace fallback.",
                                dest.display()
                            );
                            tracing::error!(
                                subagent_id = %request.id,
                                worktree = %dest.display(),
                                "Worktree missing; refusing shared-workspace fallback"
                            );
                            return child_run_output(
                                failure_result(&request, &msg),
                                completion_data,
                                None,
                            );
                        }
                        isolation_fallback = isolation_requested;
                        tracing::warn!(
                            subagent_id = %request.id,
                            worktree = %dest.display(),
                            isolation_fallback,
                            "Resumed subagent worktree dir missing with no snapshot; shared-workspace fallback (opt-in)"
                        );
                        None
                    }
                }
            }
        }
    } else if isolation_requested {
        let source_cwd = parent_source_cwd(&ctx, request.cwd.as_deref());
        let dest = match crate::session::worktree::worktree_base_dir_for_source(&source_cwd) {
            Ok(base) => base.join(format!("subagent-{}", request.id)),
            Err(e) => {
                tracing::warn!(
                    subagent_id = %request.id,
                    error = %e,
                    "Could not resolve worktree base dir, using temp dir for subagent worktree"
                );
                std::env::temp_dir()
                    .join("grok-subagent-worktrees")
                    .join(&request.id)
            }
        };
        // RC12: prune soft-preserved worktrees (keep-N) + free-space guard so
        // parallel densify loops don't fill the disk (os error 112).
        let mut skip_worktree_create = false;
        if let Some(parent_base) = dest.parent() {
            prune_soft_preserved_worktrees(parent_base);
            // Second prune pass if still low: drop to keep-N/2 (or age-only if KEEP_N=0).
            if ensure_min_free_space_for_worktree(parent_base).is_err() {
                let keep = soft_preserve_keep_n();
                if keep == 0 {
                    prune_soft_preserved_worktrees_by_age(parent_base, soft_preserve_max_age() / 2);
                } else {
                    prune_soft_preserved_worktrees_with_cap(parent_base, (keep / 2).max(1));
                }
            }
            if let Err(msg) = ensure_min_free_space_for_worktree(parent_base) {
                if !allow_shared_fallback {
                    tracing::error!(
                        subagent_id = %request.id,
                        error = %msg,
                        "Pre-spawn disk guard refused worktree create"
                    );
                    return child_run_output(failure_result(&request, &msg), completion_data, None);
                }
                isolation_fallback = true;
                skip_worktree_create = true;
                tracing::warn!(
                    subagent_id = %request.id,
                    error = %msg,
                    isolation_fallback,
                    "Disk guard failed; shared-workspace fallback (opt-in)"
                );
            }
        }
        let source_clone = source_cwd;
        let subagent_id = request.id.clone();
        let creation_mode: xai_fast_worktree::CreationMode = ctx.worktree_type.into();
        // RC9: default clean-slate seed (HEAD only) so land/diff are agent-only.
        // Opt into dirty parent copy: GROK_SUBAGENT_WORKTREE_SEED=dirty|preserve.
        // Explicit clean: clean|head|head-only (same as default).
        let (seed_label, working_tree_mode) = parse_worktree_seed_mode(
            &std::env::var("GROK_SUBAGENT_WORKTREE_SEED").unwrap_or_default(),
        );
        worktree_seed = Some(seed_label);
        let btrfs_delegate = crate::session::worktree::btrfs_delegate_from_env();
        let create_result = if skip_worktree_create {
            None
        } else {
            Some(
                tokio::task::spawn_blocking(move || {
                    let mut builder = xai_fast_worktree::WorktreeBuilder::new(&source_clone, &dest)
                        .working_tree_mode(working_tree_mode)
                        .creation_mode(creation_mode)
                        .worktree_kind(xai_fast_worktree::WorktreeKind::Subagent)
                        .session_id(subagent_id);
                    if let Some(delegate) = btrfs_delegate {
                        builder = builder.btrfs_delegate(delegate);
                    }
                    builder.create()
                })
                .await,
            )
        };
        match create_result {
            None => {
                // Disk guard opted into shared fallback — no worktree path.
                None
            }
            Some(Ok(Ok(report))) => {
                tracing::info!(
                    subagent_id = %request.id,
                    worktree_path = %report.worktree_path.display(),
                    commit = %report.commit,
                    "Created isolated worktree for subagent"
                );
                // Capture spawn baseline (full tree after create, before agent edits)
                // so diff/land are agent-only even when the parent was dirty.
                let baseline_ref_name = format!("refs/grok/subagent-baselines/{}", request.id);
                let source_repo = resolve_subagent_source_repo(&ctx, request.cwd.as_deref());
                match crate::session::worktree::snapshot_subagent_worktree(
                    &report.worktree_path,
                    &source_repo,
                    &baseline_ref_name,
                )
                .await
                {
                    Ok(baseline_ref) => {
                        tracing::info!(
                            subagent_id = %request.id,
                            baseline_ref = %baseline_ref,
                            "Recorded subagent worktree spawn baseline"
                        );
                        spawn_baseline_ref = Some(baseline_ref);
                    }
                    Err(e) => {
                        // RC13 Wave D: fail closed by default — land without baseline
                        // attributes dirty-parent noise to the agent. Opt into soft
                        // continue with GROK_SUBAGENT_ALLOW_BASELINE_SOFT_FAIL=1.
                        let allow_soft = std::env::var("GROK_SUBAGENT_ALLOW_BASELINE_SOFT_FAIL")
                            .map(|v| {
                                matches!(
                                    v.trim().to_ascii_lowercase().as_str(),
                                    "1" | "true" | "yes" | "on"
                                )
                            })
                            .unwrap_or(false);
                        if !allow_soft {
                            tracing::error!(
                                subagent_id = %request.id,
                                error = %e,
                                "Failed to snapshot spawn baseline; refusing isolation spawn (RC13)"
                            );
                            let _ = xai_fast_worktree::remove_worktree(&report.worktree_path);
                            let _ = std::fs::remove_dir_all(&report.worktree_path);
                            let msg = format!(
                                "Isolated worktree baseline capture failed ({e}). \
                                 Land would not be agent-only without a baseline. \
                                 Retry spawn, free disk, or set \
                                 GROK_SUBAGENT_ALLOW_BASELINE_SOFT_FAIL=1 to continue \
                                 with land force-required later."
                            );
                            return child_run_output(
                                failure_result(&request, &msg),
                                completion_data,
                                None,
                            );
                        }
                        tracing::warn!(
                            subagent_id = %request.id,
                            error = %e,
                            "Failed to snapshot spawn baseline; continuing with soft-fail (ALLOW_BASELINE_SOFT_FAIL=1)"
                        );
                    }
                }
                // Health check: refuse empty / non-materialized trees (overlay
                // upper only, failed checkout). File tools can still see parent
                // via DisplayCwd remap while shell CWD is empty — densify P1.
                if let Err(health) = validate_subagent_worktree_materialized(&report.worktree_path)
                {
                    tracing::error!(
                        subagent_id = %request.id,
                        worktree = %report.worktree_path.display(),
                        error = %health,
                        "Worktree create reported success but tree is not usable"
                    );
                    let _ = xai_fast_worktree::remove_worktree(&report.worktree_path);
                    let _ = std::fs::remove_dir_all(&report.worktree_path);
                    if !allow_shared_fallback {
                        let msg = format!(
                            "Isolated worktree at {} is empty or incomplete after create ({health}). \
                             Refusing to start. Retry spawn or set \
                             GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 only if shared parent is OK.",
                            report.worktree_path.display()
                        );
                        return child_run_output(
                            failure_result(&request, &msg),
                            completion_data,
                            None,
                        );
                    }
                    isolation_fallback = true;
                    None
                } else {
                    if let Err(e) = write_live_worktree_marker(&report.worktree_path) {
                        if !allow_shared_fallback {
                            return child_run_output(
                                failure_result(&request, &e),
                                completion_data,
                                None,
                            );
                        }
                        tracing::warn!(
                            subagent_id = %request.id,
                            error = %e,
                            "live worktree marker write failed; prune may reclaim tree"
                        );
                    }
                    Some(report.worktree_path)
                }
            }
            Some(Ok(Err(e))) => {
                if !allow_shared_fallback {
                    let msg = format!(
                        "Failed to create isolated worktree for subagent \
                         (isolation required): {e}. Set \
                         GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 to opt into \
                         shared-workspace fallback (emits isolation_fallback; \
                         the run is NOT isolated)."
                    );
                    tracing::error!(
                        subagent_id = %request.id,
                        error = %e,
                        "Worktree creation failed; refusing shared-workspace fallback"
                    );
                    return child_run_output(failure_result(&request, &msg), completion_data, None);
                }
                isolation_fallback = true;
                tracing::warn!(
                    subagent_id = %request.id,
                    error = %e,
                    isolation_fallback,
                    "Failed to create worktree; shared-workspace fallback (opt-in)"
                );
                None
            }
            Some(Err(e)) => {
                if !allow_shared_fallback {
                    let msg = format!(
                        "Worktree creation task panicked (isolation required): {e}. Set \
                         GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 to opt into \
                         shared-workspace fallback."
                    );
                    tracing::error!(
                        subagent_id = %request.id,
                        error = %e,
                        "Worktree creation panicked; refusing shared-workspace fallback"
                    );
                    return child_run_output(failure_result(&request, &msg), completion_data, None);
                }
                isolation_fallback = true;
                tracing::warn!(
                    subagent_id = %request.id,
                    error = %e,
                    isolation_fallback,
                    "Worktree creation task panicked; shared-workspace fallback (opt-in)"
                );
                None
            }
        }
    } else {
        None
    };

    // P0 tombstone: isolation worktree path must exist before the child boots.
    // Without this, every shell/file tool fails with OS error 267 (Windows
    // "directory name is invalid") when prune/external delete raced spawn.
    if let Some(ref wt) = worktree_path {
        if !wt.is_dir() {
            if isolation_requested && !allow_shared_fallback {
                let msg = format!(
                    "Isolated worktree path does not exist (tombstoned or never created): {}. \
                     Refusing to start with a missing CWD (OS error 267). \
                     Re-spawn isolation=worktree, restore from snapshot, or set \
                     GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 only if shared parent writes are acceptable.",
                    wt.display()
                );
                tracing::error!(
                    subagent_id = %request.id,
                    worktree = %wt.display(),
                    "Worktree path missing before child start; fail-closed"
                );
                return child_run_output(failure_result(&request, &msg), completion_data, None);
            }
            isolation_fallback = isolation_requested;
            tracing::warn!(
                subagent_id = %request.id,
                worktree = %wt.display(),
                isolation_fallback,
                "Worktree path missing before child start; shared-workspace fallback (opt-in)"
            );
            // Drop invalid path so resolve_child_cwd uses parent.
            // (worktree_path is not mut here — rebind below)
        }
    }
    // If path was missing and we opted into shared fallback, clear it.
    let worktree_path = match worktree_path {
        Some(ref wt) if !wt.is_dir() => None,
        other => other,
    };

    // Defense in depth: isolation=worktree with no path must not start unless
    // the operator opted into shared fallback (already set isolation_fallback).
    if isolation_requested && worktree_path.is_none() && !allow_shared_fallback {
        let msg = "isolation=worktree requested but no worktree path is available \
             (create/rehydrate failed earlier without a user-visible error). \
             Refusing shared-workspace start. Re-spawn, free disk, or set \
             GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1 only if shared parent writes are acceptable."
            .to_string();
        tracing::error!(
            subagent_id = %request.id,
            "isolation_requested without worktree_path; fail-closed"
        );
        return child_run_output(failure_result(&request, &msg), completion_data, None);
    }
    if isolation_requested && worktree_path.is_none() && allow_shared_fallback {
        isolation_fallback = true;
    }

    // RC11: resume Reuse/Rehydrate never hit the fresh-create baseline path above.
    // Capture a FRESH baseline for THIS child id at resume start so dispose
    // export / diff / land stay agent-only (not dirty-parent bulk FOOTGUN).
    // Prefer a new snapshot of the live tree; fall back to the source's
    // baseline_ref from meta.json when snapshot fails.
    if resume_source.is_some() && spawn_baseline_ref.is_none() {
        if let Some(ref wt) = worktree_path {
            let baseline_ref_name = format!("refs/grok/subagent-baselines/{}", request.id);
            let source_repo = resolve_subagent_source_repo(&ctx, request.cwd.as_deref());
            match crate::session::worktree::snapshot_subagent_worktree(
                wt,
                &source_repo,
                &baseline_ref_name,
            )
            .await
            {
                Ok(baseline_ref) => {
                    tracing::info!(
                        subagent_id = %request.id,
                        baseline_ref = %baseline_ref,
                        "Recorded resume-time spawn baseline for agent-only land/diff"
                    );
                    spawn_baseline_ref = Some(baseline_ref);
                }
                Err(e) => {
                    let inherited = resume_source.as_ref().and_then(|src| {
                        durable_source_baseline_ref(
                            &src.subagent_id,
                            &ctx.parent_session_id,
                            &ctx.parent_cwd,
                        )
                    });
                    if let Some(base) = inherited {
                        tracing::warn!(
                            subagent_id = %request.id,
                            error = %e,
                            baseline_ref = %base,
                            "Resume baseline snapshot failed; inheriting source baseline_ref"
                        );
                        spawn_baseline_ref = Some(base);
                    } else {
                        tracing::warn!(
                            subagent_id = %request.id,
                            error = %e,
                            "Failed to snapshot resume baseline; diff/land may include dirty-parent files"
                        );
                    }
                }
            }
        }
    }

    let worktree_freshly_created = resume_source.is_none() && worktree_path.is_some();
    // Remove a freshly created worktree if we bail before the normal
    // completion dispose path (model/bootstrap/spawn failures).
    let mut fresh_worktree_guard = FreshWorktreeGuard::new(
        worktree_freshly_created
            .then(|| worktree_path.clone())
            .flatten(),
    );
    if let Some(raw_cwd) = request.cwd.as_deref() {
        match sanitize_cwd_value(raw_cwd) {
            Some(cwd_path) => {
                if worktree_path.is_none() && resume_source.is_none() {
                    let p = Path::new(&cwd_path);
                    if !p.is_dir() {
                        let msg = if p.exists() {
                            format!("cwd \"{cwd_path}\" exists but is not a directory")
                        } else {
                            format!("cwd \"{cwd_path}\" does not exist")
                        };
                        return child_run_output(
                            failure_result(&request, &msg),
                            completion_data,
                            None,
                        );
                    }
                }
                request.cwd = Some(cwd_path);
            }
            None => request.cwd = None,
        }
    }
    if effective_runtime.reasoning_effort.is_some() || effective_runtime.capability_mode.is_some() {
        tracing::info!(
            subagent_id = %request.id,
            reasoning_effort = ?effective_runtime.reasoning_effort,
            capability_mode = ?effective_runtime.capability_mode,
            "Resolved runtime overrides for subagent"
        );
    }
    effective_runtime.capability_mode = xai_grok_subagent_resolution::intersect_capability_modes(
        effective_runtime.capability_mode,
        definition.capability_mode,
    );
    let child_depth = request
        .runtime_overrides
        .spawn_depth
        .unwrap_or(ctx.parent_depth + 1);
    let tools_before_policy = definition.tool_config.tools.len();
    let allow_nested_subagents = child_depth < ctx.subagents_max_depth;
    xai_grok_subagent_resolution::apply_child_tool_policy(
        &mut definition,
        effective_runtime.capability_mode,
        allow_nested_subagents,
    );
    // Stamp the ceiling onto the definition so AgentBuilder's post-inject
    // capability clamp re-runs (blocks write re-injection under read-only).
    // Without this, inject_default_tools + write_file_enabled re-adds `write`.
    definition.capability_mode = effective_runtime.capability_mode;
    if let Some(mode) = effective_runtime.capability_mode {
        tracing::info!(
            subagent_id = %request.id,
            capability_mode = ?mode,
            tools_remaining = definition.tool_config.tools.len(),
            "Applied capability mode filter to agent tool config"
        );
    }
    if !allow_nested_subagents && definition.tool_config.tools.len() < tools_before_policy {
        tracing::info!(
            subagent_id = %request.id,
            child_depth,
            "Stripped task tool from child at max depth"
        );
    }
    if request.owner.is_workflow() {
        definition.tool_config.tools.retain(|tool| {
            !matches!(
                tool.id.rsplit(':').next(),
                Some("scheduler_create" | "scheduler_list" | "scheduler_delete")
            )
        });
    }
    // Fork reuses parent conversation prefix and prefers the parent model for
    // cache locality — but an *explicit* Task/spawn model override (e.g. goal
    // planner role model) must win so multi-model orchestration works.
    if request.fork_context && request.runtime_overrides.model.is_none() {
        effective_runtime.model = Some(ctx.model_id.0.to_string());
    }
    let (mut effective_sampling_config, mut effective_model_id) = resolve_effective_model_config(
        effective_runtime.model.as_deref(),
        &request.subagent_type,
        &definition.model,
        &ctx,
    )
    .await;
    let subagent_max_turns = resolve_subagent_max_turns(definition.max_turns, ctx.parent_max_turns);
    {
        let model_str = &effective_sampling_config.model;
        let model_unknown = !model_str.is_empty()
            && !ctx.available_models.is_empty()
            && !ctx.available_models.contains_key(model_str)
            && !ctx
                .available_models
                .values()
                .any(|e| e.info().model == *model_str);
        if model_unknown {
            let (parent_config, parent_mid) = read_parent_sampling_config(&ctx).await;
            tracing::warn!(
                subagent_id = %request.id,
                resolved_model = %model_str,
                parent_model = %parent_config.model,
                "Resolved subagent model not found in available models — \
                 falling back to parent model"
            );
            effective_sampling_config = parent_config;
            effective_model_id = parent_mid;
        }
    }
    if let Some(ref source) = resume_source
        && let Some(ref source_model) = source.model_id
        && !source_model.is_empty()
        && effective_model_id.0.as_ref() != source_model.as_str()
    {
        if let Some(resolved) = resolve_model_override_to_config(source_model, &ctx) {
            tracing::info!(
                subagent_id = %request.id,
                resolved_model = %effective_model_id.0,
                source_model = source_model,
                "Pinning resumed child to source model"
            );
            effective_sampling_config = resolved.0;
            effective_model_id = resolved.1;
        } else {
            let msg = format!(
                "Cannot resume from subagent '{}': source model '{source_model}' \
                 is no longer available in the model catalogue.",
                source.subagent_id,
            );
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
    }
    // Wall-clock / tool / stall budget AFTER model resolve so NVIDIA platform
    // defaults (1h timeout, 30 min stall) apply when spawn omits timeout_ms.
    // Order: explicit timeout_ms > agent-def timeout_secs > NVIDIA 3600s > none.
    let execution_budget = SubagentExecutionBudget::resolve_with_platform_and_scope(
        &definition,
        ctx.parent_max_turns,
        request.runtime_overrides.timeout_ms,
        request.runtime_overrides.stall_timeout_ms,
        Some(effective_model_id.0.as_ref()),
        request
            .allowed_paths
            .as_ref()
            .is_some_and(|p| !p.is_empty()),
    );
    append_execution_budget_prompt(&mut definition, execution_budget);
    if let Some(effort) = effective_runtime.reasoning_effort
        && ctx
            .models_manager
            .model_supports_reasoning_effort(effective_model_id.0.as_ref())
    {
        // Fork's typed effort enum; strings align 1:1 with the sampler's
        // canonical tokens (none..ultra), so parse is total.
        match effort.as_str().parse::<ReasoningEffort>() {
            Ok(eff) => effective_sampling_config.reasoning_effort = Some(eff),
            Err(err) => {
                tracing::warn!(
                    value = %effort,
                    error = %err,
                    "subagent reasoning_effort: parse failed, ignoring override"
                )
            }
        }
    }
    let subagent_id = request.id.clone();
    let child_session_id = acp::SessionId::new(subagent_id.clone());
    let override_cwd = select_override_cwd(resume_source.as_ref(), request.cwd.as_deref());
    let effective_cwd_path =
        resolve_child_cwd(worktree_path.as_deref(), override_cwd, &ctx.parent_cwd);
    // Honesty: when isolation claims worktree (no fallback), child CWD must look
    // like a product worktree — never the bare parent repo.
    if isolation_requested && !isolation_fallback {
        if !path_looks_like_subagent_worktree(&effective_cwd_path) {
            let msg = format!(
                "isolation=worktree claimed but resolved child CWD is not a subagent worktree \
                 (`{}`). Refusing start to avoid shared parent writes. \
                 Re-spawn isolation=worktree, or use isolation=none explicitly when shared \
                 parent writes are intended (e.g. Blender MCP absolute export paths).",
                effective_cwd_path.display()
            );
            tracing::error!(
                subagent_id = %request.id,
                child_cwd = %effective_cwd_path.display(),
                worktree = ?worktree_path.as_ref().map(|p| p.display().to_string()),
                "Child CWD failed subagent-worktree pattern; fail-closed"
            );
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
    }
    let effective_cwd = effective_cwd_path.to_string_lossy().into_owned();
    let child_session_info = SessionInfo {
        id: child_session_id.clone(),
        cwd: effective_cwd,
    };
    let child_session_dir = session::persistence::session_dir(&child_session_info);
    let parent_session_dir = session::persistence::session_dir(&SessionInfo {
        id: acp::SessionId::new(ctx.parent_session_id.clone()),
        cwd: ctx.parent_cwd.to_string_lossy().to_string(),
    });
    let subagent_meta_dir = parent_session_dir.join("subagents").join(&subagent_id);
    let InitialContext {
        source: context_source,
        copy_error: fork_copy_error,
        prefix_len: inherited_prefix_len,
        conversation: forked_conversation,
        verbatim_fork: context_verbatim_fork,
    } = match bootstrap_initial_context(
        &request,
        resume_source.as_ref(),
        &ctx,
        &child_session_info,
        &child_session_dir,
        effective_model_id.0.as_ref(),
        effective_sampling_config.context_window,
    )
    .await
    {
        BootstrapInitialContext::Ready(ctx) => ctx,
        BootstrapInitialContext::ResumeAbort(msg) => {
            tracing::error!(
                subagent_id = %request.id,
                error = %msg,
                "Resume-copy failed, aborting subagent spawn"
            );
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
    };
    let verbatim_mirror_fork =
        context_source == InitialContextSource::Forked && context_verbatim_fork;
    let task_prompt_text = prompt.clone();
    let (mut forked_conversation, mut inherited_prefix_len) =
        (forked_conversation, inherited_prefix_len.unwrap_or(0));
    if context_source != InitialContextSource::Resumed
        && !verbatim_mirror_fork
        && let Some(ref pi) = effective_runtime.persona_instructions
    {
        let reminder = xai_grok_sampling_types::conversation::ConversationItem::system_reminder(
            format!("<system-reminder>\n{pi}\n</system-reminder>"),
        );
        let insert_at = inherited_prefix_len.min(forked_conversation.len());
        forked_conversation.insert(insert_at, reminder);
        inherited_prefix_len += 1;
    }
    let effective_source_str = match &context_source {
        InitialContextSource::New => "new",
        InitialContextSource::Forked => "forked",
        InitialContextSource::Resumed => "resumed",
    };
    let subagent_meta = SubagentMeta {
        subagent_id: subagent_id.clone(),
        parent_session_id: ctx.parent_session_id.clone(),
        child_session_id: child_session_id.0.to_string(),
        subagent_type: request.subagent_type.clone(),
        description: request.description.clone(),
        prompt: request.prompt.clone(),
        status: "running".to_string(),
        started_at: chrono::Utc::now(),
        completed_at: None,
        duration_ms: None,
        tool_calls: None,
        turns: None,
        error: None,
        effective_context_source: Some(effective_source_str.to_string()),
        context_normalized: fork_context_normalized(&context_source, context_verbatim_fork),
        fork_copy_error: fork_copy_error.clone(),
        persona: effective_runtime.persona.clone(),
        resumed_from: request.resume_from.clone(),
        child_cwd: Some(child_session_info.cwd.clone()),
        display_cwd: worktree_path.as_ref().map(|_| {
            parent_source_cwd(&ctx, request.cwd.as_deref())
                .to_string_lossy()
                .into_owned()
        }),
        worktree_path: worktree_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        snapshot_ref: None,
        baseline_ref: spawn_baseline_ref.clone(),
        worktree_state: if worktree_path.is_some() {
            Some("live".to_string())
        } else {
            None
        },
        isolation_fallback: Some(isolation_fallback),
        isolation_requested: Some(if isolation_requested {
            "worktree".to_string()
        } else {
            "none".to_string()
        }),
        patch_path: None,
        diffstat: None,
        changed_paths: None,
        land_status: None,
        // Inherit source allowlist on resume when the caller omits one so land
        // continues to enforce the original spawn boundary (RC11 harness).
        // Fail closed: non-empty request that normalizes to nothing is rejected
        // later (before SetAllowedWritePaths); store only valid prefixes.
        allowed_paths: {
            let from_request = request.allowed_paths.as_ref().filter(|p| !p.is_empty());
            if let Some(raw) = from_request {
                let norm: Vec<String> = raw
                    .iter()
                    .filter_map(|s| {
                        xai_grok_tools::implementations::grok_build::subagent_worktree::normalize_allowlist_path(
                            s,
                        )
                    })
                    .collect();
                if norm.is_empty() {
                    // Keep raw so meta reflects the caller's intent; spawn aborts below.
                    Some(raw.clone())
                } else {
                    Some(norm)
                }
            } else {
                resume_source.as_ref().and_then(|src| {
                    durable_source_allowed_paths(
                        &src.subagent_id,
                        &ctx.parent_session_id,
                        &ctx.parent_cwd,
                    )
                })
            }
        },
        worktree_seed: worktree_seed.map(|s| s.to_string()).or_else(|| {
            // Resume: inherit source seed from durable meta when present.
            resume_source.as_ref().and_then(|src| {
                durable_subagent_meta(&src.subagent_id, &ctx.parent_session_id, &ctx.parent_cwd)
                    .and_then(|m| m.worktree_seed)
            })
        }),
        effective_model_id: Some(effective_model_id.0.to_string()),
    };
    // C2: non-empty allowed_paths that all fail normalization must not start unrestricted.
    if let Some(raw) = request.allowed_paths.as_ref().filter(|p| !p.is_empty()) {
        let any_valid = raw.iter().any(|s| {
            xai_grok_tools::implementations::grok_build::subagent_worktree::normalize_allowlist_path(
                s,
            )
            .is_some()
        });
        if !any_valid {
            let msg = format!(
                "allowed_paths has no valid relative prefixes (got {raw:?}). \
                 Use repo-relative paths like `crates/foo/` — absolute paths and `..` are rejected."
            );
            return child_run_output(failure_result(&request, &msg), completion_data, None);
        }
    }
    write_subagent_meta(&subagent_meta_dir, &subagent_meta);
    if let (Some(bucket_url), Some(upload_method)) = (&ctx.gcs_bucket_url, &ctx.gcs_upload_method) {
        let gcs_meta = SubagentSessionMetadata::from_meta(
            &subagent_meta,
            Some(&*effective_model_id.0),
            Some(&child_session_info.cwd),
            None,
            None,
            None,
            effective_runtime
                .reasoning_effort
                .as_ref()
                .map(|e| e.as_str()),
            effective_runtime.role_name.as_deref(),
            request.parent_prompt_id.as_deref(),
            0,
        );
        let bucket = bucket_url.clone();
        let method = upload_method.clone();
        let auth_for_spawn = ctx.auth_manager.clone();
        tokio::spawn(async move {
            upload_subagent_metadata(&gcs_meta, &bucket, method, auth_for_spawn).await;
        });
    }
    let gcs_upload_ctx = GcsUploadContext {
        bucket_url: ctx.gcs_bucket_url.clone(),
        upload_method: ctx.gcs_upload_method.clone(),
        model_id: Some(effective_model_id.0.to_string()),
        cwd: Some(child_session_info.cwd.clone()),
        reasoning_effort: effective_runtime
            .reasoning_effort
            .map(|e| e.as_str().to_string()),
        role_name: effective_runtime.role_name.clone(),
        parent_prompt_id: request.parent_prompt_id.clone(),
        auth_manager: ctx.auth_manager.clone(),
        isolation_mode: Some(format!("{:?}", effective_runtime.isolation)),
        capability_mode: effective_runtime
            .capability_mode
            .as_ref()
            .map(|m| format!("{m:?}")),
        depth: child_depth,
    };
    emit_subagent_notification(
        gateway,
        &ctx.parent_session_id,
        SessionUpdate::SubagentSpawned {
            subagent_id: subagent_id.clone(),
            child_session_id: child_session_id.0.to_string(),
            parent_session_id: ctx.parent_session_id.clone(),
            parent_prompt_id: request.parent_prompt_id.clone(),
            subagent_type: request.subagent_type.clone(),
            description: request.description.clone(),
            effective_context_source: Some(effective_source_str.to_string()),
            context_normalized: fork_context_normalized(&context_source, context_verbatim_fork),
            capability_mode: effective_runtime
                .capability_mode
                .and_then(|m| serde_json::to_value(m).ok())
                .and_then(|v| v.as_str().map(String::from)),
            persona: effective_runtime.persona.clone(),
            role: effective_runtime.role_name.clone(),
            model: Some(effective_model_id.0.to_string()),
            resumed_from: request.resume_from.clone(),
            budget: execution_budget.wire(),
            workflow_run_id: request.owner.workflow_run_id().map(str::to_string),
            child_cwd: Some(child_session_info.cwd.clone()),
            display_cwd: worktree_path.as_ref().map(|_| {
                parent_source_cwd(&ctx, request.cwd.as_deref())
                    .to_string_lossy()
                    .into_owned()
            }),
            worktree_path: worktree_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            isolation_requested: Some(if isolation_requested {
                "worktree".to_string()
            } else {
                "none".to_string()
            }),
            isolation_fallback,
        },
        ctx.parent_cmd_tx.as_ref(),
    );
    completion_data.spawned_notification_emitted = true;
    let early_gcs_ctx = GcsUploadContext {
        bucket_url: ctx.gcs_bucket_url.clone(),
        upload_method: ctx.gcs_upload_method.clone(),
        model_id: None,
        cwd: None,
        isolation_mode: None,
        capability_mode: None,
        reasoning_effort: effective_runtime
            .reasoning_effort
            .map(|e| e.as_str().to_string()),
        role_name: effective_runtime.role_name.clone(),
        parent_prompt_id: request.parent_prompt_id.clone(),
        depth: 0,
        auth_manager: ctx.auth_manager.clone(),
    };
    let sampling_client = match crate::sampling::Client::new(effective_sampling_config.clone()) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Sampling client error: {e}");
            let result = fail_subagent(
                &msg,
                &subagent_id,
                &child_session_id,
                &subagent_meta_dir,
                0,
                &early_gcs_ctx,
            );
            return child_run_output(result, completion_data, None);
        }
    };
    let persistence = match session::persistence::new_with_explicit_dir(
        &child_session_info,
        child_session_dir.clone(),
        effective_model_id.clone(),
        sampling_client,
        effective_sampling_config.model.clone(),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("Persistence error: {e}");
            let result = fail_subagent(
                &msg,
                &subagent_id,
                &child_session_id,
                &subagent_meta_dir,
                0,
                &early_gcs_ctx,
            );
            return child_run_output(result, completion_data, None);
        }
    };
    let child_cwd = resolve_child_cwd(worktree_path.as_deref(), override_cwd, &ctx.parent_cwd);
    let covered_by_parent = xai_fsnotify::watch_root_covers(&ctx.parent_cwd, &child_cwd);
    let subagent_fs_watch = FsWatchCapabilities {
        hunk_tracking: ctx.hunk_tracking_enabled && !covered_by_parent,
        ..FsWatchCapabilities::none()
    };
    let child_cwd_abs = xai_grok_paths::AbsPathBuf::new(child_cwd).unwrap_or_else(|_| {
        xai_grok_paths::AbsPathBuf::new(std::env::current_dir().unwrap_or_default())
            .expect("current_dir should be absolute")
    });
    // Densify / asset-kit: ensure Blender/Godot paths are visible to confined
    // children even when the binary lives outside the worktree (Program Files).
    let mut child_session_env = (*ctx.session_env).clone();
    inject_densify_engine_env(&mut child_session_env);
    if let Some(ref wt) = worktree_path {
        inject_worktree_cargo_env(&mut child_session_env, wt);
    }
    let mut tool_ctx = ToolContext::with_preloaded_env(
        child_cwd_abs,
        Some(gateway.clone()),
        Some(child_session_id.clone()),
        ctx.fs.clone(),
        ctx.terminal.clone(),
        ctx.hunk_tracker_handle.clone(),
        child_session_env,
    )
    .with_hunk_tracking_enabled(ctx.hunk_tracking_enabled);
    tool_ctx.subagent_event_tx = Some(ctx.subagent_event_tx.clone());
    let task_output_budget = request
        .runtime_overrides
        .output_token_budget
        .map(crate::tools::tool_context::TaskOutputTokenBudget::limited);
    tool_ctx.task_output_token_budget = task_output_budget.clone();
    tool_ctx.sampler_retry_only_before_output = task_output_budget.is_some();
    tool_ctx.monitor_event_buffer = Some(MonitorEventBuffer::default());
    tool_ctx.subagent_depth = child_depth;
    tool_ctx.lsp = ctx.lsp.clone();
    tool_ctx.process_scope = ctx.process_scope.clone();
    let parent_traceparent = xai_file_utils::trace_context::current_traceparent();
    let tracker_child_cwd = child_session_info.cwd.clone();
    let tracker_model_id = effective_model_id.0.to_string();
    let initial_child_tokens = xai_chat_state::estimate_conversation_tokens(&forked_conversation);
    let model_entry = crate::agent::config::find_model_by_id(
        &ctx.available_models,
        effective_model_id.0.as_ref(),
    );
    let model_has_own_creds = model_entry.is_some_and(|entry| entry.has_own_credentials());
    let inherited_auth_type = subagent_auth_type(model_entry, &ctx.auth_method_id);
    let credentials = xai_chat_state::Credentials {
        api_key: effective_sampling_config.api_key.clone(),
        auth_type: inherited_auth_type,
        alpha_test_key: ctx.alpha_test_key.clone(),
        client_version: effective_sampling_config.client_version.clone(),
    };
    xai_grok_telemetry::unified_log::info(
        "subagent spawn credentials",
        None,
        Some(serde_json::json!({
            "subagent_id": &request.id,
            "subagent_type": &request.subagent_type,
            "effective_model": effective_model_id.0.as_ref(),
            "effective_model_raw": &effective_sampling_config.model,
            "base_url": &effective_sampling_config.base_url,
            "key_prefix": key_prefix(&effective_sampling_config.api_key),
            "auth_type": format!("{:?}", inherited_auth_type),
            "model_has_own_creds": model_has_own_creds,
            "auth_method_id": ctx.auth_method_id.0.as_ref(),
            "parent_model": ctx.model_id.0.as_ref(),
            "parent_key_prefix": key_prefix(&ctx.sampling_config.api_key),
            "context_window": effective_sampling_config.context_window,
        })),
    );
    let attribution_callback: Option<xai_grok_sampler::SharedAttributionCallback> =
        effective_sampling_config.attribution_callback.clone();
    let agent_memory_scope = definition.memory;
    let agent_name_for_memory = definition.name.clone();
    let is_plugin_agent = definition.plugin_name.is_some();
    let yolo_policy_block = xai_grok_workspace::permission::resolution::yolo_disabled_by_policy();
    let agent_permission_mode = resolve_subagent_permission_mode(
        definition.permission_mode.clone(),
        is_plugin_agent,
        yolo_policy_block,
    );
    if agent_permission_mode != definition.permission_mode {
        if is_plugin_agent {
            tracing::warn!(
                agent = %definition.name,
                plugin = ?definition.plugin_name,
                "ignoring permissionMode on plugin agent (not supported for security)"
            );
        } else {
            tracing::warn!(
                agent = %definition.name,
                "ignoring subagent permissionMode=bypassPermissions: always-approve disabled by managed policy"
            );
        }
    }
    if let Some(scope) = agent_memory_scope {
        let memory_tools: Vec<xai_grok_tools::registry::types::ToolConfig> = vec![
            (&grok_build::ReadFileTool).into(),
            (&grok_build::SearchReplaceTool).into(),
            (&opencode::OpenCodeWriteTool).into(),
        ];
        for tc in memory_tools {
            if !definition.tool_config.tools.iter().any(|t| t.id == tc.id) {
                definition.tool_config.tools.push(tc);
            }
        }
        // Memory inject can re-add write/edit after capability filter — re-clamp.
        if let Some(mode) = definition.capability_mode {
            mode.filter_tool_config(&mut definition.tool_config);
        }
        if xai_tool_types::is_safe_agent_name(&agent_name_for_memory) {
            let resolved_mem = scope.resolve_dir(&agent_name_for_memory, &ctx.parent_cwd);
            let memory_dir = &resolved_mem.path;
            let memory_md = memory_dir.join("MEMORY.md");
            if memory_md.is_file()
                && let Ok(content) = std::fs::read_to_string(&memory_md)
            {
                const MAX_LINES: usize = 200;
                const MAX_BYTES: usize = 25 * 1024;
                let truncated: String = content
                    .lines()
                    .take(MAX_LINES)
                    .collect::<Vec<_>>()
                    .join("\n");
                let truncated =
                    xai_grok_tools::util::truncate::truncate_str(&truncated, MAX_BYTES).to_string();
                if !truncated.is_empty() {
                    let injection = format!(
                        "\n\n<agent-memory>\nMemory directory: {}\n\n{truncated}\n</agent-memory>",
                        memory_dir.display()
                    );
                    definition.prompt_body =
                        Some(definition.prompt_body.unwrap_or_default() + injection.as_str());
                }
            }
        } else {
            tracing::warn!(
                agent = %agent_name_for_memory,
                "skipping agent memory: name is not a safe path segment"
            );
        }
    }
    let is_plugin_agent = definition.plugin_name.is_some();
    if let Some(ref hooks_config) = definition.hooks {
        if is_plugin_agent {
            tracing::warn!(
                agent = %definition.name,
                plugin = ?definition.plugin_name,
                "ignoring hooks on plugin agent (not supported for security)"
            );
        } else if !crate::agent::folder_trust::agent_inline_hooks_allowed(definition.scope, || {
            crate::agent::folder_trust::project_scope_allowed(&ctx.parent_cwd)
        }) {
            tracing::warn!(
                agent = %definition.name,
                "ignoring hooks on untrusted project agent (folder not trusted; re-run with --trust)"
            );
        } else {
            let hooks_val = hooks_config.as_value();
            let (specs, errors) = xai_grok_hooks::config::parse_hooks_from_value_with_dir(
                &hooks_val,
                &format!(
                    "{}{}",
                    xai_grok_hooks::config::AGENT_HOOK_PREFIX,
                    definition.name
                ),
                &ctx.parent_cwd,
            );
            for e in &errors {
                tracing::warn!(agent = %definition.name, error = ?e, "agent hook parse error");
            }
            if !specs.is_empty() {
                let specs: Vec<_> = specs
                    .into_iter()
                    .map(|mut s| {
                        if s.event == xai_grok_hooks::event::HookEventName::Stop {
                            s.event = xai_grok_hooks::event::HookEventName::SubagentStop;
                        }
                        s
                    })
                    .collect();
                let mut registry = ctx
                    .hook_registry
                    .as_ref()
                    .map(|r| (**r).clone())
                    .unwrap_or_default();
                registry.append_specs(specs);
                ctx.hook_registry = Some(std::sync::Arc::new(registry));
            }
        }
    }
    let agent_mcp_servers: Vec<_> = if !agent_owned_mcp_servers_allowed(is_plugin_agent) {
        if !definition.mcp_servers.is_empty() {
            tracing::warn!(
                agent = %definition.name,
                plugin = ?definition.plugin_name,
                "ignoring mcpServers on plugin agent (not supported for security)"
            );
        }
        vec![]
    } else {
        definition
                .mcp_servers
                .iter()
                .filter_map(|entry| match entry {
                    xai_grok_agent::config::McpServerRef::Named(name) => {
                        ctx.parent_mcp_configs
                            .iter()
                            .find(|s| {
                                crate::session::mcp_servers::mcp_server_name(s) == name
                            })
                            .cloned()
                            .or_else(|| {
                                tracing::warn!(agent = %definition.name, server = name, "mcpServers: named ref not found in parent");
                                None
                            })
                    }
                    xai_grok_agent::config::McpServerRef::Inline { name, config } => {
                        if let serde_json::Value::Object(obj) = config
                            && obj.contains_key("type")
                        {
                            let mut flat = obj.clone();
                            flat.insert(
                                "name".to_string(),
                                serde_json::Value::String(name.clone()),
                            );
                            if let Ok(server) = serde_json::from_value::<
                                agent_client_protocol::McpServer,
                            >(serde_json::Value::Object(flat)) {
                                return Some(server);
                            }
                            tracing::debug!(agent = %definition.name, server = name, "ACP wire format parse failed, trying map-keyed");
                        }
                        if let Some(inner_obj) = config.as_object() {
                            let mut flat = inner_obj.clone();
                            flat.insert(
                                "name".to_string(),
                                serde_json::Value::String(name.clone()),
                            );
                            if let Ok(server) = serde_json::from_value::<
                                agent_client_protocol::McpServer,
                            >(serde_json::Value::Object(flat)) {
                                return Some(server);
                            }
                        }
                        tracing::warn!(agent = %definition.name, server = name, "mcpServers: inline config could not be parsed");
                        None
                    }
                })
                .collect()
    };
    let parent_mcp_pool =
        resolve_inherited_mcp_pool(ctx.parent_mcp_pool.take(), &definition.mcp_inheritance);
    let mcp_inherited_count = parent_mcp_pool
        .as_ref()
        .map(|p| p.len() as u32)
        .unwrap_or(0);
    if mcp_inherited_count > 0 {
        tracing::info!(
            subagent_id = %request.id,
            mcp_count = mcp_inherited_count,
            "Subagent inherited MCP servers from parent pool"
        );
    }
    let inherit_skills = definition.inherit_skills;
    let definition_background = definition.background.unwrap_or(false);
    if inherit_skills && ctx.parent_skills.is_none() {
        let parent_cwd_str = ctx.parent_cwd.to_string_lossy().to_string();
        ctx.parent_skills = Some(
            xai_grok_agent::prompt::skills::list_skills_with_plugins(
                Some(&parent_cwd_str),
                &ctx.parent_skills_config,
                ctx.plugin_registry.as_deref(),
                ctx.parent_compat,
            )
            .await,
        );
    }
    let skills_inherited_count = if inherit_skills {
        ctx.parent_skills
            .as_ref()
            .map(|s| s.len() as u32)
            .unwrap_or(0)
    } else {
        0
    };
    if skills_inherited_count > 0 {
        tracing::info!(
            subagent_id = %request.id,
            skills_count = skills_inherited_count,
            "Subagent inherited skills from parent"
        );
    }
    let mcp_owned_count = agent_mcp_servers.len() as u32;
    xai_grok_telemetry::session_ctx::log_event(xai_grok_telemetry::events::SubagentLaunched {
        subagent_id: request.id.clone(),
        parent_session_id: request.parent_session_id.clone(),
        subagent_type: request.subagent_type.clone(),
        owner: telemetry_owner_kind(&request),
        workflow_run_id: request.owner.workflow_run_id().map(str::to_string),
        queued_ms: queued_for.map(|queued| u64::try_from(queued.as_millis()).unwrap_or(u64::MAX)),
        session_running: u32::try_from(session_running).unwrap_or(u32::MAX),
        persona: request.runtime_overrides.persona.clone(),
        fork_context: matches!(context_source, InitialContextSource::Forked),
        resume_from: request.resume_from.clone(),
        isolated_worktree: worktree_path.is_some(),
        mcp_inherited_count,
        mcp_owned_count,
        skills_inherited_count,
    });
    let subagent_session_default_agent_profile = Some(definition.name.clone());
    let subagent_model_id = effective_sampling_config.model.clone();
    let _ = persistence
        .tx
        .send(crate::session::persistence::PersistenceMsg::CurrentModel {
            model_id: effective_model_id.clone(),
            agent_name: Some(definition.name.clone()),
            reasoning_effort: Some(effective_sampling_config.reasoning_effort),
        });
    let spawn_result = session::spawn_session_on_thread(
        child_session_info,
        gateway.clone(),
        effective_sampling_config,
        credentials,
        crate::agent::auth_method::new_shared_auth_method_id(Some(ctx.auth_method_id.clone())),
        Some(ctx.auth_manager.clone()),
        attribution_callback,
        tool_ctx,
        agent_mcp_servers,
        vec![],
        Default::default(),
        parent_mcp_pool,
        Vec::new(),
        true,
        false,
        None,
        persistence,
        forked_conversation,
        None,
        None,
        initial_child_tokens,
        crate::session::StartupHints {
            inherited_prefix_len: Some(inherited_prefix_len),
            is_subagent: true,
            non_interactive: ctx.parent_non_interactive,
            parent_session_id: Some(ctx.parent_session_id.clone()),
            subagent_type: Some(request.subagent_type.clone()),
            preserve_inherited_system: verbatim_mirror_fork,
            ..Default::default()
        },
        xai_grok_workspace::permission::ClientType::Generic,
        ctx.resolve_auto_compact_threshold_percent(&subagent_model_id),
        xai_grok_agent::DEFAULT_SYSTEM_PROMPT_LABEL.to_string(),
        xai_chat_state::CompactionMode::Summary,
        ctx.resolve_compaction_verbatim_input(),
        ctx.resolve_compaction_tool_choice(),
        false,
        None,
        None,
        std::sync::Arc::new(parking_lot::Mutex::new(
            xai_grok_workspace::file_system::CodebaseIndexManager::new(),
        )),
        false,
        subagent_fs_watch,
        None,
        None,
        None,
        None,
        false,
        false,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        definition,
        subagent_session_default_agent_profile,
        if inherit_skills {
            ctx.parent_skills_config.clone()
        } else {
            xai_grok_agent::prompt::skills::SkillsConfig::default()
        },
        if inherit_skills {
            ctx.parent_skills.take()
        } else {
            None
        },
        ctx.parent_compat,
        false,
        None,
        None,
        None,
        Vec::new(),
        None,
        if verbatim_mirror_fork {
            None
        } else if let Some(scope) = agent_memory_scope {
            if xai_tool_types::is_safe_agent_name(&agent_name_for_memory) {
                ctx.memory_config.as_ref().map(|mc| {
                    let mut c = mc.clone();
                    let resolved = scope.resolve_dir(&agent_name_for_memory, &ctx.parent_cwd);
                    c.enabled = true;
                    c.root_dir_override = Some(resolved.path);
                    c.flat_memory_root = resolved.is_project_scoped;
                    c
                })
            } else {
                ctx.memory_config.clone()
            }
        } else {
            ctx.memory_config.clone()
        },
        false,
        Default::default(),
        ctx.managed_mcp_state.clone(),
        ctx.managed_mcp_proxy_base_url.clone(),
        effective_model_id,
        ctx.yolo_mode
            || matches!(
                agent_permission_mode,
                xai_grok_agent::config::PermissionMode::BypassPermissions
            ),
        false,
        None,
        ctx.inference_idle_timeout_secs,
        None,
        ctx.web_search_sampling_config.clone(),
        ctx.web_fetch_config.clone(),
        ctx.image_gen_config.clone(),
        ctx.video_gen_config.clone(),
        ctx.app_builder_deployer_config.clone(),
        ctx.write_file_enabled,
        ctx.goal_enabled,
        ctx.background_workflows_enabled,
        true,
        ctx.subagents_max_depth,
        ctx.workflow_max_concurrent_agents,
        ctx.ask_user_question_enabled,
        ctx.client_hooks.clone(),
        // Isolation: model sees the *source git repo* path; tools rewrite abs
        // worktree paths onto that repo (nested checkouts under a non-git
        // umbrella must not remap onto the umbrella root).
        worktree_path.as_ref().map(|_| {
            parent_source_cwd(&ctx, request.cwd.as_deref())
                .to_string_lossy()
                .into_owned()
        }),
        std::collections::HashMap::new(),
        Vec::new(),
        xai_grok_agent::prompt::context::PromptAudience::Subagent,
        effective_runtime.role_prompt.clone(),
        None,
        ctx.disable_web_search,
        ctx.backend_tools_enabled,
        ctx.respect_gitignore,
        ctx.path_not_found_hints,
        ctx.resolve_tool_params_json(),
        ctx.plugin_registry.clone(),
        None,
        ctx.models_manager.clone(),
        parent_traceparent,
        ctx.permission_handle.clone(),
        ctx.api_key_provider.clone(),
        ctx.image_description_model.clone(),
        ctx.hook_registry.clone(),
        ctx.workspace_ops.clone(),
        vec![],
        ctx.todo_gate,
        std::mem::take(&mut ctx.remote_settings),
        std::mem::take(&mut ctx.laziness_debug_log),
        ctx.parent_terminal_backend.clone(),
        if request.owner.is_workflow() {
            None
        } else {
            ctx.parent_scheduler_handle.clone()
        },
        subagent_max_turns,
        if verbatim_mirror_fork && !request.owner.is_workflow() {
            std::mem::take(&mut ctx.parent_tool_definitions)
        } else {
            None
        },
        false,
    )
    .await;
    let (child_handle, mut permission_rx, _system_prompt, child_thread) = match spawn_result {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Failed to spawn child session: {e}");
            let result = fail_subagent(
                &msg,
                &subagent_id,
                &child_session_id,
                &subagent_meta_dir,
                start.elapsed().as_millis() as u64,
                &gcs_upload_ctx,
            );
            return child_run_output(result, completion_data, None);
        }
    };
    let promoted = reporter
        .started(StartedChild {
            child_session_id: child_session_id.0.to_string(),
            persona: effective_runtime.persona.clone(),
            resumed_from: request.resume_from.clone(),
            child_cwd: tracker_child_cwd,
            worktree_path: worktree_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            effective_model_id: tracker_model_id.clone(),
            definition_background,
            control: ShellChildRuntime {
                child_handle: child_handle.clone(),
                _child_thread: child_thread,
            },
        })
        .await;
    if !promoted {
        ctx.workspace_ops
            .end_local_session(child_session_id.0.as_ref());
        let result = cancel_pending_shell_child(
            &child_handle.cmd_tx,
            &subagent_id,
            &child_session_id,
            &subagent_meta_dir,
            worktree_path.as_deref(),
            worktree_freshly_created,
            start.elapsed().as_millis() as u64,
            &gcs_upload_ctx,
        )
        .await;
        return child_run_output(result, completion_data, None);
    }
    spawn_progress_publisher(
        child_handle.signals_handle.clone(),
        gateway.clone(),
        ctx.parent_session_id.clone(),
        request.id.clone(),
        child_session_id.0.to_string(),
        start,
        cancel_token.clone(),
        goal_tick_cmd_tx(ctx.goal_enabled, ctx.parent_cmd_tx.as_ref()),
    );
    // Wall-clock / first-progress start AFTER worktree create + child spawn
    // so setup time is not charged against the child (inc_01a0025459).
    let budget_monitor = spawn_subagent_budget_monitor(
        execution_budget,
        &child_handle,
        std::time::Instant::now(),
        cancel_token.clone(),
    );
    let (before_copy_tx, before_copy_rx) = tokio::sync::oneshot::channel();
    let _ = child_handle.cmd_tx.send(SessionCommand::CopyFile {
        respond_to: before_copy_tx,
    });
    if let Some(overrides) = ctx.inherited_tool_overrides.clone() {
        let _ = child_handle
            .cmd_tx
            .send(SessionCommand::SetToolOverrides { overrides });
    }
    // Write-time allowed_paths (same prefixes as land/diff meta). Use the
    // effective list already written to meta (request or resume-inherited).
    if let Some(paths) = subagent_meta
        .allowed_paths
        .as_ref()
        .map(|p| {
            p.iter()
                .filter_map(|s| {
                    xai_grok_tools::implementations::grok_build::subagent_worktree::normalize_allowlist_path(
                        s,
                    )
                })
                .collect::<Vec<_>>()
        })
        .filter(|p| !p.is_empty())
    {
        let _ = child_handle
            .cmd_tx
            .send(SessionCommand::SetAllowedWritePaths { paths });
    }
    let (prompt_tx, prompt_rx) = oneshot::channel();
    let prompt_text = task_prompt_text;
    let child_prompt_id = uuid::Uuid::now_v7().to_string();
    let turn_started_at = chrono::Utc::now().to_rfc3339();
    let _ = child_handle.cmd_tx.send(SessionCommand::Prompt {
        prompt_id: child_prompt_id.clone(),
        prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(prompt_text))],
        prompt_mode: crate::session::plan_mode::PromptMode::Agent,
        artifact_upload_ctx: ctx.gcs_bucket_url.as_ref().and_then(|_| {
            ctx.gcs_upload_method.as_ref().map(|method| {
                crate::upload::manifest::ArtifactUploadContext {
                    gcs_config: crate::session::repo_changes::TraceExportConfig {
                        bucket_url: ctx.gcs_bucket_url.clone(),
                        service_account_key: None,
                        prefix_dir: None,
                        gcs_prefix: Some(format!("{}/turn_0", child_session_id.0)),
                        absolute_paths: false,
                        archive_name_override: None,
                        upload_method: method.clone(),
                    },
                    artifact_tracker: crate::upload::manifest::new_artifact_tracker(),
                }
            })
        }),
        client_identifier: None,
        screen_mode: None,
        verbatim: true,
        traceparent: xai_file_utils::trace_context::current_traceparent(),
        json_schema: request.runtime_overrides.output_schema.clone(),
        send_now: false,
        admission: None,
        tool_overrides_update: None,
        respond_to: prompt_tx,
        persist_ack: None,
        parsed_prompt_tx: None,
    });
    let wait_outcome = await_subagent_turn_or_cancellation(prompt_rx, cancel_token.clone()).await;
    let budget_trigger = budget_monitor.and_then(|m| m.finish());
    let duration_ms = start.elapsed().as_millis() as u64;
    let mut turn_token_totals: Option<(u64, u64, u64)> = None;
    let mut cancellation_may_hide_usage = false;
    let mut result = match wait_outcome {
        SubagentWaitOutcome::Cancelled => {
            let (tool_calls, turns) = signals_snapshot_counts(&child_handle).await;
            cancellation_may_hide_usage = turns > 0 || tool_calls > 0;
            SubagentResult {
                success: false,
                cancelled: true,
                error: Some("Subagent was cancelled".to_string()),
                subagent_id: request.id.clone(),
                child_session_id: child_session_id.0.to_string(),
                tool_calls,
                turns,
                duration_ms,
                worktree_path: worktree_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string()),
                isolation_fallback,
                worktree_seed: worktree_seed.map(|s| s.to_string()),
                ..Default::default()
            }
        }
        SubagentWaitOutcome::TurnResult(turn_result) => {
            let was_cancelled = cancel_token.is_cancelled();
            let (tool_calls, turns) = match &*turn_result {
                Ok(Ok(crate::session::commands::PromptTurnOk {
                    turn_snapshot: Some(snapshot),
                    ..
                })) => {
                    turn_token_totals = Some((
                        snapshot.turn_input_tokens,
                        snapshot.turn_cached_input_tokens,
                        snapshot.turn_output_tokens,
                    ));
                    (
                        snapshot.current.tool_call_count,
                        snapshot.current.turn_count,
                    )
                }
                _ => signals_snapshot_counts(&child_handle).await,
            };
            let final_text = child_handle
                .chat_state_handle
                .get_last_assistant_text()
                .await
                .unwrap_or_default();
            let result_tokens = child_handle.chat_state_handle.get_total_tokens().await;
            match *turn_result {
                Ok(Ok(crate::session::commands::PromptTurnOk {
                    completion_kind: PromptCompletionKind::Cancelled { category, context },
                    ..
                })) => {
                    cancellation_may_hide_usage = true;
                    let reason = cancellation_error_message(category, context.as_ref());
                    SubagentResult {
                        success: false,
                        cancelled: true,
                        error: Some(reason),
                        termination_reason: Some("cancelled".to_string()),
                        output: if final_text.is_empty() {
                            std::sync::Arc::from(format!(
                                "Subagent '{}' ({}) was cancelled. {} tool calls, {} turns.",
                                request.description, request.subagent_type, tool_calls, turns
                            ))
                        } else {
                            std::sync::Arc::from(final_text)
                        },
                        subagent_id: request.id.clone(),
                        child_session_id: child_session_id.0.to_string(),
                        tool_calls,
                        turns,
                        duration_ms,
                        tokens_used: result_tokens,
                        output_tokens_used: 0,
                        output_usage_incomplete: true,
                        total_tokens_used: 0,
                        worktree_path: worktree_path
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string()),
                        snapshot_ref: None,
                        worktree_state: None,
                        patch_path: None,
                        diffstat: None,
                        changed_paths: None,
                        baseline_ref: None,
                        isolation_fallback,
                        worktree_seed: worktree_seed.map(|s| s.to_string()),
                        backgrounded: false,
                        error_class: None,
                    }
                }
                Ok(Ok(crate::session::commands::PromptTurnOk {
                    completion_kind: PromptCompletionKind::MaxTurnsReached { limit },
                    ..
                })) => SubagentResult {
                    success: false,
                    cancelled: true,
                    error: Some(format!("max turns reached (limit: {limit})")),
                    termination_reason: Some("max_turns".to_string()),
                    output: if final_text.is_empty() {
                        std::sync::Arc::from(format!(
                            "Subagent '{}' ({}) hit max-turns limit ({limit}). {} tool calls, {} turns.",
                            request.description, request.subagent_type, tool_calls, turns
                        ))
                    } else {
                        std::sync::Arc::from(final_text)
                    },
                    subagent_id: request.id.clone(),
                    child_session_id: child_session_id.0.to_string(),
                    tool_calls,
                    turns,
                    duration_ms,
                    tokens_used: result_tokens,
                    output_tokens_used: 0,
                    output_usage_incomplete: true,
                    total_tokens_used: 0,
                    worktree_path: worktree_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string()),
                    snapshot_ref: None,
                    worktree_state: None,
                    patch_path: None,
                    diffstat: None,
                    changed_paths: None,
                    baseline_ref: None,
                    isolation_fallback,
                    worktree_seed: worktree_seed.map(|s| s.to_string()),
                    backgrounded: false,
                    error_class: None,
                },
                Ok(Ok(crate::session::commands::PromptTurnOk {
                    structured_output, ..
                })) => {
                    let wanted_schema = request.runtime_overrides.output_schema.is_some();
                    let (success, error, output) = match (wanted_schema, structured_output) {
                        (true, Some(Ok(value))) => {
                            (true, None, std::sync::Arc::from(value.to_string()))
                        }
                        (true, Some(Err(e))) => (
                            false,
                            Some(format!("structured output validation failed: {e}")),
                            std::sync::Arc::from(final_text),
                        ),
                        (true, None) => (
                            false,
                            Some("structured output requested but none produced".to_string()),
                            std::sync::Arc::from(final_text),
                        ),
                        (false, _) => (
                            true,
                            None,
                            if final_text.is_empty() {
                                std::sync::Arc::from(format!(
                                    "Subagent '{}' ({}) completed successfully. {} tool calls, {} turns.",
                                    request.description, request.subagent_type, tool_calls, turns
                                ))
                            } else {
                                std::sync::Arc::from(final_text)
                            },
                        ),
                    };
                    SubagentResult {
                        success,
                        error,
                        output,
                        subagent_id: request.id.clone(),
                        child_session_id: child_session_id.0.to_string(),
                        tool_calls,
                        turns,
                        duration_ms,
                        tokens_used: result_tokens,
                        output_tokens_used: 0,
                        output_usage_incomplete: true,
                        total_tokens_used: 0,
                        worktree_path: worktree_path
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string()),
                        isolation_fallback,
                        worktree_seed: worktree_seed.map(|s| s.to_string()),
                        ..Default::default()
                    }
                }
                Ok(Err(e)) => {
                    cancellation_may_hide_usage = was_cancelled;
                    SubagentResult {
                        success: false,
                        cancelled: was_cancelled,
                        error: Some(if was_cancelled {
                            "Subagent was cancelled".to_string()
                        } else {
                            format!("Session error: {e}")
                        }),
                        subagent_id: request.id.clone(),
                        child_session_id: child_session_id.0.to_string(),
                        tool_calls,
                        turns,
                        duration_ms,
                        worktree_path: worktree_path
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string()),
                        isolation_fallback,
                        worktree_seed: worktree_seed.map(|s| s.to_string()),
                        ..Default::default()
                    }
                }
                Err(_) => {
                    cancellation_may_hide_usage = was_cancelled;
                    SubagentResult {
                        success: false,
                        cancelled: was_cancelled,
                        error: Some(if was_cancelled {
                            "Subagent was cancelled".to_string()
                        } else {
                            "Child session dropped unexpectedly".to_string()
                        }),
                        subagent_id: request.id.clone(),
                        child_session_id: child_session_id.0.to_string(),
                        tool_calls,
                        turns,
                        duration_ms,
                        worktree_path: worktree_path
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string()),
                        isolation_fallback,
                        worktree_seed: worktree_seed.map(|s| s.to_string()),
                        ..Default::default()
                    }
                }
            }
        }
    };
    // If the budget monitor hard-killed the child, classify the cancellation.
    // Keep usable partial text when no structured output was required.
    if let Some(trigger) = budget_trigger {
        if matches!(
            trigger,
            SubagentBudgetTrigger::Timeout
                | SubagentBudgetTrigger::MaxToolCalls
                | SubagentBudgetTrigger::Stall
        ) {
            let partial_ok = can_use_partial_budget_result(
                true,
                result.output.as_ref(),
                request.runtime_overrides.output_schema.is_some(),
            );
            result.termination_reason = Some(trigger.termination_reason().to_string());
            if partial_ok {
                result.success = true;
                result.cancelled = false;
                result.error = None;
            } else {
                result.cancelled = true;
                result.success = false;
                result.error = Some(budget_exhausted_message(trigger, execution_budget));
            }
        }
    }
    if let Some(trace_gcs_config) = gcs_upload_ctx.upload_method.as_ref().map(|method| {
        crate::session::repo_changes::TraceExportConfig {
            bucket_url: gcs_upload_ctx.bucket_url.clone(),
            service_account_key: None,
            prefix_dir: None,
            gcs_prefix: Some(format!("{}/turn_0", child_session_id.0)),
            absolute_paths: false,
            archive_name_override: None,
            upload_method: method.clone(),
        }
    }) {
        let (copy_tx, session_copy_rx) = tokio::sync::oneshot::channel();
        let _ = child_handle.cmd_tx.send(SessionCommand::CopyFile {
            respond_to: copy_tx,
        });
        let turn_messages: Option<xai_chat_state::TurnCapture> = {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if child_handle
                .cmd_tx
                .send(SessionCommand::TakeTurnMessages { respond_to: tx })
                .is_ok()
            {
                rx.await.ok().flatten()
            } else {
                None
            }
        };
        let streaming_partial = crate::upload::turn::take_streaming_partial(
            &child_handle.cmd_tx,
            child_prompt_id.clone(),
            result.success,
            gcs_upload_ctx.model_id.clone(),
        )
        .await
        .map(|mut cap| {
            cap.reason = Some(if result.cancelled {
                "subagent_cancel".to_string()
            } else {
                "subagent_non_completed".to_string()
            });
            cap
        });
        let mut permission_events = Vec::new();
        while let Ok(event) = permission_rx.try_recv() {
            permission_events.push(event);
        }
        let trace_ctx = PromptTraceContext {
            gcs_config: trace_gcs_config,
            session_info: child_handle.info.clone(),
            turn_number: 0,
            session_handle: child_handle.clone(),
            session_registry_enabled: false,
            upload_queue: None,
            artifact_tracker: crate::upload::manifest::new_artifact_tracker(),
            auth_manager: ctx.auth_manager.clone(),
        };
        let session_dir = crate::session::persistence::session_dir(&child_handle.info);
        if let Ok(prompt_bytes) = std::fs::read(session_dir.join("system_prompt.txt")) {
            let gcs_path = format!("{}/system_prompt.txt", child_session_id.0);
            crate::upload::trace::upload_trace_artifact(
                &trace_ctx,
                &prompt_bytes,
                &gcs_path,
                "text/plain",
                "system_prompt",
            )
            .await;
        }
        if let Ok(ctx_bytes) = std::fs::read(session_dir.join("prompt_context.json")) {
            let gcs_path = format!("{}/prompt_context.json", child_session_id.0);
            crate::upload::trace::upload_trace_artifact(
                &trace_ctx,
                &ctx_bytes,
                &gcs_path,
                "application/json",
                "prompt_context",
            )
            .await;
        }
        upload_session_state(
            &trace_ctx,
            "before",
            before_copy_rx,
            crate::upload::turn::UploadWait::Confirm,
        )
        .await;
        let subagent_auth = ctx.auth_manager.current();
        let metadata = PromptMetadata {
            schema_version: GCS_SCHEMA_VERSION.to_string(),
            session_id: child_session_id.0.to_string(),
            turn_number: 0,
            request_id: child_prompt_id.clone(),
            turn_started_at: turn_started_at.clone(),
            repo_root: None,
            remote_url: None,
            user_id: subagent_auth.as_ref().map(|a| a.user_id.clone()),
            user_email: subagent_auth.as_ref().and_then(|a| a.email.clone()),
            team_id: subagent_auth.as_ref().and_then(|a| a.team_id.clone()),
            client_source: Some("subagent".to_string()),
            client_version: ctx.sampling_config.client_version.clone(),
            model: gcs_upload_ctx.model_id.clone().unwrap_or_default(),
            reasoning_effort: child_handle
                .reasoning_effort
                .map(|e| e.as_str().to_string()),
            experiment_id: None,
            host_os: std::env::consts::OS.to_string(),
            host_arch: std::env::consts::ARCH.to_string(),
            prompt_has_image: Some(false),
            prompt_was_truncated: Some(false),
            prompt_verbatim: Some(true),
            cwd: Some(child_handle.info.cwd.clone()),
            agent_type: Some(request.subagent_type.clone()),
            shell_version: Some(xai_grok_version::VERSION.to_string()),
            workspace_type: None,
            sandbox: local_sandbox_telemetry(),
        };
        upload_metadata(&trace_ctx, metadata).await;
        let resolved_model = child_handle
            .get_model_metadata()
            .await
            .resolved_model_id
            .or_else(|| gcs_upload_ctx.model_id.clone());
        let turn_result_meta = TurnResultMetadata {
            schema_version: GCS_SCHEMA_VERSION,
            request_id: child_prompt_id,
            completed: result.success,
            stop_reason: None,
            total_tokens: None,
            input_tokens: turn_token_totals.map(|t| t.0),
            cached_input_tokens: turn_token_totals.map(|t| t.1),
            output_tokens: turn_token_totals.map(|t| t.2),
            error: result.error.clone(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            signals: None,
            turn_delta: None,
            start_prompt_mode: Some(crate::session::plan_mode::PromptMode::Agent.to_string()),
            end_prompt_mode: Some(crate::session::plan_mode::PromptMode::Agent.to_string()),
            resolved_model,
            subagents_spawned: vec![],
        };
        upload_turn_result(
            &trace_ctx,
            &turn_result_meta,
            crate::upload::turn::UploadWait::Confirm,
        )
        .await;
        match complete_prompt_trace(
            trace_ctx,
            permission_events,
            session_copy_rx,
            turn_messages,
            streaming_partial,
            crate::upload::turn::UploadWait::Confirm,
        )
        .await
        {
            Ok(_) => {
                tracing::debug!(
                    subagent_id = %request.id,
                    child_session_id = %child_session_id.0,
                    "Subagent trace artifacts uploaded"
                );
            }
            Err(e) => {
                tracing::warn!(
                    subagent_id = %request.id,
                    error = %e,
                    "Subagent trace upload failed (non-fatal)"
                );
            }
        }
    }
    completion_data.set_persisted_output_dir(persist_subagent_output(&subagent_meta_dir, &result));
    persist_subagent_completion(&subagent_meta_dir, &result, &gcs_upload_ctx);
    let final_status = result.status().to_string();
    let snapshot_dispose_enabled = ctx.resolve_subagent_worktree_snapshot_enabled();
    let telemetry_tokens = if result.tool_calls > 0 || result.success {
        child_handle.chat_state_handle.get_total_tokens().await
    } else {
        0
    };
    completion_data.telemetry_tokens = telemetry_tokens;
    let task_budget_usage = task_output_budget.as_ref().map(|budget| budget.usage());
    let (subagent_usage_by_model, subagent_usage_incomplete, output_tokens_used, total_tokens_used) =
        match child_handle.chat_state_handle.try_get_session_usage().await {
            Ok(u) => {
                let output_tokens = u.totals.output_tokens;
                let total_tokens = canonical_total_tokens(&u.totals);
                let has_usage_entries = !u.by_model.is_empty();
                let usage_incomplete = usage_is_incomplete(
                    u.incomplete,
                    cancellation_may_hide_usage,
                    total_tokens,
                    has_usage_entries,
                );
                (
                    Some(u.by_model.into_iter().collect::<Vec<_>>()),
                    usage_incomplete,
                    (!usage_incomplete).then_some(output_tokens),
                    Some(total_tokens),
                )
            }
            Err(()) => (None, true, None, None),
        };
    result.total_tokens_used = total_tokens_used.unwrap_or(0);
    if let Some((task_spent, task_incomplete)) = task_budget_usage {
        result.output_tokens_used = output_tokens_used.unwrap_or(task_spent);
        result.output_usage_incomplete =
            task_incomplete || subagent_usage_incomplete || output_tokens_used.is_none();
    } else {
        result.output_tokens_used = output_tokens_used.unwrap_or(0);
        result.output_usage_incomplete = subagent_usage_incomplete || output_tokens_used.is_none();
    }
    let fold_acked = record_subagent_usage(
        ctx.parent_cmd_tx.as_ref(),
        subagent_usage_by_model,
        request.parent_prompt_id.clone(),
        subagent_usage_incomplete,
    )
    .await;
    if !fold_acked {
        tracing::warn!(
            subagent_id = %request.id,
            parent_prompt_id = ?request.parent_prompt_id,
            "subagent usage not applied; parent bill marked incomplete"
        );
        let sticky_prompt = request.parent_prompt_id.clone();
        let marked_by_parent = if let Some(cmd_tx) = ctx.parent_cmd_tx.as_ref() {
            let (respond_to, ack) = tokio::sync::oneshot::channel();
            if cmd_tx
                .send(
                    crate::session::commands::SessionCommand::MarkSubagentUsageNotApplied {
                        parent_prompt_id: sticky_prompt.clone(),
                        respond_to,
                    },
                )
                .is_ok()
            {
                ack.await.is_ok()
            } else {
                false
            }
        } else {
            false
        };
        if !marked_by_parent && let Some(pid) = sticky_prompt {
            let (respond_to, ack) = tokio::sync::oneshot::channel();
            if ctx
                .subagent_event_tx
                .send(SubagentEvent::MarkUsageNotApplied(
                    SubagentMarkUsageNotAppliedRequest {
                        parent_session_id: ctx.parent_session_id.clone(),
                        prompt_id: pid,
                        respond_to,
                    },
                ))
                .is_ok()
            {
                let _ = ack.await;
            }
        }
    }
    let outcome = if result.success {
        xai_grok_telemetry::events::Outcome::Completed
    } else if result.cancelled {
        xai_grok_telemetry::events::Outcome::Cancelled
    } else {
        xai_grok_telemetry::events::Outcome::Error
    };
    xai_grok_telemetry::session_ctx::log_event(xai_grok_telemetry::events::SubagentCompleted {
        subagent_id: request.id.clone(),
        parent_session_id: request.parent_session_id.clone(),
        owner: telemetry_owner_kind(&request),
        workflow_run_id: request.owner.workflow_run_id().map(str::to_string),
        outcome,
        duration_ms: result.duration_ms,
        tool_calls: result.tool_calls,
        tokens_used: if telemetry_tokens > 0 {
            Some(telemetry_tokens)
        } else {
            None
        },
    });
    match (
        &ctx.parent_terminal_backend,
        &ctx.parent_notification_handle,
    ) {
        (Some(parent_tb), Some(parent_notif_handle)) => {
            if !request.surface_completion {
                let reparented_task_ids: Vec<String> = parent_tb
                    .list_tasks()
                    .await
                    .into_iter()
                    .filter(|t| {
                        !t.completed && t.owner_session_id.as_deref() == Some(&*child_session_id.0)
                    })
                    .map(|t| t.task_id)
                    .collect();
                if !reparented_task_ids.is_empty()
                    && let Some(cmd_tx) = ctx.parent_cmd_tx.as_ref()
                {
                    let _ = cmd_tx.send(SessionCommand::RecordGoalTurnTaskIds {
                        task_ids: reparented_task_ids,
                    });
                }
            }
            let parent_backend_weak = std::sync::Arc::downgrade(parent_tb);
            parent_tb
                .reparent_notifications(
                    &child_session_id.0,
                    &ctx.parent_session_id,
                    parent_notif_handle.clone(),
                    parent_backend_weak,
                )
                .await;
        }
        (Some(_), None) | (None, Some(_)) => {
            tracing::warn!(
                child_session_id = %child_session_id.0,
                parent_session_id = %ctx.parent_session_id,
                has_terminal_backend = ctx.parent_terminal_backend.is_some(),
                has_notification_handle = ctx.parent_notification_handle.is_some(),
                "skipping reparent_notifications: parent_terminal_backend and \
                 parent_notification_handle must both be Some"
            );
        }
        (None, None) => {}
    }
    let _ = child_handle.cmd_tx.send(SessionCommand::Shutdown(
        crate::session::ShutdownKind::Graceful,
    ));
    ctx.workspace_ops
        .end_local_session(child_session_id.0.as_ref());
    let mut disposed_snapshot_ref: Option<String> = None;
    let mut worktree_removed = false;
    let retain_worktree = request.runtime_overrides.retain_worktree == Some(true);
    if let Some(ref wt_path) = worktree_path {
        if snapshot_dispose_enabled {
            let ref_name = format!("refs/grok/subagents/{}", request.id);
            let source_repo = resolve_subagent_source_repo(&ctx, request.cwd.as_deref());
            match crate::session::worktree::snapshot_subagent_worktree(
                wt_path,
                &source_repo,
                &ref_name,
            )
            .await
            {
                Ok(snapshot_ref) => {
                    // Export changes.patch + diffstat before optional removal so
                    // recovery artifacts survive cleanup.
                    let mut patch_path: Option<String> = None;
                    let mut diffstat_summary: Option<String> = None;
                    // Prefer spawn baseline so patch is agent-only (RC9).
                    let baseline_for_export = spawn_baseline_ref.clone().or_else(|| {
                        std::fs::read_to_string(subagent_meta_dir.join("meta.json"))
                            .ok()
                            .and_then(|raw| {
                                serde_json::from_str::<SubagentMeta>(&raw)
                                    .ok()
                                    .and_then(|m| m.baseline_ref)
                            })
                    });
                    match crate::session::worktree::export_subagent_changes_patch_with_baseline(
                        Some(wt_path.as_path()),
                        &source_repo,
                        &snapshot_ref,
                        baseline_for_export.as_deref(),
                        &subagent_meta_dir,
                    )
                    .await
                    {
                        Ok(exported) => {
                            patch_path = Some(exported.patch_path.to_string_lossy().into_owned());
                            diffstat_summary = Some(exported.diffstat_summary);
                            // Load top changed paths for completion summary.
                            if let Ok(names) =
                                std::fs::read_to_string(subagent_meta_dir.join("changed_paths.txt"))
                            {
                                let paths: Vec<String> = names
                                    .lines()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .filter(|s| {
                                        !xai_grok_tools::implementations::grok_build::subagent_worktree::is_harness_land_path(s)
                                    })
                                    .take(20)
                                    .map(str::to_string)
                                    .collect();
                                if !paths.is_empty() {
                                    result.changed_paths = Some(paths.clone());
                                }
                            }
                            tracing::info!(
                                subagent_id = %request.id,
                                patch_path = ?patch_path,
                                files_changed = exported.files_changed,
                                insertions = exported.insertions,
                                deletions = exported.deletions,
                                baseline = ?baseline_for_export,
                                "exported subagent changes.patch (agent-only when baseline set)"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                subagent_id = %request.id,
                                worktree_path = %wt_path.display(),
                                error = %e,
                                "failed to export subagent changes.patch; continuing dispose"
                            );
                        }
                    }

                    // Soft-preserve by default so trees don't vanish mid-review.
                    // Set GROK_SUBAGENT_SOFT_PRESERVE=0 to restore immediate delete.
                    let soft_preserve = !retain_worktree
                        && std::env::var("GROK_SUBAGENT_SOFT_PRESERVE")
                            .map(|v| {
                                !matches!(
                                    v.trim().to_ascii_lowercase().as_str(),
                                    "0" | "false" | "off" | "no"
                                )
                            })
                            .unwrap_or(true);
                    let keep_live = retain_worktree || soft_preserve;

                    // Persist snapshot_ref + patch first (resume-critical).
                    let persisted = update_subagent_meta_dispose(
                        &subagent_meta_dir,
                        &SubagentMetaDisposeUpdate {
                            snapshot_ref: Some(snapshot_ref.clone()),
                            baseline_ref: baseline_for_export.clone(),
                            status: Some(final_status.clone()),
                            worktree_state: Some(if keep_live {
                                "preserved".to_string()
                            } else {
                                "preserved".to_string()
                            }),
                            patch_path: patch_path.clone(),
                            diffstat: diffstat_summary.clone(),
                            changed_paths: result.changed_paths.clone(),
                            land_status: Some("pending".to_string()),
                            clear_worktree_path: false,
                        },
                    );
                    if persisted {
                        disposed_snapshot_ref = Some(snapshot_ref.clone());
                        result.snapshot_ref = Some(snapshot_ref);
                        result.patch_path = patch_path;
                        result.diffstat = diffstat_summary;
                        result.baseline_ref = baseline_for_export.clone();
                        if keep_live {
                            result.worktree_state = Some("preserved".to_string());
                            // Child finished: drop live marker so keep-N may reclaim
                            // this tree on later spawns (never while running).
                            clear_live_worktree_marker(wt_path);
                            tracing::info!(
                                subagent_id = %request.id,
                                worktree_path = %wt_path.display(),
                                retain_worktree,
                                soft_preserve,
                                "snapshotted subagent worktree; kept on disk for review"
                            );
                            // Best-effort free-space reclaim after densify dispose
                            // (debounced; never blocks completion).
                            maybe_post_subagent_disk_clean();
                            // Evict oldest soft-preserved peers so densify waves
                            // stay within keep-N without waiting for the next spawn.
                            if soft_preserve {
                                if let Some(base) = wt_path.parent() {
                                    prune_soft_preserved_worktrees(base);
                                }
                            }
                        } else {
                            match crate::session::worktree::remove_subagent_worktree(wt_path).await
                            {
                                Ok(()) => {
                                    worktree_removed = true;
                                    result.worktree_state = Some("cleaned".to_string());
                                    let _ = update_subagent_meta_dispose(
                                        &subagent_meta_dir,
                                        &SubagentMetaDisposeUpdate {
                                            worktree_state: Some("cleaned".to_string()),
                                            clear_worktree_path: true,
                                            ..Default::default()
                                        },
                                    );
                                    tracing::info!(
                                        subagent_id = %request.id,
                                        worktree_path = %wt_path.display(),
                                        "snapshotted and removed subagent worktree"
                                    );
                                    maybe_post_subagent_disk_clean();
                                }
                                Err(e) => {
                                    result.worktree_state = Some("preserved".to_string());
                                    tracing::warn!(
                                        subagent_id = %request.id,
                                        worktree_path = %wt_path.display(),
                                        error = %e,
                                        "snapshotted subagent worktree but removal failed; ref persisted for resume"
                                    );
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            subagent_id = %request.id,
                            worktree_path = %wt_path.display(),
                            "snapshot_ref not persisted; preserving worktree for resume"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        subagent_id = %request.id,
                        worktree_path = %wt_path.display(),
                        error = %e,
                        "Failed to snapshot subagent worktree; preserving for review"
                    );
                }
            }
        } else {
            result.worktree_state = Some("preserved".to_string());
            tracing::info!(
                subagent_id = %request.id,
                worktree_path = %wt_path.display(),
                "Worktree preserved for review"
            );
        }
    }
    if worktree_removed {
        result.worktree_path = None;
    }
    // Auto Developer Log: structured product incidents for Turbo maintainers.
    {
        let meta_path = subagent_meta_dir.join("meta.json");
        let _ = xai_grok_developer_log::detect_worktree_dispose(
            &xai_grok_developer_log::WorktreeDisposeSignal {
                subagent_id: request.id.to_string(),
                parent_session_id: Some(request.parent_session_id.clone()),
                session_id: Some(request.id.to_string()),
                worktree_path: result.worktree_path.clone(),
                worktree_removed,
                worktree_state: result.worktree_state.clone(),
                snapshot_ref: disposed_snapshot_ref
                    .clone()
                    .or_else(|| result.snapshot_ref.clone()),
                patch_path: result.patch_path.clone(),
                meta_path: Some(meta_path.display().to_string()),
                model: Some(tracker_model_id.clone()),
                provider: None,
            },
        );
        if result.isolation_fallback {
            let _ = xai_grok_developer_log::detect_isolation_fallback(
                &xai_grok_developer_log::IsolationFallbackSignal {
                    subagent_id: request.id.to_string(),
                    session_id: Some(request.parent_session_id.clone()),
                    reason: "worktree create/resume fell back to shared parent cwd".into(),
                },
            );
        }
        if matches!(
            result.error_class.as_deref(),
            Some("stall" | "timeout" | "budget")
        ) {
            let _ = xai_grok_developer_log::detect_subagent_stall(
                &xai_grok_developer_log::StallSignal {
                    subagent_id: request.id.to_string(),
                    session_id: Some(request.id.to_string()),
                    parent_session_id: Some(request.parent_session_id.clone()),
                    model: Some(tracker_model_id.clone()),
                    provider: None,
                    duration_ms: Some(result.duration_ms),
                    last_tool: None,
                    reason: result
                        .error
                        .clone()
                        .or_else(|| result.error_class.clone())
                        .unwrap_or_else(|| "subagent stalled or timed out".into()),
                },
            );
        }
    }
    // Normal completion path owns dispose/preserve; do not double-remove.
    fresh_worktree_guard.disarm();
    let success = result.success && !result.cancelled;
    let preview = crate::util::truncate(&result.output, 200);
    let level_fn = if success {
        xai_grok_telemetry::unified_log::info
    } else {
        xai_grok_telemetry::unified_log::error
    };
    level_fn(
        if success {
            "subagent completed"
        } else {
            "subagent failed"
        },
        None,
        Some(serde_json::json!({
            "subagent_id": &request.id,
            "subagent_type": &request.subagent_type,
            "effective_model": tracker_model_id,
            "success": success,
            "cancelled": result.cancelled,
            "duration_ms": result.duration_ms,
            "turns": result.turns,
            "tool_calls": result.tool_calls,
            "output_preview": preview,
            "error": &result.error,
        })),
    );
    child_run_output(result, completion_data, disposed_snapshot_ref)
}

// Soft-preserve keep-N + free-space helpers live in `super` (mod.rs) so unit
// tests can exercise live-marker protection without async spawn.

#[cfg(test)]
mod cargo_env_tests {
    use super::inject_worktree_cargo_env;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn worktree_cargo_env_overwrites_parent_target_dir() {
        let mut env = HashMap::new();
        env.insert(
            "CARGO_TARGET_DIR".into(),
            r"H:\Apps\grok build\turbo-grok-build\target".into(),
        );
        let wt = PathBuf::from(r"C:\Users\x\.grok\worktrees\repo\subagent-01abc");
        inject_worktree_cargo_env(&mut env, &wt);
        let pinned = env.get("CARGO_TARGET_DIR").unwrap();
        assert!(
            pinned.contains("subagent-01abc"),
            "expected worktree target, got {pinned}"
        );
        assert!(!pinned.contains(r"H:\Apps"));
        assert_eq!(env.get("GROK_WORKTREE_CARGO_TARGET"), Some(pinned));
    }
}
