//! Feature Request Log (FRL) — structured product-capability requests.
//!
//! Parallel to Auto Developer Log incidents, but for **missing / desired**
//! product surface rather than bugs. Agents file via `feature_request_log`;
//! maintainers triage with `turbo features`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::{Environment, Evidence, ReporterKind, Source};

/// Schema version stamped into every feature-request document.
pub const FR_SCHEMA_VERSION: u32 = 1;

/// Product priority for a feature request (not the same as incident severity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RequestPriority {
    /// Blocks current harness / production workflow without a workable path.
    MustHave,
    /// Materially improves agent throughput; workaround exists but is costly.
    #[default]
    ShouldHave,
    /// Quality-of-life / polish.
    NiceToHave,
    /// Speculative / research.
    Exploratory,
}

impl RequestPriority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MustHave => "must_have",
            Self::ShouldHave => "should_have",
            Self::NiceToHave => "nice_to_have",
            Self::Exploratory => "exploratory",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "must_have" | "must" | "p0" | "critical" | "blocker" => Some(Self::MustHave),
            "should_have" | "should" | "p1" | "high" => Some(Self::ShouldHave),
            "nice_to_have" | "nice" | "p2" | "medium" => Some(Self::NiceToHave),
            "exploratory" | "explore" | "p3" | "low" | "wishlist" => Some(Self::Exploratory),
            _ => None,
        }
    }

    /// Lower is more urgent (for sorting).
    pub fn rank(self) -> u8 {
        match self {
            Self::MustHave => 0,
            Self::ShouldHave => 1,
            Self::NiceToHave => 2,
            Self::Exploratory => 3,
        }
    }
}

impl std::fmt::Display for RequestPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable taxonomy for feature-request fingerprinting and product triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RequestClass {
    /// Missing or incomplete tool on the agent surface.
    ToolSurface,
    /// Workflow / orchestration capability.
    Workflow,
    /// Subagent spawn, resume, isolation, land/diff, allowlists.
    Subagent,
    /// TUI / Game Mode / keyboard / UX.
    UiUx,
    /// Provider / model routing / catalog.
    ProviderModel,
    /// MCP server integration or connect reliability (product side).
    McpIntegration,
    /// Docs, boot card, operator CLI gaps.
    Documentation,
    /// Performance / scale / concurrency.
    Performance,
    /// Public/API surface, config, flags.
    ApiSurface,
    /// Scheduler / keep-N / automation loops.
    Scheduler,
    /// Memory, skills, plugins.
    Extensibility,
    #[default]
    Other,
}

impl RequestClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolSurface => "tool_surface",
            Self::Workflow => "workflow",
            Self::Subagent => "subagent",
            Self::UiUx => "ui_ux",
            Self::ProviderModel => "provider_model",
            Self::McpIntegration => "mcp_integration",
            Self::Documentation => "documentation",
            Self::Performance => "performance",
            Self::ApiSurface => "api_surface",
            Self::Scheduler => "scheduler",
            Self::Extensibility => "extensibility",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tool_surface" | "tool" | "tools" => Some(Self::ToolSurface),
            "workflow" | "workflows" => Some(Self::Workflow),
            "subagent" | "subagents" | "isolation" | "land" => Some(Self::Subagent),
            "ui_ux" | "ui" | "ux" | "tui" | "game_mode" => Some(Self::UiUx),
            "provider_model" | "provider" | "model" | "models" => Some(Self::ProviderModel),
            "mcp_integration" | "mcp" => Some(Self::McpIntegration),
            "documentation" | "docs" => Some(Self::Documentation),
            "performance" | "perf" => Some(Self::Performance),
            "api_surface" | "api" | "config" => Some(Self::ApiSurface),
            "scheduler" | "schedule" => Some(Self::Scheduler),
            "extensibility" | "plugins" | "skills" | "memory" => Some(Self::Extensibility),
            "other" | "unknown" => Some(Self::Other),
            _ => None,
        }
    }

    pub fn default_priority(self) -> RequestPriority {
        match self {
            Self::ToolSurface | Self::Subagent | Self::Workflow => RequestPriority::ShouldHave,
            Self::Performance | Self::McpIntegration | Self::ProviderModel => {
                RequestPriority::ShouldHave
            }
            Self::UiUx | Self::ApiSurface | Self::Scheduler | Self::Extensibility => {
                RequestPriority::NiceToHave
            }
            Self::Documentation | Self::Other => RequestPriority::NiceToHave,
        }
    }
}

impl std::fmt::Display for RequestClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle status (shared semantics with incidents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    #[default]
    Open,
    Acknowledged,
    Planned,
    Shipped,
    Declined,
}

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Planned => "planned",
            Self::Shipped => "shipped",
            Self::Declined => "declined",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "acknowledged" | "ack" => Some(Self::Acknowledged),
            "planned" | "accepted" | "roadmap" => Some(Self::Planned),
            "shipped" | "done" | "resolved" | "closed" => Some(Self::Shipped),
            "declined" | "wontdo" | "wontfix" | "rejected" => Some(Self::Declined),
            _ => None,
        }
    }

    pub fn is_openish(self) -> bool {
        matches!(self, Self::Open | Self::Acknowledged | Self::Planned)
    }
}

impl std::fmt::Display for RequestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Canonical feature request stored under `feature-request-log/requests/`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeatureRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub fingerprint: String,
    pub title: String,
    pub summary: String,
    pub request_class: RequestClass,
    pub priority: RequestPriority,
    pub status: RequestStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component: Vec<String>,
    pub occurrence_count: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// Why agents need this — concrete harness / user scenario.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_case: Option<String>,
    /// What agents do today without the feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_workaround: Option<String>,
    /// Desired product behavior or API shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_behavior: Option<String>,
    /// Optional acceptance criteria (bullet strings).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub evidence: Evidence,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Proving git commit when status is shipped (no secrets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship_sha: Option<String>,
    /// Optional short ship/decline note (no secrets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship_note: Option<String>,
}

/// Input for creating or merging a feature request (agent tool / CLI).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureRequestReport {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub request_class: RequestClass,
    #[serde(default)]
    pub priority: Option<RequestPriority>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_case: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_workaround: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_behavior: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub evidence: Evidence,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Result of a feature-request report (new or merged).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureRequestResult {
    pub request_id: String,
    pub fingerprint: String,
    pub is_new: bool,
    pub occurrence_count: u32,
    pub path: String,
    pub priority: RequestPriority,
    pub request_class: RequestClass,
    pub title: String,
}

/// Append-only event line in `events.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureRequestEvent {
    pub ts: DateTime<Utc>,
    pub request_id: String,
    pub fingerprint: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Default agent source stamp for the tool.
pub fn agent_source(tool: &str, model: Option<String>) -> Source {
    Source {
        reporter: ReporterKind::Agent,
        auto: false,
        reporter_model: model,
        tool: Some(tool.into()),
        detector: None,
    }
}
