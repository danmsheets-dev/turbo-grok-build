//! New-architecture tool implementations (NewTool trait).
//!
//! Each sub-module here contains a tool that implements `NewTool` instead
//! of the old `Tool` trait. During migration, old implementations live in
//! `implementations/<tool>/` and new implementations live in
//! `implementations/grok_build/<tool>/`.
//!
//! The [`register_all()`] function is the single entry-point for wiring up
//! the standard toolset. It inserts shared resources (`Terminal`,
//! `AvailableSkills`, `BashParams`) and registers every built-in tool.
pub mod ask_user_question;
pub mod bash;
pub mod browser;
pub mod developer_log;
pub mod feature_request_log;
#[path = "deploy_app_stub.rs"]
pub mod deploy_app;
pub mod enter_plan_mode;
pub mod exit_plan_mode;
pub mod grep;
pub mod image_edit;
pub mod image_gen;
pub mod kill_task;
pub mod list_dir;
pub mod lsp;
pub mod monitor;
pub mod read_file;
pub mod resolve_path;
pub mod scheduler;
pub mod search_replace;
pub mod workspace_tree;
pub mod spawn_many;
pub(crate) mod storage;
pub mod subagent_worktree;
pub mod task;
pub mod task_output;
pub mod todo;
pub mod update_goal;
pub mod video_gen;
pub mod web_fetch;
pub mod web_search;
pub mod workflow;
pub use ask_user_question::AskUserQuestionTool;
pub use bash::BashTool;
pub use browser::{
    BROWSER_CLICK_TOOL_NAME, BROWSER_EVAL_TOOL_NAME, BROWSER_FILL_TOOL_NAME,
    BROWSER_NAVIGATE_TOOL_NAME, BROWSER_SCREENSHOT_TOOL_NAME, BROWSER_SNAPSHOT_TOOL_NAME,
    BROWSER_TABS_TOOL_NAME, BrowserClickTool, BrowserEvalTool, BrowserFillTool, BrowserHandle,
    BrowserNavigateTool, BrowserScreenshotTool, BrowserSnapshotTool, BrowserTabsTool,
};
pub use deploy_app::{AppBuilderDeployerConfig, DEPLOY_APP_TOOL_NAME};
pub use developer_log::{DEVELOPER_LOG_TOOL_NAME, DeveloperLogTool};
pub use feature_request_log::{FEATURE_REQUEST_LOG_TOOL_NAME, FeatureRequestLogTool};
pub use enter_plan_mode::EnterPlanModeTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use grep::GrepTool;
pub use image_edit::{IMAGE_EDIT_TOOL_NAME, ImageEditTool};
pub use image_gen::{
    IMAGE_GEN_TOOL_NAME, IMAGINE_COMMAND_NAME, ImageGenTool, imagine_instruction,
    imagine_usage_message,
};
pub use kill_task::{KillTaskTool, KillTerminalCommandTool};
pub use list_dir::ListDirTool;
pub use lsp::LspTool;
pub use monitor::tool::MonitorTool;
pub use read_file::ReadFileTool;
pub use resolve_path::{RESOLVE_PATH_TOOL_NAME, ResolvePathTool};
pub use workspace_tree::{WORKSPACE_TREE_TOOL_NAME, WorkspaceTreeTool};
pub use scheduler::create::{
    LoopFireMode, SCHEDULER_CREATE_TOOL_NAME, SchedulerCreateTool, loop_schedule_instruction,
    loop_usage_message,
};
pub use scheduler::delete::{SCHEDULER_DELETE_TOOL_NAME, SchedulerDeleteTool};
pub use scheduler::list::SchedulerListTool;
pub use search_replace::SearchReplaceTool;
pub use spawn_many::{SPAWN_MANY_TOOL_NAME, SpawnManyTool};
pub use subagent_worktree::diff::{DIFF_SUBAGENT_TOOL_NAME, DiffSubagentTool};
pub use subagent_worktree::discard::{DISCARD_SUBAGENT_TOOL_NAME, DiscardSubagentTool};
pub use subagent_worktree::land::{LAND_SUBAGENT_TOOL_NAME, LandSubagentTool};
pub use task::TaskTool;
pub use task_output::{GetTerminalCommandOutputTool, TaskOutputTool, WaitTasksTool};
pub use todo::TodoWriteTool;
pub use update_goal::{UPDATE_GOAL_TOOL_NAME, UpdateGoalTool};
pub use video_gen::{
    IMAGE_TO_VIDEO_TOOL_NAME, IMAGINE_VIDEO_COMMAND_NAME, ImageToVideoTool,
    REFERENCE_TO_VIDEO_TOOL_NAME, ReferenceToVideoTool, imagine_video_instruction,
    imagine_video_usage_message,
};
pub use web_fetch::{WebFetchClient, WebFetchConfig, WebFetchParams, WebFetchTool};
pub use web_search::WebSearchTool;
pub use workflow::{WORKFLOW_TOOL_NAME, WorkflowTool};
