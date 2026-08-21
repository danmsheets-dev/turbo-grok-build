use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::TaskQueueState;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentOpts {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub capability_mode: Option<String>,
    #[serde(default)]
    pub isolation_worktree: bool,
    #[serde(default)]
    pub fork_context: bool,
    #[serde(default)]
    pub resume_from: Option<String>,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub phase: Option<String>,
    /// Hard wall-clock limit for this workflow child agent (milliseconds).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Progress stall budget (milliseconds). When set, overrides platform
    /// defaults (e.g. NVIDIA 180s). Token/tool progress resets the timer.
    #[serde(default)]
    pub stall_timeout_ms: Option<u64>,
    /// When true with worktree isolation, keep the child worktree after snapshot.
    #[serde(default)]
    pub retain_worktree: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub agent_id: String,
    pub success: bool,
    pub output: serde_json::Value,
    pub cancelled: bool,
    pub tokens_used: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BudgetState {
    pub total: Option<u64>,
    pub spent: u64,
    pub reserved: u64,
    pub remaining: Option<u64>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum HostError {
    #[error("workflow agent-call quota exceeded: requested {requested}, maximum {maximum}")]
    AgentCallQuotaExceeded { requested: u64, maximum: u64 },
    #[error("workflow token budget exceeded")]
    BudgetExceeded,
    #[error("workflow cancelled")]
    Cancelled,
    #[error("unsupported in this context: {0}")]
    Unsupported(String),
    #[error("host failure: {0}")]
    Failed(String),
}

#[derive(Debug)]
pub enum WorkflowHostRequest {
    ReserveAgentCalls {
        count: u64,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    ReleaseAgentCalls {
        count: u64,
        reply: oneshot::Sender<Result<(), HostError>>,
    },
    SpawnAgent {
        opts: AgentOpts,
        reply: oneshot::Sender<Result<AgentResult, HostError>>,
    },
    Phase {
        title: String,
        replayed: bool,
    },
    Log {
        message: String,
        replayed: bool,
    },
    Telemetry {
        name: String,
        fields: serde_json::Value,
        replayed: bool,
    },
    BudgetQuery {
        reply: oneshot::Sender<Result<BudgetState, HostError>>,
    },
    RenderTemplate {
        name: String,
        vars: serde_json::Value,
        reply: oneshot::Sender<Result<String, HostError>>,
    },
    WriteScratchFile {
        name: String,
        content: String,
        reply: oneshot::Sender<Result<String, HostError>>,
    },
    ReadScratchFile {
        name: String,
        reply: oneshot::Sender<Result<String, HostError>>,
    },
    GitDiffSince {
        commit: String,
        reply: oneshot::Sender<Result<String, HostError>>,
    },
    TaskStart {
        task_id: String,
        reply: oneshot::Sender<Result<TaskQueueState, HostError>>,
    },
    TaskComplete {
        task_id: String,
        result_summary: Option<String>,
        reply: oneshot::Sender<Result<TaskQueueState, HostError>>,
    },
    TaskFail {
        task_id: String,
        result_summary: Option<String>,
        reply: oneshot::Sender<Result<TaskQueueState, HostError>>,
    },
    TaskQueue {
        reply: oneshot::Sender<Result<TaskQueueState, HostError>>,
    },
}

impl WorkflowHostRequest {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ReserveAgentCalls { .. } => "reserve_agent_calls",
            Self::ReleaseAgentCalls { .. } => "release_agent_calls",
            Self::SpawnAgent { .. } => "spawn_agent",
            Self::Phase { .. } => "phase",
            Self::Log { .. } => "log",
            Self::Telemetry { .. } => "telemetry",
            Self::BudgetQuery { .. } => "budget",
            Self::RenderTemplate { .. } => "render_template",
            Self::WriteScratchFile { .. } => "write_scratch_file",
            Self::ReadScratchFile { .. } => "read_scratch_file",
            Self::GitDiffSince { .. } => "git_diff_since",
            Self::TaskStart { .. } => "task_start",
            Self::TaskComplete { .. } => "task_complete",
            Self::TaskFail { .. } => "task_fail",
            Self::TaskQueue { .. } => "task_queue",
        }
    }
}
