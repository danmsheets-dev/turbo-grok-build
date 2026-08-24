use agent_client_protocol as acp;
use xai_grok_tools::implementations::grok_build::scheduler::create::{
    SCHEDULE_COMMAND_NAME, SCHEDULER_CREATE_TOOL_NAME, SCHEDULER_LIST_TOOL_NAME, ScheduleVerb,
    parse_schedule_verb, schedule_instruction, schedule_usage_message,
};
use xai_grok_tools::implementations::grok_build::SCHEDULER_DELETE_TOOL_NAME;

use crate::slash::command::{
    AppCtx, ArgItem, CommandExecCtx, CommandResult, ScheduledTaskPreview, SlashCommand,
};

const SCHEDULE_REQUIRED_TOOLS: &[&str] = &[
    SCHEDULER_CREATE_TOOL_NAME,
    SCHEDULER_LIST_TOOL_NAME,
    SCHEDULER_DELETE_TOOL_NAME,
];

pub struct ScheduleCommand;

impl SlashCommand for ScheduleCommand {
    fn name(&self) -> &str {
        SCHEDULE_COMMAND_NAME
    }

    fn description(&self) -> &str {
        "Standing scheduled jobs (interval or at-time; no 7-day expiry)"
    }

    fn usage(&self) -> &str {
        "/schedule [at|every] <when> <prompt-or-recipe>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[at|every] <when> <prompt-or-recipe> | list | show <id> | cancel <id>")
    }

    fn required_tools(&self) -> &[&str] {
        SCHEDULE_REQUIRED_TOOLS
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(vec![
            ArgItem::new("list", "list", "list", "List standing scheduled jobs"),
            ArgItem::new("show", "show", "show ", "Show one job by id"),
            ArgItem::new("cancel", "cancel", "cancel ", "Cancel a job by id"),
            ArgItem::new("at", "at", "at ", "One-shot datetime (ISO-8601)"),
            ArgItem::new("every", "every", "every ", "Recurring interval or weekday clock"),
        ])
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim().is_empty() {
            return CommandResult::Message(schedule_usage_message().to_string());
        }

        let preview = match parse_schedule_verb(args) {
            ScheduleVerb::Create { rest } => {
                let human_schedule = preview_human_schedule(rest);
                Some(ScheduledTaskPreview {
                    prompt: rest.to_string(),
                    human_schedule,
                    next_fire_at: None,
                    tag: "schedule".into(),
                })
            }
            _ => None,
        };

        CommandResult::InjectSkill {
            display_text: format!("/schedule {args}"),
            prompt_blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(
                schedule_instruction(args),
            ))],
            display_as_skill: false,
            scheduled_task_preview: preview,
            task_id: None,
        }
    }
}

fn preview_human_schedule(rest: &str) -> String {
    let trimmed = rest.trim();
    let (first, after) = trimmed
        .split_once(char::is_whitespace)
        .map(|(a, b)| (a, b.trim()))
        .unwrap_or((trimmed, ""));
    let lower = first.to_ascii_lowercase();
    if is_interval_token(first) && !after.is_empty() {
        return interval_token_to_human(first);
    }
    if lower == "at" && !after.is_empty() {
        let when = after
            .split_once(char::is_whitespace)
            .map(|(w, _)| w)
            .unwrap_or(after);
        return format!("at {when}");
    }
    if lower == "every" && !after.is_empty() {
        return format!("every {after}");
    }
    "scheduling…".into()
}

fn is_interval_token(s: &str) -> bool {
    if s.len() < 2 {
        return false;
    }
    let (digits, suffix) = s.split_at(s.len() - 1);
    matches!(suffix, "s" | "m" | "h" | "d")
        && digits.chars().all(|c| c.is_ascii_digit())
        && digits.parse::<u64>().is_ok_and(|n| n > 0)
}

fn interval_token_to_human(token: &str) -> String {
    let (digits, suffix) = token.split_at(token.len() - 1);
    let n: u64 = digits.parse().unwrap_or(0);
    match suffix {
        "m" if n == 1 => "every 1 minute".into(),
        "m" => format!("every {n} minutes"),
        "h" if n == 1 => "every 1 hour".into(),
        "h" => format!("every {n} hours"),
        "d" if n == 1 => "every 1 day".into(),
        "d" => format!("every {n} days"),
        "s" if n <= 1 => "every 1 second".into(),
        "s" => format!("every {n} seconds"),
        _ => format!("every {token}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    fn run_schedule(args: &str) -> CommandResult {
        let models = ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        ScheduleCommand.run(&mut ctx, args)
    }

    #[test]
    fn requires_all_scheduler_tools() {
        assert_eq!(
            ScheduleCommand.required_tools(),
            &[
                "scheduler_create",
                "scheduler_list",
                "scheduler_delete"
            ]
        );
    }

    #[test]
    fn empty_args_are_usage() {
        match run_schedule("   ") {
            CommandResult::Message(msg) => assert_eq!(msg, schedule_usage_message()),
            other => panic!("expected usage Message, got {other:?}"),
        }
    }

    #[test]
    fn create_injects_standing_instruction() {
        match run_schedule("1h search rust async") {
            CommandResult::InjectSkill {
                prompt_blocks,
                scheduled_task_preview: Some(preview),
                ..
            } => {
                let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
                    panic!("expected a text prompt block");
                };
                assert_eq!(text.text, schedule_instruction("1h search rust async"));
                assert!(text.text.contains("standing: true"));
                assert!(text.text.contains("durable: true"));
                assert!(text.text.contains("search rust async"));
                assert!(text.text.contains("Schedules/"));
                assert_eq!(preview.tag, "schedule");
                assert_eq!(preview.human_schedule, "every 1 hour");
            }
            other => panic!("expected InjectSkill with preview, got {other:?}"),
        }
    }

    #[test]
    fn meeting_join_recipe_forbids_start_process() {
        match run_schedule("at 2026-08-24T09:00 meeting join https://example.com/join Standup") {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
                    panic!("expected a text prompt block");
                };
                assert!(text.text.contains("meeting_join"));
                assert!(text.text.contains("Start-Process"));
                assert!(text.text.contains("meeting_join: true"));
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }

    #[test]
    fn list_calls_scheduler_list() {
        match run_schedule("list") {
            CommandResult::InjectSkill {
                prompt_blocks,
                scheduled_task_preview: None,
                ..
            } => {
                let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
                    panic!("expected a text prompt block");
                };
                assert!(text.text.contains("scheduler_list"));
                assert!(!text.text.contains("scheduler_create with standing"));
            }
            other => panic!("expected InjectSkill without preview, got {other:?}"),
        }
    }

    #[test]
    fn cancel_calls_scheduler_delete() {
        match run_schedule("cancel abc123") {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
                    panic!("expected a text prompt block");
                };
                assert!(text.text.contains("scheduler_delete"));
                assert!(text.text.contains("abc123"));
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }

    #[test]
    fn instruction_matches_shared_helper() {
        let args = "every weekday 08:00 stat https://status.example";
        match run_schedule(args) {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let acp::ContentBlock::Text(text) = &prompt_blocks[0] else {
                    panic!("expected a text prompt block");
                };
                assert_eq!(text.text, schedule_instruction(args));
            }
            other => panic!("expected InjectSkill, got {other:?}"),
        }
    }
}
