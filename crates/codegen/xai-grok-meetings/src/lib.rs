//! Fathom-style meeting notetaker primitives used by Turbo tools and `/meeting`.
//!
//! This crate does **not** capture audio or talk to Zoom/Teams SDKs. Capture +
//! Grok STT live in `xai-grok-tools` (`meeting_*` tools). Join-URL parsing,
//! on-disk transcript/notes, and slash-command prompts live here so the pager
//! can inject `/meeting` without pulling the audio stack.

pub mod knowledge;
pub mod slash;
pub mod store;
pub mod summary;
pub mod trigger;
pub mod url;

pub use knowledge::{briefing, read_knowledge_dir, write_knowledge_dir};
pub use slash::{
    MEETING_ASK_TOOL_NAME, MEETING_COMMAND_NAME, MEETING_JOIN_TOOL_NAME,
    MEETING_KNOWLEDGE_TOOL_NAME, MEETING_NOTES_TOOL_NAME, MEETING_REPLY_TOOL_NAME,
    MEETING_STATUS_TOOL_NAME, MEETING_STOP_TOOL_NAME, MEETING_TRANSCRIPT_TOOL_NAME,
    ask_instruction, join_instruction, knowledge_instruction, notes_instruction,
    reply_instruction, split_join_args, status_instruction, stop_instruction,
    transcript_instruction, usage_message,
};
pub use store::{
    CaptureSource, MeetingMeta, MeetingStatus, MeetingStore, TranscriptSegment, clear_current,
    is_safe_meeting_id, meeting_dir, new_meeting_id, read_current_id, write_current,
};
pub use summary::{
    WORKSPACE_MEETINGS_DIR, compose_summary_markdown, default_meeting_title,
    extract_title_from_markdown, local_date_stamp, recap_dest_is_safe, sanitize_meeting_name,
    summary_filename, unique_summary_path, workspace_meetings_dir, write_workspace_summary,
};
pub use trigger::extract_turbo_question;
pub use url::{MeetingKind, MeetingPlatform, MeetingUrl, ParseError, parse as parse_meeting_url};
