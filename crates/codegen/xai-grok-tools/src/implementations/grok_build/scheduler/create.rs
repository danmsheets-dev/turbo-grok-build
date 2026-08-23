use crate::types::requirements::{Expr, ToolRequirement};

use crate::types::tool::{ToolKind, ToolNamespace};

use super::interval::{interval_to_human, parse_interval, task_human_schedule};
use super::types::{ScheduledTask, SchedulerCommand, SchedulerHandle, scheduler_tool_error};
use super::when::{
    AtSpec, next_weekday_clock, next_weekly_clock, parse_at, seconds_until,
};

// Canonical /loop and /schedule wording lives in the light API crate so other
// consumers can link it without the tools implementation crate; re-exported
// to keep paths stable.
pub use xai_grok_tools_api::slash_commands::{
    LoopFireMode, SCHEDULE_COMMAND_NAME, SCHEDULER_CREATE_TOOL_NAME, SCHEDULER_LIST_TOOL_NAME,
    ScheduleRecipe, ScheduleVerb, expand_schedule_recipe, loop_schedule_instruction,
    loop_usage_message, parse_schedule_recipe, parse_schedule_verb, schedule_instruction,
    schedule_usage_message,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct SchedulerCreateInput {
    #[serde(default)]
    #[schemars(
        description = "Id of an existing task to update in place: provided fields replace old \
                       values, omitted ones are unchanged, the schedule keeps its phase, and an \
                       unknown id errors. Omit to create a task."
    )]
    pub task_id: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Interval between executions, e.g. \"5m\", \"2h\", \"1d\". \
                       Required to create; optional with task_id"
    )]
    pub interval: Option<String>,

    #[serde(default)]
    #[schemars(description = "The prompt text to execute on each scheduled fire. \
                       Required to create; optional with task_id")]
    pub prompt: Option<String>,

    #[serde(
        default = "default_true",
        deserialize_with = "crate::types::schema::deserialize_lenient_bool"
    )]
    #[schemars(skip)]
    pub recurring: bool,

    /// Whether the task persists across sessions. Default false (session-only).
    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_option_bool"
    )]
    #[schemars(
        description = "Whether the task persists across sessions. Default: false. \
                       Create-only: ignored with task_id"
    )]
    pub durable: Option<bool>,

    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_option_bool"
    )]
    #[schemars(
        description = "Run each fire as a main-conversation turn instead of a background \
                       subagent; set true only when runs need the conversation's context. \
                       Default: false. Create-only: ignored with task_id"
    )]
    pub foreground: Option<bool>,

    /// Whether to fire immediately on creation. Default false (wait for the
    /// first interval — a "scheduled" task should not run on creation unless
    /// explicitly asked to).
    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_bool"
    )]
    #[schemars(
        description = "Whether to fire immediately on creation (true) or wait for the first \
                       interval (false). Default: false. Create-only: ignored with task_id"
    )]
    pub fire_immediately: bool,

    #[serde(default)]
    #[schemars(
        description = "Optional human title for Schedules/YYYY-MM-DD - <title>.md result files."
    )]
    pub title: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "One-shot datetime (ISO-8601 / 2026-08-24T09:00) or a weekday clock \
                       (`weekday 08:00`, `monday 09:00`). One-shot: interval = seconds until \
                       then (min 60s), fires once. Weekday clocks are standing. \
                       Create-only: ignored with task_id."
    )]
    pub at: Option<String>,

    /// Standing `/schedule` jobs skip the 7-day `/loop` expiry.
    #[serde(
        default,
        alias = "no_expire",
        deserialize_with = "crate::types::schema::deserialize_lenient_option_bool"
    )]
    #[schemars(
        description = "If true, the job does not auto-expire after 7 days (`expires_at = None`). \
                       Use for /schedule. /loop leaves this unset and keeps the 7-day cap. \
                       Create-only: ignored with task_id."
    )]
    pub standing: Option<bool>,

    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_option_bool"
    )]
    #[schemars(
        description = "Meeting-join recipe: fire with meeting tools (not read-only). \
                       Create-only: ignored with task_id."
    )]
    pub meeting_join: Option<bool>,

    #[serde(
        default,
        deserialize_with = "crate::types::schema::deserialize_lenient_option_bool"
    )]
    #[schemars(
        description = "Required true when creating a meeting-join schedule. The operator must \
                       approve the first join; later fires reuse this confirmation. \
                       Create-only: ignored with task_id."
    )]
    pub confirm: Option<bool>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerCreateOutput {
    pub id: String,
    pub human_schedule: String,
    #[serde(default)]
    pub updated: bool,
}

