//! Canonical Auto Developer Log incident schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Schema version stamped into every incident document.
pub const SCHEMA_VERSION: u32 = 1;

/// Product-facing issue severity (P0 highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    P0,
    P1,
    #[default]
    P2,
    P3,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "p0" | "critical" => Some(Self::P0),
            "p1" | "high" => Some(Self::P1),
            "p2" | "medium" => Some(Self::P2),
            "p3" | "low" => Some(Self::P3),
            _ => None,
        }
    }

    /// Lower is more severe (for sorting).
    pub fn rank(self) -> u8 {
        match self {
            Self::P0 => 0,
            Self::P1 => 1,
            Self::P2 => 2,
            Self::P3 => 3,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// High-level incident kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IncidentKind {
    Bug,
    ProductFriction,
    FeatureGap,
    ProviderCompat,
    DocsGap,
    Perf,
    #[default]
    Unknown,
}

impl IncidentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::ProductFriction => "product_friction",
            Self::FeatureGap => "feature_gap",
            Self::ProviderCompat => "provider_compat",
            Self::DocsGap => "docs_gap",
            Self::Perf => "perf",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bug" => Some(Self::Bug),
            "product_friction" | "friction" => Some(Self::ProductFriction),
            "feature_gap" | "feature" => Some(Self::FeatureGap),
            "provider_compat" | "provider" => Some(Self::ProviderCompat),
            "docs_gap" | "docs" => Some(Self::DocsGap),
            "perf" | "performance" => Some(Self::Perf),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

impl std::fmt::Display for IncidentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable taxonomy for fingerprinting and triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    WorktreeTombstone,
    IsolationFallback,
    SubagentStall,
    ProtocolDeser,
    Provider400,
    ProviderAuth,
    ToolSchema,
    LandConflict,
    McpConnect,
    CatalogStale,
    DocsGap,
    FeatureGap,
    PerfRegression,
    WorkLostRisk,
    #[default]
    Unknown,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorktreeTombstone => "worktree_tombstone",
            Self::IsolationFallback => "isolation_fallback",
            Self::SubagentStall => "subagent_stall",
            Self::ProtocolDeser => "protocol_deser",
            Self::Provider400 => "provider_400",
            Self::ProviderAuth => "provider_auth",
            Self::ToolSchema => "tool_schema",
            Self::LandConflict => "land_conflict",
            Self::McpConnect => "mcp_connect",
            Self::CatalogStale => "catalog_stale",
            Self::DocsGap => "docs_gap",
            Self::FeatureGap => "feature_gap",
            Self::PerfRegression => "perf_regression",
            Self::WorkLostRisk => "work_lost_risk",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "worktree_tombstone" => Some(Self::WorktreeTombstone),
            "isolation_fallback" => Some(Self::IsolationFallback),
            "subagent_stall" => Some(Self::SubagentStall),
            "protocol_deser" => Some(Self::ProtocolDeser),
            "provider_400" => Some(Self::Provider400),
            "provider_auth" => Some(Self::ProviderAuth),
            "tool_schema" => Some(Self::ToolSchema),
            "land_conflict" => Some(Self::LandConflict),
            "mcp_connect" => Some(Self::McpConnect),
            "catalog_stale" => Some(Self::CatalogStale),
            "docs_gap" => Some(Self::DocsGap),
            "feature_gap" => Some(Self::FeatureGap),
            "perf_regression" => Some(Self::PerfRegression),
            "work_lost_risk" => Some(Self::WorkLostRisk),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }

    pub fn default_severity(self) -> Severity {
        match self {
            Self::WorktreeTombstone | Self::WorkLostRisk | Self::ProtocolDeser => Severity::P0,
            Self::IsolationFallback
            | Self::SubagentStall
            | Self::Provider400
            | Self::ProviderAuth
            | Self::LandConflict => Severity::P1,
            Self::ToolSchema
            | Self::McpConnect
            | Self::CatalogStale
            | Self::FeatureGap
            | Self::PerfRegression => Severity::P2,
            Self::DocsGap | Self::Unknown => Severity::P3,
        }
    }

    pub fn default_kind(self) -> IncidentKind {
        match self {
            Self::WorktreeTombstone
            | Self::IsolationFallback
            | Self::SubagentStall
            | Self::ProtocolDeser
            | Self::ToolSchema
            | Self::LandConflict
            | Self::WorkLostRisk => IncidentKind::Bug,
            Self::Provider400 | Self::ProviderAuth | Self::CatalogStale | Self::McpConnect => {
                IncidentKind::ProviderCompat
            }
            Self::FeatureGap => IncidentKind::FeatureGap,
            Self::DocsGap => IncidentKind::DocsGap,
            Self::PerfRegression => IncidentKind::Perf,
            Self::Unknown => IncidentKind::Unknown,
        }
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Incident lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IncidentStatus {
    #[default]
    Open,
    Acknowledged,
    Resolved,
    Wontdo,
}

impl IncidentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Acknowledged => "acknowledged",
            Self::Resolved => "resolved",
            Self::Wontdo => "wontdo",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "acknowledged" | "ack" => Some(Self::Acknowledged),
            "resolved" | "fixed" | "closed" => Some(Self::Resolved),
            "wontdo" | "wontfix" | "wont_fix" => Some(Self::Wontdo),
            _ => None,
        }
    }
}

impl std::fmt::Display for IncidentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who filed the incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReporterKind {
    #[default]
    Agent,
    Runtime,
    Human,
    Cli,
}

impl ReporterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Runtime => "runtime",
            Self::Human => "human",
            Self::Cli => "cli",
        }
    }
}

/// Confidence that the repro steps reproduce the issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReproConfidence {
    High,
    #[default]
    Medium,
    Low,
    Unknown,
}

/// Runtime environment snapshot (redacted / low-PII).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Environment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hyper_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_hash: Option<String>,
}

/// Structured reproduction notes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Repro {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(default)]
    pub confidence: ReproConfidence,
}

/// Pointers to durable evidence (paths are under ~/.grok, not secrets).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Evidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_events: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Provenance of the report.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Source {
    pub reporter: ReporterKind,
    #[serde(default)]
    pub auto: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporter_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detector: Option<String>,
}

/// Canonical product incident stored under `developer-log/incidents/`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Incident {
    pub schema_version: u32,
    pub incident_id: String,
    pub fingerprint: String,
    pub kind: IncidentKind,
    pub title: String,
    pub summary: String,
    pub severity: Severity,
    pub status: IncidentStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component: Vec<String>,
    pub error_class: ErrorClass,
    pub occurrence_count: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub repro: Repro,
    #[serde(default)]
    pub evidence: Evidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Input for creating or merging an incident (agent tool / detectors).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportRequest {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub kind: Option<IncidentKind>,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub error_class: ErrorClass,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub component: Vec<String>,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub repro: Repro,
    #[serde(default)]
    pub evidence: Evidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_fix: Option<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Optional override; when empty, computed from error_class + components.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Result of a report (new or merged).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportResult {
    pub incident_id: String,
    pub fingerprint: String,
    pub is_new: bool,
    pub occurrence_count: u32,
    pub path: String,
    pub severity: Severity,
    pub error_class: ErrorClass,
    pub title: String,
}

/// Append-only raw event line in `events.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub ts: DateTime<Utc>,
    pub incident_id: String,
    pub fingerprint: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
