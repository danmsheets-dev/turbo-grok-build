//! `/meeting` — Fathom-style notetaker (join URL → record → Grok STT → notes).

use xai_grok_tools::implementations::grok_build::{
    MEETING_COMMAND_NAME, MEETING_JOIN_TOOL_NAME, ask_instruction, join_instruction,
    knowledge_instruction, notes_instruction, split_join_args, status_instruction,
    stop_instruction, transcript_instruction, usage_message,
};

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

const REQUIRED_TOOLS: &[&str] = &[MEETING_JOIN_TOOL_NAME];

pub struct MeetingCommand;

impl SlashCommand for MeetingCommand {
    fn name(&self) -> &str {
        MEETING_COMMAND_NAME
    }

    fn description(&self) -> &str {
        "Fathom-style meeting notes (join URL, transcribe, recap, coworker Q&A)"
    }

    fn usage(&self) -> &str {
        "/meeting join <url> [name] | stop | notes | ask [q]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("join <url> [name] | stop | notes | ask [q]")
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn required_tools(&self) -> &[&str] {
        REQUIRED_TOOLS
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let args = args.trim();
        if args.is_empty() {
            return CommandResult::Message(usage_message().to_string());
        }
        let (verb, rest) = args
            .split_once(char::is_whitespace)
            .map(|(v, r)| (v, r.trim()))
            .unwrap_or((args, ""));

        // `/meeting ask` with no argument drains a question a *meeting
        // participant* wrote, so the resulting turn must be confined exactly
        // like the auto-ask path. `/meeting ask <text>` is the operator's own
        // words and stays a normal turn.
        let drains_participant_question = matches!(verb, "ask" | "q") && rest.is_empty();
        let task_id = drains_participant_question.then(|| {
            format!(
                "{}slash",
                xai_grok_tools::implementations::grok_build::meeting::MEETING_QA_TASK_PREFIX
            )
        });

        let instruction = match verb {
            "join" => {
                if rest.is_empty() {
                    return CommandResult::Message(usage_message().to_string());
                }
                let (url, title) = split_join_args(rest);
                join_instruction(url, title)
            }
            "stop" => stop_instruction(),
            "status" => status_instruction(),
            "transcript" => transcript_instruction(),
            "notes" | "recap" | "summary" => notes_instruction(),
            "knowledge" | "notes-dir" | "folder" => {
                if rest.is_empty() {
                    return CommandResult::Message(usage_message().to_string());
                }
                knowledge_instruction(rest)
            }
            "ask" | "q" => ask_instruction(if rest.is_empty() { None } else { Some(rest) }),
            _ => {
                if looks_like_url(verb) {
                    let (url, title) = split_join_args(args);
                    join_instruction(url, title)
                } else {
                    return CommandResult::Message(usage_message().to_string());
                }
            }
        };
        CommandResult::InjectSkill {
            display_text: format!("/meeting {args}"),
            prompt_blocks: vec![agent_client_protocol::ContentBlock::Text(
                agent_client_protocol::TextContent::new(instruction),
            )],
            display_as_skill: false,
            scheduled_task_preview: None,
            task_id,
        }
    }
}

fn looks_like_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_join_tool() {
        assert_eq!(MeetingCommand.required_tools(), &["meeting_join"]);
    }

    #[test]
    fn empty_args_are_usage() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        assert!(matches!(
            MeetingCommand.run(&mut ctx, ""),
            CommandResult::Message(_)
        ));
    }

    #[test]
    fn join_injects_url() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        let result = MeetingCommand.run(
            &mut ctx,
            "join https://teams.microsoft.com/l/meetup-join/abc",
        );
        match result {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let text = format!("{prompt_blocks:?}");
                assert!(text.contains("meetup-join"));
                assert!(text.contains("meeting_join"));
                assert!(text.contains("Start-Process"));
                assert!(text.contains("Do NOT use bash"));
            }
            other => panic!("expected inject, got {other:?}"),
        }
    }

    #[test]
    fn join_accepts_meeting_name() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        let result = MeetingCommand.run(
            &mut ctx,
            "join https://teams.microsoft.com/l/meetup-join/abc Weekly website standup",
        );
        match result {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let text = format!("{prompt_blocks:?}");
                assert!(text.contains("meetup-join"));
                assert!(text.contains("Weekly website standup"));
            }
            other => panic!("expected inject, got {other:?}"),
        }
    }

    #[test]
    fn stop_injects_work_only_recap() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        let result = MeetingCommand.run(&mut ctx, "stop");
        match result {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let text = format!("{prompt_blocks:?}");
                assert!(text.contains("meeting_stop"));
                assert!(text.contains("meeting_notes"));
                assert!(text.contains("Work-only") || text.contains("small talk"));
                assert!(text.contains("Meetings/"));
                assert!(text.contains("For you"));
                assert!(text.contains("Projects"));
            }
            other => panic!("expected inject, got {other:?}"),
        }
    }

    #[test]
    fn bare_url_means_join() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        let result = MeetingCommand.run(
            &mut ctx,
            "https://teams.microsoft.com/meet/2907709513066?p=abc Weekly standup",
        );
        match result {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let text = format!("{prompt_blocks:?}");
                assert!(text.contains("2907709513066"));
                assert!(text.contains("meeting_join"));
                assert!(text.contains("Weekly standup"));
            }
            other => panic!("expected inject, got {other:?}"),
        }
    }

    #[test]
    fn ask_injects_research_instruction() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = super::super::tests::make_ctx(&models);
        let result = MeetingCommand.run(&mut ctx, "ask How is the new website project going");
        match result {
            CommandResult::InjectSkill { prompt_blocks, .. } => {
                let text = format!("{prompt_blocks:?}");
                assert!(text.contains("website"));
                assert!(text.contains("meeting_ask"));
                assert!(text.contains("meeting_reply"));
                assert!(text.contains("workspace"));
                assert!(text.contains("MCP"));
                assert!(!text.contains("ONLY under the knowledge folder"));
            }
            other => panic!("expected inject, got {other:?}"),
        }
    }
}