impl xai_tool_runtime::ToolOutput for SchedulerCreateOutput {}

#[derive(Debug, Default)]
pub struct SchedulerCreateTool;

impl crate::types::tool_metadata::ToolMetadata for SchedulerCreateTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Create a scheduled task that runs a prompt on a recurring interval, or update an existing one in place.

Set fire_immediately: true to also fire once on creation; by default the first run waits for the interval.

To change an existing task, pass its task_id: provided fields replace old values, omitted ones are unchanged, and the schedule keeps its phase. An unknown id errors.

Usage notes:
- Interval format: "5m" (minutes), "2h" (hours), "1d" (days), "60s" (seconds, min 60)
- Optional `at`: one-shot datetime (ISO-8601 / 2026-08-24T09:00) or weekday clock (`weekday 08:00`)
- Optional `standing`/`no_expire`: skip the 7-day expiry (use for /schedule; /loop keeps the cap)
- Optional `title`: used for Schedules/YYYY-MM-DD - <title>.md result files
- Maximum 50 scheduled tasks at once
- /loop jobs auto-expire after 7 days; standing /schedule jobs do not
- One-shot `at` fires once (interval = seconds until then, min 60s)"#
        // TODO: scheduler tools share ToolKind::Other so they can't be template-ized
        // via ${{ tools.by_kind.* }}. If tool name randomization is needed, add
        // dedicated ToolKind variants (SchedulerCreate, SchedulerDelete, SchedulerList).
    }

    fn emitted_notifications(&self) -> &'static [&'static str] {
        &["ScheduledTaskCreated"]
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for SchedulerCreateTool {
    type Args = SchedulerCreateInput;
    type Output = SchedulerCreateOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(SCHEDULER_CREATE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            "scheduler_create",
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.scheduler_create",
        skip_all,
        fields(interval = input.interval.as_deref().unwrap_or(""), task_id = input.task_id.as_deref().unwrap_or(""))
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: SchedulerCreateInput,
    ) -> Result<SchedulerCreateOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        let interval_secs = input
            .interval
            .as_deref()
            .map(parse_interval)
            .transpose()
            .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;

        let sender = {
            let res = resources.lock().await;
            res.get::<SchedulerHandle>()
                .ok_or_else(|| {
                    xai_tool_runtime::ToolError::custom("missing_resource", "SchedulerHandle")
                })?
                .0
                .clone()
        };

        let send_and_wait = |cmd: SchedulerCommand,
                             reply_rx: tokio::sync::oneshot::Receiver<
            Result<ScheduledTask, super::types::SchedulerError>,
        >| async move {
            sender.send(cmd).map_err(|_| {
                xai_tool_runtime::ToolError::custom("process_manager", "Scheduler actor stopped")
            })?;
            reply_rx
                .await
                .map_err(|_| {
                    xai_tool_runtime::ToolError::custom(
                        "process_manager",
                        "Scheduler actor dropped reply",
                    )
                })?
                .map_err(scheduler_tool_error)
        };

        if let Some(task_id) = input.task_id {
            if input.prompt.is_none() && interval_secs.is_none() {
                return Err(xai_tool_runtime::ToolError::invalid_arguments(
                    "nothing to update: provide interval and/or prompt alongside task_id",
                ));
            }
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let updated = send_and_wait(
                SchedulerCommand::Update {
                    id: task_id,
                    prompt: input.prompt,
                    interval_secs,
                    reply: reply_tx,
                },
                reply_rx,
            )
            .await?;

            return Ok(SchedulerCreateOutput {
                id: updated.id,
                human_schedule: interval_to_human(updated.interval_secs),
                updated: true,
            });
        }

        let at_spec = input
            .at
            .as_deref()
            .map(parse_at)
            .transpose()
            .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;

        if !input.recurring && at_spec.is_none() {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "one-shot tasks require `at` (ISO-8601 / 2026-08-24T09:00); \
                 otherwise run a background terminal command \
                 (`sleep <secs> && <command>`, background: true) or do the work now",
            ));
        }

        if at_spec.is_none() && interval_secs.is_none() {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "interval is required when creating a task (or pass `at` for a one-shot)",
            ));
        }

        let prompt_in = input.prompt.ok_or_else(|| {
            xai_tool_runtime::ToolError::invalid_arguments(
                "prompt is required when creating a task",
            )
        })?;
        let recipe = parse_schedule_recipe(&prompt_in);
        let is_recipe = !matches!(recipe, ScheduleRecipe::Freeform { .. });
        let (expanded, recipe_title, recipe_meeting) = expand_schedule_recipe(&prompt_in);

        let now = chrono::Utc::now();
        let standing = input.standing.unwrap_or(false);
        let meeting_join = input.meeting_join.unwrap_or(false) || recipe_meeting;
        let prompt = if is_recipe || standing {
            expanded
        } else {
            prompt_in
        };
        if meeting_join && !input.confirm.unwrap_or(false) {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "meeting-join schedules require confirm=true after the operator approved \
                 this job (one-time). Do not create the timer until they confirm.",
            ));
        }
        let mut weekdays_only = false;
        let mut first_fire: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut recurring = true;

        let interval_secs = match at_spec {
            Some(AtSpec::Once(dt)) => {
                let delay = seconds_until(now, dt)
                    .map_err(|e| xai_tool_runtime::ToolError::invalid_arguments(e.to_string()))?;
                first_fire = Some(dt);
                if let Some(secs) = interval_secs {
                    // Recurring standing job whose first fire is `at`.
                    secs
                } else {
                    recurring = false;
                    delay
                }
            }
            Some(AtSpec::Weekdays(time)) => {
                weekdays_only = true;
                first_fire = Some(next_weekday_clock(now, time));
                interval_secs.unwrap_or(86_400)
            }
            Some(AtSpec::Weekly(weekday, time)) => {
                first_fire = Some(next_weekly_clock(now, weekday, time));
                interval_secs.unwrap_or(86_400 * 7)
            }
            None => interval_secs.ok_or_else(|| {
                xai_tool_runtime::ToolError::invalid_arguments(
                    "interval is required when creating a task (or pass `at` for a one-shot)",
                )
            })?,
        };

        let durable = input.durable.unwrap_or(false);
        let mut task = ScheduledTask::with_fire_immediately(
            interval_secs,
            prompt,
            recurring,
            durable,
            input.fire_immediately && first_fire.is_none(),
        );
        task.foreground = input.foreground.unwrap_or(false);
        task.title = input
            .title
            .filter(|t| !t.trim().is_empty())
            .or_else(|| {
                if is_recipe || standing {
                    Some(recipe_title)
                } else {
                    None
                }
            });
        task.weekdays_only = weekdays_only;
        task.meeting_join = meeting_join;
        if standing || !recurring || weekdays_only || at_spec.is_some() {
            // /schedule jobs: no 7-day expiry. /loop leaves standing unset.
            if standing || !recurring || weekdays_only {
                task.apply_standing();
            }
        }
        if standing || weekdays_only || at_spec.is_some() {
            if meeting_join {
                // Meeting tools are ToolKind::Other (clamped by ReadOnly/ReadWrite).
                task.isolation = Some(xai_tool_types::SubagentIsolationMode::None);
                task.capability_mode = Some(xai_tool_types::SubagentCapabilityMode::All);
            } else {
                // Parent cwd + Write/WebSearch, jailed to Schedules/ at write time.
                task.isolation = Some(xai_tool_types::SubagentIsolationMode::None);
                task.capability_mode = Some(xai_tool_types::SubagentCapabilityMode::ReadWrite);
            }
        }
        if let Some(first) = first_fire {
            if first <= now {
                return Err(xai_tool_runtime::ToolError::invalid_arguments(
                    "at time must be in the future",
                ));
            }
            task.anchor_first_fire(first);
        }

        let human_schedule = task_human_schedule(&task);

        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let created = send_and_wait(
            SchedulerCommand::Create {
                task,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await?;

        Ok(SchedulerCreateOutput {
            id: created.id,
            human_schedule,
            updated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::implementations::grok_build::scheduler::actor::SchedulerActor;
    use crate::notification::types::ToolNotificationHandle;
    use crate::types::resources::{Resources, SharedResources, State};
    use crate::types::tool_metadata::test_ctx;
    use xai_tool_runtime::Tool;

    fn scheduler_resources() -> (SharedResources, tokio_util::sync::CancellationToken) {
        let mut resources = Resources::new();
        resources.register_state::<super::super::types::SchedulerState>();
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        resources.insert(SchedulerHandle(cmd_tx));
        let shared = resources.into_shared();

        let (notif_handle, _notif_rx) = ToolNotificationHandle::channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let actor = SchedulerActor {
            resources: shared.clone(),
            resources_persistence: std::sync::Arc::new(
                crate::persistence::ResourcesPersistence::noop(),
            ),
            notification_handle: notif_handle,
            cmd_rx,
            cancel_token: cancel_token.clone(),
            clock: Default::default(),
            pending_removal: None,
            blocked_expiries: Default::default(),
        };
        tokio::spawn(actor.run());
        (shared, cancel_token)
    }

    fn scheduler_resources_with_cwd(
        cwd: std::path::PathBuf,
    ) -> (SharedResources, tokio_util::sync::CancellationToken) {
        let mut resources = Resources::new();
        resources.register_state::<super::super::types::SchedulerState>();
        resources.insert(crate::types::resources::Cwd(cwd));
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        resources.insert(SchedulerHandle(cmd_tx));
        let shared = resources.into_shared();

        let (notif_handle, _notif_rx) = ToolNotificationHandle::channel();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let actor = SchedulerActor {
            resources: shared.clone(),
            resources_persistence: std::sync::Arc::new(
                crate::persistence::ResourcesPersistence::noop(),
            ),
            notification_handle: notif_handle,
            cmd_rx,
            cancel_token: cancel_token.clone(),
            clock: Default::default(),
            pending_removal: None,
            blocked_expiries: Default::default(),
        };
        tokio::spawn(actor.run());
        (shared, cancel_token)
    }

    fn input(json: serde_json::Value) -> SchedulerCreateInput {
        serde_json::from_value(json).expect("valid input json")
    }

    async fn task_count(resources: &SharedResources) -> usize {
        let res = resources.lock().await;
        res.get::<State<super::super::types::SchedulerState>>()
            .map(|s| s.tasks.len())
            .unwrap_or(0)
    }

    #[tokio::test]
    async fn create_requires_interval_and_prompt() {
        let (resources, cancel) = scheduler_resources();

        let err = SchedulerCreateTool
            .run(test_ctx(resources.clone()), input(serde_json::json!({})))
            .await
            .expect_err("create without interval must fail");
        assert!(err.to_string().contains("interval is required"));

        let err = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({"interval": "5m"})),
            )
            .await
            .expect_err("create without prompt must fail");
        assert!(err.to_string().contains("prompt is required"));

        assert_eq!(task_count(&resources).await, 0);
        cancel.cancel();
    }

    #[tokio::test]
    async fn recurring_false_errors_with_sleep_guidance() {
        let (resources, cancel) = scheduler_resources();

        let err = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "5m", "prompt": "check", "recurring": false
                })),
            )
            .await
            .expect_err("one-shot must be rejected");
        assert!(err.to_string().contains("sleep"), "steers to sleep: {err}");
        assert_eq!(task_count(&resources).await, 0);
        cancel.cancel();
    }

    #[tokio::test]
    async fn update_unknown_task_id_errors_and_never_creates() {
        let (resources, cancel) = scheduler_resources();

        let err = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "task_id": "nonexistent", "prompt": "new prompt"
                })),
            )
            .await
            .expect_err("unknown id must error");
        assert!(err.to_string().contains("no scheduled task with id"));
        assert_eq!(
            task_count(&resources).await,
            0,
            "strict update must not fall back to create"
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn update_ignores_legacy_recurring_flag() {
        let (resources, cancel) = scheduler_resources();

        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({"interval": "5m", "prompt": "check deploy"})),
            )
            .await
            .expect("create succeeds");

        let updated = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "task_id": created.id, "interval": "10m", "recurring": false
                })),
            )
            .await
            .expect("update succeeds despite legacy flag");
        assert!(updated.updated);
        assert_eq!(updated.human_schedule, "every 10 minutes");
        cancel.cancel();
    }

    #[tokio::test]
    async fn update_with_no_patch_fields_errors() {
        let (resources, cancel) = scheduler_resources();

        let err = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({"task_id": "abc123"})),
            )
            .await
            .expect_err("empty patch must error");
        assert!(err.to_string().contains("nothing to update"));
        cancel.cancel();
    }

    #[tokio::test]
    async fn create_then_update_patches_in_place() {
        let (resources, cancel) = scheduler_resources();

        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({"interval": "5m", "prompt": "check deploy"})),
            )
            .await
            .expect("create succeeds");
        assert!(!created.updated);
        assert_eq!(created.human_schedule, "every 5 minutes");

        let updated = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({"task_id": created.id, "interval": "10m"})),
            )
            .await
            .expect("update succeeds");
        assert!(updated.updated);
        assert_eq!(updated.id, created.id, "identity preserved");
        assert_eq!(updated.human_schedule, "every 10 minutes");
        assert_eq!(task_count(&resources).await, 1, "no second task");
        cancel.cancel();
    }

    #[test]
    fn schema_hides_recurring_and_advertises_task_id() {
        let schema = schemars::schema_for!(SchedulerCreateInput);
        let json = serde_json::to_string(&schema).unwrap();
        assert!(
            !json.contains("recurring"),
            "recurring must not be advertised: {json}"
        );
        assert!(json.contains("task_id"));
    }

    #[test]
    fn loop_usage_message_has_no_host_default() {
        let usage = loop_usage_message();
        assert!(usage.contains("Usage: /loop"));
        assert!(
            !usage.contains("10m"),
            "usage must not claim a default: {usage}"
        );
    }

    #[test]
    fn loop_schedule_instruction_holds_invariants() {
        let args = "every 30 minutes do x";
        let instr = loop_schedule_instruction(args, LoopFireMode::Detached);
        assert!(
            !instr.contains("10m"),
            "instruction must not default: {instr}"
        );
        assert!(instr.contains("Deriving the interval"));
        assert!(instr.contains("<number><unit>"));
        assert!(instr.contains("ask the user how often"));
        assert!(instr.contains("Do NOT execute the prompt inline"));
        // Raw request forwarded verbatim for the model to parse.
        assert!(instr.contains(args));
    }

    async fn created_task(
        resources: &crate::types::resources::SharedResources,
        id: &str,
    ) -> super::super::types::ScheduledTask {
        let res = resources.lock().await;
        res.get::<crate::types::resources::State<super::super::types::SchedulerState>>()
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .expect("task")
    }

    #[tokio::test]
    async fn standing_job_has_no_7_day_expiry() {
        let (resources, cancel) = scheduler_resources();
        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "5m",
                    "prompt": "search rust async",
                    "standing": true,
                    "durable": true,
                    "title": "rust async"
                })),
            )
            .await
            .expect("create succeeds");
        let task = created_task(&resources, &created.id).await;
        assert!(task.standing);
        assert!(task.expires_at.is_none(), "standing job must not expire");
        assert!(task.durable);
        assert_eq!(
            task.isolation,
            Some(xai_tool_types::SubagentIsolationMode::None)
        );
        assert_eq!(
            task.capability_mode,
            Some(xai_tool_types::SubagentCapabilityMode::ReadWrite)
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn meeting_join_requires_confirm() {
        let (resources, cancel) = scheduler_resources();
        let err = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "1d",
                    "prompt": "meeting join https://teams.microsoft.com/meet/1",
                    "standing": true,
                    "durable": true
                })),
            )
            .await
            .expect_err("unconfirmed meeting-join must fail");
        assert!(
            err.to_string().contains("confirm=true"),
            "steers to confirm: {err}"
        );
        assert_eq!(task_count(&resources).await, 0);
        cancel.cancel();
    }

    #[tokio::test]
    async fn search_recipe_is_expanded() {
        let (resources, cancel) = scheduler_resources();
        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "1h",
                    "prompt": "search rust async",
                    "standing": true,
                    "durable": true
                })),
            )
            .await
            .expect("create succeeds");
        let task = created_task(&resources, &created.id).await;
        assert!(
            task.prompt.contains("Search the web"),
            "stored prompt must be expanded: {}",
            task.prompt
        );
        assert!(!task.meeting_join);
        assert_eq!(
            task.capability_mode,
            Some(xai_tool_types::SubagentCapabilityMode::ReadWrite)
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn standing_job_writes_workspace_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let (resources, cancel) = scheduler_resources_with_cwd(dir.path().to_path_buf());
        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "1h",
                    "prompt": "search rust async",
                    "standing": true,
                    "durable": true,
                    "title": "rust async"
                })),
            )
            .await
            .expect("create succeeds");
        // Actor persist is async after Create; poll the index briefly.
        let path = super::super::disk::schedule_index_path(dir.path());
        let mut found = false;
        for _ in 0..50 {
            if path.exists() {
                if let Ok(idx) = super::super::disk::load_schedule_index(dir.path()) {
                    if idx.tasks.iter().any(|t| t.id == created.id) {
                        found = true;
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(found, "standing create must write {}", path.display());
        cancel.cancel();
    }

    #[tokio::test]
    async fn one_shot_at_is_in_the_future() {
        let (resources, cancel) = scheduler_resources();
        let at = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "at": at,
                    "prompt": "brief the morning",
                    "standing": true,
                    "durable": true,
                    "title": "morning brief"
                })),
            )
            .await
            .expect("create succeeds");
        let task = created_task(&resources, &created.id).await;
        assert!(!task.recurring);
        assert!(task.expires_at.is_none());
        assert!(task.next_fire_at() > chrono::Utc::now());
        assert!(
            created.human_schedule.starts_with("at "),
            "one-shot schedule: {}",
            created.human_schedule
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn past_at_is_rejected() {
        let (resources, cancel) = scheduler_resources();
        let at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let err = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "at": at,
                    "prompt": "too late"
                })),
            )
            .await
            .expect_err("past at must fail");
        assert!(
            err.to_string().contains("future"),
            "steers to future: {err}"
        );
        assert_eq!(task_count(&resources).await, 0);
        cancel.cancel();
    }

    #[tokio::test]
    async fn meeting_join_sets_flag_and_write_capability() {
        let (resources, cancel) = scheduler_resources();
        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "1d",
                    "prompt": "call meeting_join",
                    "standing": true,
                    "meeting_join": true,
                    "confirm": true,
                    "durable": true
                })),
            )
            .await
            .expect("create succeeds");
        let task = created_task(&resources, &created.id).await;
        assert!(task.meeting_join);
        assert_eq!(
            task.isolation,
            Some(xai_tool_types::SubagentIsolationMode::None)
        );
        assert_eq!(
            task.capability_mode,
            Some(xai_tool_types::SubagentCapabilityMode::All)
        );
        cancel.cancel();
    }

    #[tokio::test]
    async fn loop_create_keeps_7_day_expiry_and_no_worktree() {
        let (resources, cancel) = scheduler_resources();
        let created = SchedulerCreateTool
            .run(
                test_ctx(resources.clone()),
                input(serde_json::json!({
                    "interval": "5m",
                    "prompt": "check deploy",
                    "fire_immediately": true
                })),
            )
            .await
            .expect("create succeeds");
        let task = created_task(&resources, &created.id).await;
        assert!(!task.standing);
        assert!(task.expires_at.is_some());
        assert!(task.isolation.is_none());
        assert!(task.capability_mode.is_none());
        cancel.cancel();
    }
}
