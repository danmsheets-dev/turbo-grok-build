//! Headless single-turn mode (`grok -p "prompt"`).
//!
//! Runs the agent in-process via
//! `spawn_grok_shell`, sends the ACP lifecycle (init → auth → session → prompt),
//! streams text to stdout, and exits cleanly via `CancellationToken`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::ValueEnum;
use tokio_util::sync::CancellationToken;

use agent_client_protocol as acp;
use xai_acp_lib::{AcpAgentTx, AcpClientMessageBox, AcpClientRx, acp_send};
use xai_grok_shell::agent::auth_method::AuthMethodKind;
use xai_grok_shell::agent::config::Config as AgentConfig;
use xai_grok_shell::extensions::task::{CancelSubagentRequest, KillTaskRequest};
use xai_grok_shell::sampling::error::{
    RATE_LIMITED_ERROR_CODE, error_detail_from_data, format_rate_limited_user_message,
};
use xai_grok_shell::sampling::types::{
    REASONING_EFFORT_META_KEY, parse_canonical_effort_token, reasoning_effort_meta_value,
};
use xai_grok_shell::util::config as cli_config;

use crate::acp::model_state::{EffortTokenError, ModelState};
use crate::acp::spawn::{AgentShutdownGuard, spawn_grok_shell};
use crate::client_identity::{HEADLESS_CLIENT_TYPE, PAGER_CLIENT_VERSION};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Plain,
    Json,
    #[value(name = "streaming-json")]
    StreamingJson,
}

pub fn parse_json_schema(input: &str) -> anyhow::Result<serde_json::Value> {
    let schema: serde_json::Value = serde_json::from_str(input)
        .map_err(|e| anyhow::anyhow!("--json-schema: invalid JSON: {e}"))?;
    if !schema.is_object() {
        anyhow::bail!("--json-schema: must be a JSON object describing a JSON Schema");
    }
    Ok(schema)
}

#[derive(Debug, Clone)]
pub enum HeadlessPrompt {
    Text(String),
    Blocks(Vec<acp::ContentBlock>),
}

impl HeadlessPrompt {
    /// Build from mutually-exclusive CLI prompt args. `None` = interactive mode.
    pub fn from_args(
        single: Option<&str>,
        prompt_json: Option<&str>,
        prompt_file: Option<&Path>,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(text) = single {
            Self::from_text(text)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("--single: {e}"))
        } else if let Some(json_str) = prompt_json {
            Self::from_json(json_str)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("--prompt-json: {e}"))
        } else if let Some(path) = prompt_file {
            Self::from_file(path).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `.json` files are parsed as content blocks, everything else as text.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {e}", path.display()))?;

        let context = |e| anyhow::anyhow!("'{}': {e}", path.display());
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            Self::from_json(&content).map_err(context)
        } else {
            Self::from_text(&content).map_err(context)
        }
    }

    fn from_text(text: &str) -> anyhow::Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            anyhow::bail!("prompt is empty");
        }
        Ok(Self::Text(trimmed.to_string()))
    }

    fn from_json(json_str: &str) -> anyhow::Result<Self> {
        let blocks = parse_prompt_json(json_str)?;
        Ok(Self::Blocks(blocks))
    }

    pub fn into_content_blocks(self) -> Vec<acp::ContentBlock> {
        match self {
            Self::Text(text) => vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
            Self::Blocks(blocks) => blocks,
        }
    }
}

/// Parse a JSON string into ACP content blocks.
///
/// Accepts an array (`[...]`) or typed wrapper (`{"type":"acp","content":[...]}`).
fn parse_prompt_json(json_str: &str) -> anyhow::Result<Vec<acp::ContentBlock>> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| anyhow::anyhow!("Invalid JSON: {e}"))?;

    let blocks: Vec<acp::ContentBlock> = match value {
        serde_json::Value::Array(_) => serde_json::from_value(value)
            .map_err(|e| anyhow::anyhow!("Invalid ACP content blocks: {e}"))?,

        serde_json::Value::Object(ref map) => {
            let format_type = map.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
                anyhow::anyhow!(
                    "JSON object must have a \"type\" field \
                         (e.g., {{\"type\": \"acp\", \"content\": [...]}})"
                )
            })?;
            let content = map
                .get("content")
                .ok_or_else(|| anyhow::anyhow!("JSON object must have a \"content\" field"))?;

            match format_type {
                "acp" => serde_json::from_value(content.clone()).map_err(|e| {
                    anyhow::anyhow!("Invalid ACP content blocks in \"content\": {e}")
                })?,
                other => anyhow::bail!(
                    "Unsupported prompt format type: \"{other}\". Supported types: \"acp\""
                ),
            }
        }

        _ => {
            anyhow::bail!("Expected JSON array or {{\"type\": \"...\", \"content\": [...]}} object")
        }
    };

    if blocks.is_empty() {
        anyhow::bail!("content blocks array is empty");
    }
    Ok(blocks)
}

#[derive(Debug, Clone)]
pub struct HeadlessOptions {
    pub session_id: Option<String>,
    pub resume: Option<String>,
    /// The composition root pinned (or definitively missed) `resume` before
    /// the OS sandbox; materialization must not re-run local title selection.
    pub resume_title_pinned: bool,
    pub cwd: Option<PathBuf>,
    pub yolo: bool,
    pub trust: bool,
    pub output_format: OutputFormat,
    pub json_schema: Option<serde_json::Value>,
    pub model: Option<String>,
    pub rules: Option<String>,
    pub system_prompt_override: Option<String>,
    pub continue_last_session: bool,
    /// Fork on resume/continue (`--fork-session`).
    pub fork_session: bool,
    pub worktree: Option<String>,
    pub restore_code: bool,
    pub agent: Option<String>,
    pub agents_json: Option<String>,
    pub cli_tools: Option<String>,
    pub cli_disallowed_tools: Option<String>,
    pub disable_web_search: bool,
    pub allow_rules: Vec<String>,
    pub deny_rules: Vec<String>,
    pub max_turns: Option<u32>,
    pub permission_mode_flag: Option<String>,
    /// Effort token (`--reasoning-effort` / `--effort`); resolved like `/effort` after models load.
    pub reasoning_effort: Option<String>,
    /// Wait for background tasks (bash, subagent, monitor) to report
    /// `task_completed` before exiting. Default: true. Does not wait for
    /// server-side auto-wake (that runs inside the shell). Use
    /// `--no-wait-for-background` for fast smoke tests; long-lived monitors
    /// are capped by `background_wait_timeout`.
    pub wait_for_background: bool,
    /// Max time to wait for background quiescence after the first turn ends.
    pub background_wait_timeout: Duration,
    /// When true, a run that changed no files exits non-zero with
    /// `stopReason: "NoChanges"`.
    pub require_changes: bool,
    /// Opt out of the non-negotiable headless no-questions system-prompt
    /// clause and of the question-ending auto-continue recovery (HYPER-1).
    /// Rare: only for harnesses that genuinely want the model to ask.
    pub allow_interactive_questions: bool,
}

// ── CLI flag helpers ─────────────────────────────────────────────────────

/// Parse a comma-separated list into a vec, or None if empty.
fn parse_comma_list(s: Option<&str>) -> Option<Vec<String>> {
    s.and_then(|s| {
        let v: Vec<String> = s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if v.is_empty() { None } else { Some(v) }
    })
}

pub fn parse_permission_rules_strict(
    allow: &[String],
    deny: &[String],
) -> anyhow::Result<Vec<xai_grok_workspace::permission::types::PermissionRule>> {
    let (rules, errors) = parse_permission_rules_inner(allow, deny);
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .into_iter()
            .map(|(flag, rule, err)| format!("{flag} \"{rule}\": {err}"))
            .collect();
        anyhow::bail!("{}", msgs.join("; "));
    }
    Ok(rules)
}

pub fn parse_permission_rules_lenient(
    allow: &[String],
    deny: &[String],
) -> Vec<xai_grok_workspace::permission::types::PermissionRule> {
    let (rules, errors) = parse_permission_rules_inner(allow, deny);
    for (flag, rule, err) in errors {
        eprintln!("warning: {flag} \"{rule}\": {err}, skipping");
    }
    rules
}

// Deny rules are processed before allow rules so that after prepending
// to the config's rule list the order is [cli_deny, cli_allow, config_rules...].
// The policy evaluator is order-independent (deny > ask > allow), so this
// ordering is cosmetic for logging/provenance, not functional.
pub(crate) fn parse_permission_rules_inner(
    allow: &[String],
    deny: &[String],
) -> (
    Vec<xai_grok_workspace::permission::types::PermissionRule>,
    Vec<(&'static str, String, String)>,
) {
    use xai_grok_workspace::permission::rules::parse_permission_rule;
    use xai_grok_workspace::permission::types::RuleAction;

    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for rule_str in deny {
        match parse_permission_rule(rule_str, RuleAction::Deny) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(("--deny", rule_str.clone(), e.to_string())),
        }
    }
    for rule_str in allow {
        match parse_permission_rule(rule_str, RuleAction::Allow) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(("--allow", rule_str.clone(), e.to_string())),
        }
    }
    (rules, errors)
}

pub(crate) enum ResolvedAgent {
    FilePath(PathBuf),
    Name(String),
}

pub(crate) fn resolve_agent_arg(agent: &str) -> ResolvedAgent {
    let path = std::path::Path::new(agent);
    if path.exists() && path.is_file() {
        ResolvedAgent::FilePath(dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
    } else {
        ResolvedAgent::Name(agent.to_string())
    }
}

fn parse_cli_agents(
    json: &str,
) -> anyhow::Result<Vec<xai_grok_shell::agent::config::AgentDefinition>> {
    let map: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(json).map_err(|e| anyhow::anyhow!("--agents: invalid JSON: {e}"))?;
    let mut agents = Vec::with_capacity(map.len());
    for (name, mut value) in map {
        if let serde_json::Value::Object(ref mut obj) = value {
            // Accept "prompt" as an alias for "promptBody".
            if !obj.contains_key("promptBody")
                && let Some(prompt) = obj.remove("prompt")
            {
                obj.insert("promptBody".to_string(), prompt);
            }
            obj.entry("name".to_string())
                .or_insert_with(|| serde_json::Value::String(name.clone()));
            obj.entry("description".to_string())
                .or_insert_with(|| serde_json::Value::String(name.clone()));
        }
        let mut def = xai_grok_shell::agent::config::AgentDefinition::from_json(&value)
            .map_err(|e| anyhow::anyhow!("--agents: failed to parse '{name}': {e}"))?;
        def.name = name;
        agents.push(def);
    }
    Ok(agents)
}

fn apply_agent_flag(agent: &Option<String>, config: &mut xai_grok_shell::agent::config::Config) {
    if let Some(agent) = agent {
        match resolve_agent_arg(agent) {
            ResolvedAgent::FilePath(path) => config.agent_profile_path = Some(path),
            ResolvedAgent::Name(name) => config.agent.name = Some(name),
        }
    }
}

// ── Emitter ──────────────────────────────────────────────────────────────

/// Cap for `filesChanged.paths` on the `end` event (harness convention).
const FILES_CHANGED_MAX_PATHS: usize = 200;
/// Soft cap on total path-list bytes before marking `truncated`.
const FILES_CHANGED_MAX_BYTES: usize = 32 * 1024;

struct HeadlessEmitter {
    format: OutputFormat,
    parse_structured_output: bool,
    text_buffer: String,
    thought_buffer: String,
    /// Full assistant text for the *current* prompt turn. Always accumulated
    /// (even when streaming) so the headless question detector can inspect
    /// the final message without depending on `--json-schema` buffering.
    turn_assistant_text: String,
    /// Tool calls observed on the *current* prompt turn (any kind). Used to
    /// detect the HYPER-1 shape: EndTurn + zero tools + question text.
    turn_tool_calls: u32,
    /// Agent's schema-validated output (both backends), read from the
    /// prompt-response `_meta`.
    structured_output: Option<Result<serde_json::Value, String>>,
    /// From `_meta.usage`, projected onto the final result when present.
    usage: Option<serde_json::Value>,
    /// Absolute/display paths the agent edited via tools this run (Edit
    /// tool-call locations). Used for `filesChanged` on the `end` event —
    /// build products are not included because only Edit-kind tool calls
    /// contribute.
    files_changed: std::collections::BTreeSet<String>,
}

impl HeadlessEmitter {
    fn new(format: OutputFormat, parse_structured_output: bool) -> Self {
        Self {
            format,
            parse_structured_output,
            text_buffer: String::new(),
            thought_buffer: String::new(),
            turn_assistant_text: String::new(),
            turn_tool_calls: 0,
            structured_output: None,
            usage: None,
            files_changed: std::collections::BTreeSet::new(),
        }
    }

    /// Clear per-turn counters before a follow-up auto-continue prompt so
    /// the second turn is judged on its own tool activity, not the first.
    fn reset_turn_tracking(&mut self) {
        self.turn_assistant_text.clear();
        self.turn_tool_calls = 0;
    }

    fn note_tool_call(&mut self) {
        self.turn_tool_calls = self.turn_tool_calls.saturating_add(1);
    }

    /// Record paths from a completed Edit tool call (agent tool edits only).
    fn note_edit_locations(&mut self, locations: &[acp::ToolCallLocation]) {
        for loc in locations {
            let path = loc.path.display().to_string();
            if !path.is_empty() {
                self.files_changed.insert(path);
            }
        }
    }

    /// Build the capped `filesChanged` object for terminal events.
    fn files_changed_json(&self) -> serde_json::Value {
        let mut paths: Vec<String> = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;
        for p in &self.files_changed {
            if paths.len() >= FILES_CHANGED_MAX_PATHS {
                truncated = true;
                break;
            }
            let add = p.len() + 1;
            if bytes + add > FILES_CHANGED_MAX_BYTES {
                truncated = true;
                break;
            }
            bytes += add;
            paths.push(p.clone());
        }
        // Count is the full unique set; paths may be a capped prefix.
        let count = self.files_changed.len();
        if count > paths.len() {
            truncated = true;
        }
        serde_json::json!({
            "count": count,
            "paths": paths,
            "truncated": truncated,
        })
    }

    /// Read structured output from the prompt-response `_meta` — the same
    /// object headless awaits for `sessionId`/`requestId`, so delivery is
    /// deterministic (no side-channel race). `structuredOutput` carries the
    /// value, `structuredOutputError` the failure; absence leaves `None`.
    fn set_structured_output_from_meta(&mut self, meta: Option<&acp::Meta>) {
        if !self.parse_structured_output {
            return;
        }
        let Some(meta) = meta else { return };
        if let Some(err) = meta.get("structuredOutputError").and_then(|v| v.as_str()) {
            self.structured_output = Some(Err(err.to_string()));
        } else if let Some(value) = meta.get("structuredOutput") {
            self.structured_output = Some(Ok(value.clone()));
        }
    }

    fn set_usage_from_meta(&mut self, meta: Option<&acp::Meta>) {
        let Some(meta) = meta else { return };
        self.usage = meta.get("usage").cloned();
    }

    fn on_text_chunk(&mut self, text: &str) {
        // Always accumulate for the headless question detector (HYPER-1).
        self.turn_assistant_text.push_str(text);
        match self.format {
            OutputFormat::Plain => {
                use std::io::Write as _;
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            OutputFormat::StreamingJson => {
                println!("{}", serde_json::json!({"type":"text","data": text}));
                if self.parse_structured_output {
                    self.text_buffer.push_str(text);
                }
            }
            OutputFormat::Json => {
                self.text_buffer.push_str(text);
            }
        }
    }

    /// Stream event when headless auto-continues after a question-only turn.
    /// Harnesses count these to detect HYPER-1 recovery (and to bound loops).
    fn on_auto_continue(&self, reason: &str, attempt: u32) {
        match self.format {
            OutputFormat::StreamingJson => {
                println!(
                    "{}",
                    serde_json::json!({
                        "type": "auto_continue",
                        "reason": reason,
                        "attempt": attempt,
                    })
                );
            }
            OutputFormat::Plain => {
                eprintln!("headless: auto-continuing ({reason}, attempt {attempt})");
            }
            OutputFormat::Json => {}
        }
    }

    fn on_thought_chunk(&mut self, text: &str) {
        match self.format {
            OutputFormat::Plain => { /* no-op */ }
            OutputFormat::StreamingJson => {
                println!("{}", serde_json::json!({"type":"thought","data": text}));
            }
            OutputFormat::Json => {
                self.thought_buffer.push_str(text);
            }
        }
    }

    /// Stream a headless permission denial so harnesses see the refusal even
    /// when the model-facing tool body is also filled (H1). Plain format is
    /// silent — the model text path already carries the reason.
    fn on_tool_denied(&mut self, tool: &str, reason: &str) {
        match self.format {
            OutputFormat::StreamingJson => {
                println!(
                    "{}",
                    serde_json::json!({
                        "type": "tool_denied",
                        "tool": tool,
                        "reason": reason,
                    })
                );
            }
            OutputFormat::Json | OutputFormat::Plain => {}
        }
    }

    fn attach_structured_output(&self, target: &mut serde_json::Value) {
        if !self.parse_structured_output {
            return;
        }
        // The agent is the only source of validated output; never parse the raw
        // text buffer (that would bypass validation). Absent `_meta` output
        // (max-turns/cancel) → a clean error, never unvalidated JSON.
        let result = self
            .structured_output
            .clone()
            .unwrap_or_else(|| Err("model did not produce structured output".to_string()));
        match result {
            Ok(value) => {
                target["structuredOutput"] = value;
            }
            Err(e) => {
                target["structuredOutput"] = serde_json::Value::Null;
                target["structuredOutputError"] = e.into();
            }
        }
    }

    /// Final object for `--output-format json`, including spend fields when present.
    fn build_json_result(
        &self,
        stop_reason: &str,
        session_id: &str,
        request_id: &str,
    ) -> serde_json::Value {
        let mut result = serde_json::json!({
            "text": self.text_buffer,
            "stopReason": stop_reason,
            "sessionId": session_id,
            "requestId": request_id
        });
        if !self.thought_buffer.is_empty() {
            result["thought"] = serde_json::Value::String(self.thought_buffer.clone());
        }
        if let Some(usage) = &self.usage {
            attach_result_usage(&mut result, usage);
        }
        self.attach_structured_output(&mut result);
        result
    }

    /// First NDJSON line of any `streaming-json` run (H6). Harnesses pin on
    /// `schemaVersion` and the confine/permission snapshot.
    fn on_start(
        &self,
        session_id: &str,
        cwd: &std::path::Path,
        requested_model: Option<&str>,
        served_model: Option<&str>,
        permission_mode: &str,
        sandbox: Option<&str>,
        always_approve: bool,
    ) {
        if !matches!(self.format, OutputFormat::StreamingJson) {
            return;
        }
        let confine = xai_grok_tools::types::resources::process_confine_root()
            .map(|p| p.display().to_string());
        let start = serde_json::json!({
            "type": "start",
            "schemaVersion": 1,
            "sessionId": session_id,
            "cwd": cwd.display().to_string(),
            "confineRoot": confine,
            "requestedModel": requested_model,
            "servedModel": served_model,
            "permissionMode": permission_mode,
            "sandbox": sandbox,
            "binary": "hyper",
            "version": xai_grok_version::VERSION,
            "alwaysApprove": always_approve,
        });
        println!("{start}");
    }

    /// Served model may only become known after session model resolution.
    fn on_model_resolved(&self, served_model: &str) {
        if !matches!(self.format, OutputFormat::StreamingJson) {
            return;
        }
        println!(
            "{}",
            serde_json::json!({
                "type": "model_resolved",
                "servedModel": served_model,
            })
        );
    }

    fn on_end(&mut self, stop_reason: &str, session_id: &str, request_id: &str) {
        match self.format {
            OutputFormat::Plain => {
                println!();
            }
            OutputFormat::StreamingJson => {
                let mut end = serde_json::json!({
                    "type": "end",
                    "schemaVersion": 1,
                    "stopReason": stop_reason,
                    "sessionId": session_id,
                    "requestId": request_id,
                    "filesChanged": self.files_changed_json(),
                });
                self.attach_usage_fields(&mut end, false);
                self.attach_structured_output(&mut end);
                println!("{end}");
            }
            OutputFormat::Json => {
                let mut result = self.build_json_result(stop_reason, session_id, request_id);
                result["filesChanged"] = self.files_changed_json();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string())
                );
            }
        }
    }

    fn on_error(&self, message: &str) {
        match self.format {
            OutputFormat::Plain => eprintln!("{message}"),
            OutputFormat::StreamingJson | OutputFormat::Json => {
                let mut err = serde_json::json!({"type":"error","message": message});
                // Always include `usage` on terminal paths (H7): snapshot when
                // we have one (possibly incomplete), null when the ledger never
                // started — never omit the key so harnesses do not guess.
                self.attach_usage_fields(&mut err, true);
                println!("{err}");
            }
        }
    }

    /// Attach `usage` / `usageIsIncomplete` for every terminal stream event.
    fn attach_usage_fields(&self, target: &mut serde_json::Value, incomplete_if_present: bool) {
        match &self.usage {
            Some(usage) => {
                attach_result_usage(target, usage);
                if incomplete_if_present {
                    target["usageIsIncomplete"] = serde_json::Value::Bool(true);
                }
            }
            None => {
                target["usage"] = serde_json::Value::Null;
            }
        }
    }
}

fn attach_result_usage(result: &mut serde_json::Value, usage: &serde_json::Value) {
    xai_grok_shell::extensions::notification::attach_result_usage_fail_closed(result, usage);
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn auto_respond_to_permissions(
    args: &acp::RequestPermissionRequest,
    option_kinds: &[acp::PermissionOptionKind],
) -> Option<acp::RequestPermissionResponse> {
    for &option_kind in option_kinds {
        for option in &args.options {
            if option.kind == option_kind {
                return Some(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                        option.option_id.clone(),
                    )),
                ));
            }
        }
    }
    None
}

/// Headless plan mode (and default headless) auto-allow pure read/search
/// tool kinds. Edit / Execute / Write still require YOLO or an `--allow`.
///
/// `ToolKind` here is the ACP wire kind on the permission request — not the
/// internal `xai_grok_tools` taxonomy. Read/Search/Fetch cover Grep, Glob,
/// Read, ListDir, WebFetch. Execute stays denied so Bash writes cannot slip
/// through plan mode.
fn headless_should_auto_allow_read(
    req: &acp::RequestPermissionRequest,
    permission_mode: Option<&str>,
) -> bool {
    // Plan mode is the primary case; default headless already auto-allows
    // read/grep via the permission manager's SAFE_COMMAND path, but a prompt
    // can still surface for MCP/edge tools that report Read/Search kind —
    // auto-allow those under plan only so plan mode matches harness
    // expectations without widening default headless.
    let plan = matches!(permission_mode, Some("plan"));
    if !plan {
        return false;
    }
    // ACP wire kinds for pure-read tools. Grep/Glob project as Search; list/read
    // as Read. Execute/Edit/Delete stay out so plan mode cannot write.
    // `RequestPermissionRequest.tool_call` is a `ToolCallUpdate` — kind lives
    // on `.fields`, not the top-level update.
    matches!(
        req.tool_call.fields.kind,
        Some(acp::ToolKind::Read) | Some(acp::ToolKind::Search)
    )
}

/// Explicit headless denial: prefer RejectOnce so the shell continues the
/// turn with a model-visible error; emit a `tool_denied` stream event for
/// harnesses; fall back to Cancelled only when no reject option exists.
fn headless_deny_permission(
    req: &acp::RequestPermissionRequest,
    permission_mode: Option<&str>,
    emitter: &mut HeadlessEmitter,
) -> acp::RequestPermissionResponse {
    // Title is the human-facing label (e.g. "Grep `pattern`") on
    // `fields.title`. Best-effort tool label for the stream event — the
    // model-facing body is composed in the shell from the decision outcome.
    let tool_name = req
        .tool_call
        .fields
        .title
        .as_deref()
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or("tool");
    let mode = permission_mode.unwrap_or("default");
    let reason = format!(
        "no approval is possible in headless mode (permission mode: {mode}). \
         Re-run with --always-approve, or add an --allow rule."
    );
    emitter.on_tool_denied(tool_name, &reason);
    if let Some(resp) =
        auto_respond_to_permissions(req, &[acp::PermissionOptionKind::RejectOnce])
    {
        return resp;
    }
    acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled)
}

/// "Not signed in" error message, tailored to the session type.
fn auth_required_message(interactive: bool) -> String {
    if interactive {
        "Not signed in. Run `grok login` to authenticate \
         (or `grok login --device-code` if no browser is available)."
            .to_string()
    } else {
        "Not signed in. To authenticate without a browser, run:\n  \
         grok login --device-code\n\n\
         Alternatively, set the XAI_API_KEY environment variable \
         or run `grok login` on a machine with a browser."
            .to_string()
    }
}

/// Authenticate using the agent's `defaultAuthMethodId` (source of truth for
/// `[auth] preferred_method`). Fail closed when no method is available — do not
/// invent api_key vs session ordering client-side.
///
/// Returns whether the selected method is API-key auth (for rate-limit copy).
async fn authenticate(
    acp_tx: &AcpAgentTx,
    auths: &[acp::AuthMethod],
    default_auth_method_id: Option<&acp::AuthMethodId>,
) -> anyhow::Result<bool> {
    let method_id = crate::acp::select_eager_auth_method(auths, default_auth_method_id)
        .ok_or_else(|| {
            use std::io::IsTerminal;
            let interactive = std::io::stdin().is_terminal()
                && !xai_grok_shell::util::clipboard::is_remote_session();
            anyhow::anyhow!("{}", auth_required_message(interactive))
        })?;
    let kind = AuthMethodKind::from_id(&method_id);
    // Prefer non-interactive methods only; interactive login is not usable headless.
    if kind.needs_interactive_login() {
        use std::io::IsTerminal;
        let interactive =
            std::io::stdin().is_terminal() && !xai_grok_shell::util::clipboard::is_remote_session();
        anyhow::bail!("{}", auth_required_message(interactive));
    }
    let is_api_key_auth = kind.is_api_key();
    let _resp: acp::AuthenticateResponse = acp_send(
        acp::AuthenticateRequest::new(method_id)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        acp_tx,
    )
    .await?;
    Ok(is_api_key_auth)
}

fn build_headless_init_request(
    rules: Option<&str>,
    system_prompt_override: Option<&str>,
    allow_interactive_questions: bool,
) -> acp::InitializeRequest {
    let mut meta = serde_json::json!({
        "clientType": HEADLESS_CLIENT_TYPE,
        "clientVersion": PAGER_CLIENT_VERSION,
    });
    if let Some(rules) = rules {
        meta["rules"] = serde_json::json!(rules);
    }
    if let Some(system_prompt_override) = system_prompt_override {
        meta["systemPromptOverride"] = serde_json::json!(system_prompt_override);
    }
    // `nonInteractive: true` is what `build_spawn_system_prompt` keys on for
    // the HYPER-1 no-questions clause. Only suppress that clause when the
    // harness explicitly opts back into interactive questions.
    meta["startupHints"] = serde_json::json!({
        "nonInteractive": true,
        "skipGitStatus": true,
        "skipProjectLayout": true,
    });
    if allow_interactive_questions {
        meta["allowInteractiveQuestions"] = serde_json::json!(true);
    }

    acp::InitializeRequest::new(acp::ProtocolVersion::V1)
        .client_capabilities(
            acp::ClientCapabilities::new()
                .fs(acp::FileSystemCapabilities::new())
                .terminal(false),
        )
        .meta(meta.as_object().cloned())
}

// ── HYPER-1: headless question detection + one-shot auto-continue ────────

/// Internal nudge after a headless turn that only asked a question. Bounded
/// to **one** auto-continue per run — never loop.
const HEADLESS_QUESTION_NUDGE: &str = "\
There is no interactive user. Assume the answer is no to any optional feature \
and proceed with the task as given.";

/// Whether `stop_reason` is a normal successful end (Debug of ACP `EndTurn`).
/// Other reasons (Cancelled, MaxTokens, Refusal, …) are not question-shaped
/// failures and must not trigger auto-continue.
fn is_normal_end_turn(stop_reason: &str) -> bool {
    stop_reason == "EndTurn" || stop_reason == "end_turn"
}

/// Heuristic: the assistant's final message reads as a question to the user.
///
/// Field incident shape (HYPER-1):
/// `"…Want to try it? (Requires opening a local URL)"` — ends in a question
/// mark, optionally followed by a short parenthetical note. We deliberately
/// do **not** match the browser-feature string itself (it is not in this repo
/// and would bitrot); any turn whose final substance is a question is the
/// structural failure mode.
///
/// Long post-`?` tails ("Is X true? Here is a full multi-sentence answer.")
/// are **not** treated as questions — those runs did work verbally.
pub(crate) fn looks_like_user_question(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Whole body first (covers single-paragraph field incident).
    if line_reads_as_question(trimmed) {
        return true;
    }
    // Multi-line: last non-empty line is the usual place for a closing opt-in.
    let last_line = trimmed
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(trimmed);
    line_reads_as_question(last_line)
}

/// A line/body is a question when it ends with `?`, or ends with a short
/// parenthetical after a `?` (e.g. `Want to try it? (Requires opening a local
/// URL)`).
fn line_reads_as_question(s: &str) -> bool {
    let s = s.trim();
    if s.ends_with('?') {
        return true;
    }
    // `…? (short note)` — field incident browser opt-in.
    if let Some(q) = s.rfind('?') {
        let tail = s[q + 1..].trim();
        if tail.is_empty() {
            return true;
        }
        if tail.starts_with('(') && tail.ends_with(')') && tail.len() <= 80 {
            return true;
        }
    }
    false
}

/// True when a headless turn is the exact HYPER-1 failure shape: normal end,
/// zero tool calls, assistant text that is a question, and the harness has
/// not opted into interactive questions.
fn should_auto_continue_headless_question(
    stop_reason: &str,
    tool_calls: u32,
    assistant_text: &str,
    allow_interactive_questions: bool,
    already_continued: bool,
) -> bool {
    if allow_interactive_questions || already_continued {
        return false;
    }
    if !is_normal_end_turn(stop_reason) {
        return false;
    }
    if tool_calls > 0 {
        return false;
    }
    looks_like_user_question(assistant_text)
}

struct OpenedSession {
    session_id: acp::SessionId,
    models: ModelState,
}

async fn open_session(
    acp_tx: &AcpAgentTx,
    cwd: &Path,
    session_id_flag: Option<&str>,
    restore_code: Option<bool>,
) -> anyhow::Result<OpenedSession> {
    // Pager opens sessions before the agent resolves per-vendor compat;
    // default (all-on) preserves existing behavior — the agent applies
    // the resolved config once the session is live.
    let mcp_servers =
        cli_config::load_mcp_servers(cwd, &xai_grok_tools::types::compat::CompatConfig::default());

    if let Some(sid) = session_id_flag {
        let try_load: Result<acp::LoadSessionResponse, _> = acp_send(
            acp::LoadSessionRequest::new(acp::SessionId::new(sid.to_string()), cwd.to_path_buf())
                .mcp_servers(mcp_servers.clone())
                .meta({
                    let mut m = acp::Meta::new();
                    m.insert("noReplay".into(), serde_json::Value::Bool(true));
                    if let Some(true) = restore_code {
                        m.insert("x.ai/restore_code".into(), serde_json::Value::Bool(true));
                    }
                    Some(m)
                }),
            acp_tx,
        )
        .await;
        if let Ok(resp) = try_load {
            return Ok(OpenedSession {
                session_id: acp::SessionId::new(sid.to_string()),
                models: ModelState::from(resp.models),
            });
        }
        anyhow::bail!("Session does not exist");
    }

    let new_resp: acp::NewSessionResponse = acp_send(
        acp::NewSessionRequest::new(cwd.to_path_buf()).mcp_servers(mcp_servers),
        acp_tx,
    )
    .await?;
    Ok(OpenedSession {
        session_id: new_resp.session_id,
        models: ModelState::from(new_resp.models),
    })
}

async fn open_session_with_id(
    acp_tx: &AcpAgentTx,
    cwd: &Path,
    session_id: &str,
) -> anyhow::Result<OpenedSession> {
    let cwd_str = cwd.to_string_lossy();
    crate::app::session_startup::ensure_session_id_available(session_id, &cwd_str)?;
    let mcp_servers =
        cli_config::load_mcp_servers(cwd, &xai_grok_tools::types::compat::CompatConfig::default());
    let new_resp: acp::NewSessionResponse = acp_send(
        acp::NewSessionRequest::new(cwd.to_path_buf())
            .mcp_servers(mcp_servers)
            .meta(
                serde_json::json!({ "sessionId": session_id })
                    .as_object()
                    .cloned(),
            ),
        acp_tx,
    )
    .await?;
    Ok(OpenedSession {
        session_id: new_resp.session_id,
        models: ModelState::from(new_resp.models),
    })
}

async fn fork_then_open(
    acp_tx: &AcpAgentTx,
    launch_cwd: &Path,
    parent_id: &str,
    parent_cwd: Option<&Path>,
    new_id: Option<&str>,
    restore_code: Option<bool>,
) -> anyhow::Result<OpenedSession> {
    use crate::app::session_startup::{
        effective_fork_new_cwd, ensure_session_id_available, fork_response_error,
        fork_response_new_session_id, fork_session_params, parent_session_is_worktree,
    };
    let launch_cwd_str = launch_cwd.to_string_lossy().into_owned();
    // Align with interactive: child lands under parent session cwd when the
    // parent was resolved from another directory (`newCwd` = parent_cwd).
    let new_cwd_str = effective_fork_new_cwd(&launch_cwd_str, parent_cwd);
    let write_cwd = PathBuf::from(&new_cwd_str);
    if let Some(nid) = new_id {
        ensure_session_id_available(nid, &new_cwd_str)?;
    }
    let parent_is_worktree = parent_session_is_worktree(parent_id, &write_cwd);
    let payload = fork_session_params(parent_id, &write_cwd, new_id, parent_is_worktree);
    let req = acp::ExtRequest::new(
        "x.ai/session/fork",
        serde_json::value::to_raw_value(&payload)
            .expect("serialize fork params")
            .into(),
    );
    let resp = acp_send(req, acp_tx).await?;
    if let Some(err) = fork_response_error(resp.0.get()) {
        anyhow::bail!("fork failed: {err}");
    }
    let child = fork_response_new_session_id(resp.0.get())
        .ok_or_else(|| anyhow::anyhow!("fork response missing newSessionId"))?;
    match open_session(acp_tx, &write_cwd, Some(&child), restore_code).await {
        Ok(opened) => Ok(opened),
        Err(e) => Err(anyhow::anyhow!(
            "fork succeeded as {child} but load failed: {e}"
        )),
    }
}

/// Apply `-m` / effort after session open (via `resolve_effort_for_model`, then
/// SetSessionModel).
///
/// Headless maps the classified [`EffortTokenError`] differently from the TUI: a
/// one-shot run soft-ignores effort on a non-supporting model (still applying
/// `-m`) but hard-fails on a genuinely unknown token. The TUI instead keeps the
/// `-m` switch and only toasts — intentional, since headless has no scrollback
/// to carry a non-fatal warning.
async fn apply_headless_model_and_effort(
    acp_tx: &AcpAgentTx,
    session_id: &acp::SessionId,
    models: &ModelState,
    model_name: Option<&str>,
    effort_token: Option<&str>,
) -> anyhow::Result<()> {
    if model_name.is_none() && effort_token.is_none() {
        return Ok(());
    }

    let model_id = if let Some(name) = model_name {
        models
            .resolve_by_name_or_id(name)
            .unwrap_or_else(|| acp::ModelId::new(name))
    } else {
        models.current.clone().ok_or_else(|| {
            anyhow::anyhow!("--effort/--reasoning-effort: no active model to apply effort to")
        })?
    };

    let effort = match effort_token {
        None => None,
        // Pre-catalog: the canonical token was already stamped into the agent
        // config; a remapped menu id can't resolve without a loaded catalog.
        Some(token) if models.available.is_empty() => {
            if parse_canonical_effort_token(token).is_none() {
                // Do not hardcode a level list here: without a catalog we cannot
                // know what the model offers, and advertising none/minimal/… has
                // led users to try values that then 400 on the API.
                anyhow::bail!(
                    "--effort/--reasoning-effort: unknown effort level '{token}' \
                     (model catalog unavailable; remapped menu ids require a loaded catalog)"
                );
            }
            None
        }
        Some(token) => match models.resolve_effort_for_model(&model_id, token) {
            Ok(effort) => Some(effort),
            // Soft-ignore effort on a non-supporting model; still apply `-m`.
            Err(EffortTokenError::Unsupported) => {
                tracing::warn!(
                    model = %model_id.0,
                    token,
                    "--effort/--reasoning-effort: model does not support reasoning effort; ignoring"
                );
                None
            }
            Err(err) => anyhow::bail!("--effort/--reasoning-effort: {}", err.message()),
        },
    };

    // Nothing to apply (effort pre-stamped or ignored, and no model override):
    // skip the no-op SetSessionModel.
    if model_name.is_none() && effort.is_none() {
        return Ok(());
    }

    let meta = effort.map(|eff| {
        let mut m = acp::Meta::new();
        m.insert(
            REASONING_EFFORT_META_KEY.to_string(),
            reasoning_effort_meta_value(eff),
        );
        m
    });

    acp_send(
        acp::SetSessionModelRequest::new(session_id.clone(), model_id.clone()).meta(meta),
        acp_tx,
    )
    .await
    .map_err(|e| {
        if let Some(name) = model_name {
            anyhow::anyhow!(
                "Couldn't set model '{}': {}. Run 'grok models' to see available models.",
                name,
                e
            )
        } else {
            anyhow::anyhow!("Couldn't apply reasoning effort: {e}")
        }
    })?;
    tracing::debug!(
        model_id = %model_id.0,
        effort = ?effort,
        "headless: model/effort set"
    );
    Ok(())
}

// ── Main entry point ─────────────────────────────────────────────────────

/// Startup-materialization context for headless (`-p`) runs. Never chat:
/// `HeadlessOptions` carries no chat flag, so headless resume targets are
/// always disk/GCS Build sessions.
fn headless_materialize_ctx(
    has_worktree: bool,
    resume_title_pinned: bool,
) -> crate::app::session_startup::MaterializeCtx {
    crate::app::session_startup::MaterializeCtx {
        has_worktree,
        allow_remote_restore:
            crate::app::session_startup::MaterializeCtx::default_allow_remote_restore(),
        chat_mode: false,
        title_resolution: if resume_title_pinned {
            crate::app::session_startup::TitleResolution::PinnedPreSandbox
        } else {
            crate::app::session_startup::TitleResolution::Allowed
        },
    }
}

/// Run a headless single-turn prompt.
///
/// Spawns the agent in-process, runs the full ACP lifecycle (init → auth →
/// session → prompt), streams output to stdout, and returns cleanly.
pub async fn run_single_turn(
    prompt: HeadlessPrompt,
    verbatim: bool,
    options: HeadlessOptions,
) -> Result<()> {
    // Stamp proxy requests as headless before the agent spawns and issues
    // its first request (auth enrichment, model list, etc.).
    xai_grok_shell::http::set_process_client_mode_headless();

    let cwd = match options.cwd {
        None => std::env::current_dir()?,
        Some(ref p) => dunce::canonicalize(p)?,
    };

    let mut emitter = HeadlessEmitter::new(options.output_format, options.json_schema.is_some());

    // Load config and spawn agent
    let t_spawn = Instant::now();
    let raw_config = xai_grok_shell::config::load_effective_config()
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    let mut agent_config = AgentConfig::new_from_toml_cfg(&raw_config)
        .map_err(|e| anyhow::anyhow!("Failed to create agent config: {e}"))?;

    // Canonical-only early stamp; remaps need the post-session catalog resolve below.
    if let Some(ref token) = options.reasoning_effort
        && let Some(effort) = parse_canonical_effort_token(token)
    {
        agent_config.reasoning_effort_override = Some(effort);
    }
    // So initial system prompt / `system_prompt_label` use `-m`, not a later SetSessionModel.
    if let Some(ref model) = options.model {
        agent_config.default_model_override = Some(model.clone());
    }

    agent_config.resolve_runtime_fields(&xai_grok_shell::agent::config::RuntimeResolutionContext {
        raw_config: &raw_config,
        remote_settings: None,
        is_headless: true,
        cli_subagents: None,
        cli_web_search_model: None,
        cli_session_summary_model: None,
        cli_experimental_memory: false,
        cli_no_memory: false,
        disable_web_search: options.disable_web_search,
        todo_gate: false,
        laziness_debug_log: None,
        storage_mode: None,
    });

    agent_config.mode = xai_grok_shell::agent::config::AgentMode::Headless;
    agent_config.default_yolo_mode = options.yolo;
    // Remote arg is None: the remote settings permission_mode soft-default is
    // TUI-only; headless runs must not change permission behavior on a
    // remote flag flip.
    agent_config.default_auto_mode = xai_grok_shell::util::config::effective_auto_for_launch(
        options.yolo,
        options.permission_mode_flag.as_deref(),
        None,
    );

    // No agent-level hub client URL (gateway-only cloud; workspace provider
    // hub_url lives on `grok workspace` / WorkspaceStartArgs only).

    apply_agent_flag(&options.agent, &mut agent_config);

    if let Some(ref json) = options.agents_json {
        agent_config.cli_agents = parse_cli_agents(json)?;
    }

    agent_config.cli_agent_overrides = xai_grok_shell::agent::config::CliAgentOverrides {
        tools: parse_comma_list(options.cli_tools.as_deref()),
        disallowed_tools: parse_comma_list(options.cli_disallowed_tools.as_deref()),
        permission_rules: parse_permission_rules_strict(&options.allow_rules, &options.deny_rules)?,
        max_turns: options.max_turns,
        permission_mode: options
            .permission_mode_flag
            .as_deref()
            .map(|s| {
                serde_json::from_value(serde_json::Value::String(s.to_string()))
                    .map_err(|e| anyhow::anyhow!("--permission-mode: invalid value: {e}"))
            })
            .transpose()?,
    };

    // Persist an explicit --trust grant before the agent starts.
    if options.trust {
        xai_grok_shell::agent::folder_trust::grant_folder_trust(&cwd);
    }

    let cancel = CancellationToken::new();
    let memory_config = agent_config.memory_config.clone();
    let spawned = match spawn_grok_shell(agent_config, &cancel, memory_config).await {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Couldn't start session: {e}");
            emitter.on_error(&msg);
            anyhow::bail!("{msg}");
        }
    };
    // Cancel + join on every return path (success or bail).
    let _agent_guard = AgentShutdownGuard::new(cancel.clone(), Some(spawned.thread_handle));
    let (acp_tx, mut acp_rx) = (spawned.channel.tx, spawned.channel.rx);
    crate::unified_log::init(acp_tx.clone());
    crate::unified_log::info(
        "pager started",
        None,
        Some(serde_json::json!({"mode": "headless"})),
    );
    crate::unified_log::flush();

    // Initialize with headless hints
    let init_req = build_headless_init_request(
        options.rules.as_deref(),
        options.system_prompt_override.as_deref(),
        options.allow_interactive_questions,
    );
    let init_resp: acp::InitializeResponse = match acp_send(init_req, &acp_tx).await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("Couldn't initialize: {e}");
            emitter.on_error(&msg);
            anyhow::bail!("{msg}");
        }
    };
    tracing::debug!(
        elapsed_ms = t_spawn.elapsed().as_millis() as u64,
        "headless: spawn + initialize complete"
    );

    // Authenticate using agent defaultAuthMethodId (preferred_method pin).
    let t_auth = Instant::now();
    let default_auth_method_id = crate::acp::parse_default_auth_method_id(init_resp.meta.as_ref());
    let is_api_key_auth = match authenticate(
        &acp_tx,
        &init_resp.auth_methods,
        default_auth_method_id.as_ref(),
    )
    .await
    {
        Ok(is_api_key) => is_api_key,
        Err(e) => {
            emitter.on_error(&e.to_string());
            return Err(e);
        }
    };
    tracing::debug!(
        elapsed_ms = t_auth.elapsed().as_millis() as u64,
        "headless: authenticate complete"
    );

    // Same intent + materialize path as interactive (shared SSOT).
    use crate::app::session_startup::{self, MaterializedStartup, SessionStartupFlags};
    let has_resume_id = options.resume.as_deref().filter(|s| !s.is_empty());
    let resume_most_recent = options.resume.as_deref() == Some("");
    let intent = session_startup::session_startup_intent_from_flags(SessionStartupFlags {
        session_id: options.session_id.as_deref(),
        resume_session_id: has_resume_id,
        resume_most_recent,
        continue_last_session: options.continue_last_session,
        fork_session: options.fork_session,
        has_worktree: options.worktree.is_some(),
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let cwd_str = cwd.to_string_lossy().to_string();
    let materialized = session_startup::materialize_startup_for_cwd(
        headless_materialize_ctx(options.worktree.is_some(), options.resume_title_pinned),
        intent,
        &cwd_str,
    )
    .await?;

    // Open session
    let restore_code = options.restore_code.then_some(true);
    let t_session = Instant::now();
    let opened = match materialized {
        MaterializedStartup::NewAuto => open_session(&acp_tx, &cwd, None, None).await,
        MaterializedStartup::NewWithId { session_id } => {
            open_session_with_id(&acp_tx, &cwd, &session_id).await
        }
        MaterializedStartup::Resume {
            session_id,
            original_cwd,
            ..
        } => {
            let load_cwd = original_cwd.as_deref().unwrap_or(cwd.as_path());
            open_session(&acp_tx, load_cwd, Some(session_id.as_str()), restore_code).await
        }
        MaterializedStartup::Fork {
            parent_session_id,
            parent_cwd,
            new_session_id,
            ..
        } => {
            fork_then_open(
                &acp_tx,
                &cwd,
                &parent_session_id,
                parent_cwd.as_deref(),
                new_session_id.as_deref(),
                restore_code,
            )
            .await
        }
    };
    let OpenedSession {
        session_id,
        models: session_models,
    } = match opened {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("Couldn't create session: {e}");
            emitter.on_error(&msg);
            anyhow::bail!("{msg}");
        }
    };
    tracing::debug!(
        elapsed_ms = t_session.elapsed().as_millis() as u64,
        session_id = %session_id.0,
        "headless: open_session complete"
    );

    // Debug: track headless sessions in active_sessions.json when env var is set.
    let track_active = std::env::var("GROK_TRACK_HEADLESS").is_ok();
    if track_active {
        let _ = xai_grok_shell::active_sessions::register(
            xai_grok_shell::active_sessions::ActiveSession {
                session_id: session_id.clone(),
                pid: std::process::id(),
                cwd: cwd.display().to_string(),
                opened_at: chrono::Utc::now(),
            },
        );
    }

    if let Err(e) = apply_headless_model_and_effort(
        &acp_tx,
        &session_id,
        &session_models,
        options.model.as_deref(),
        options.reasoning_effort.as_deref(),
    )
    .await
    {
        let msg = e.to_string();
        emitter.on_error(&msg);
        anyhow::bail!("{msg}");
    }

    // H6: first NDJSON line before any text/thought chunks.
    let permission_mode = options
        .permission_mode_flag
        .as_deref()
        .unwrap_or(if options.yolo {
            "bypassPermissions"
        } else {
            "default"
        });
    let served = session_models
        .current
        .as_ref()
        .map(|m| m.0.to_string());
    emitter.on_start(
        session_id.0.as_ref(),
        &cwd,
        options.model.as_deref(),
        served.as_deref(),
        permission_mode,
        xai_grok_sandbox::configured_profile_name(),
        options.yolo,
    );
    // If served model only becomes known after SetSessionModel, announce it.
    if let (Some(requested), Some(served_s)) = (options.model.as_deref(), served.as_deref())
        && requested != served_s
    {
        emitter.on_model_resolved(served_s);
    }

    // Send prompt and stream response. Outer loop allows exactly one
    // HYPER-1 auto-continue after a question-only first turn.
    let mut prompt_blocks = prompt.into_content_blocks();
    let mut headless_question_continued = false;
    let mut awaiting_user_input = false;

    let prompt_meta_base = {
        let mut meta = serde_json::Map::new();
        if verbatim {
            meta.insert("verbatim".to_string(), serde_json::Value::Bool(true));
        }
        if let Some(ref schema) = options.json_schema {
            meta.insert("outputSchema".to_string(), schema.clone());
        }
        // Screen-mode telemetry (`prompt_submitted.screen_mode`): headless is
        // its own mode, distinct from the TUI's fullscreen/inline/minimal.
        meta.insert(
            "screenMode".to_string(),
            serde_json::Value::String("headless".to_string()),
        );
        meta
    };

    // Pending background work: bash/monitor via x.ai/task_backgrounded +
    // task_completed; background subagents via SubagentSpawned + SubagentFinished
    // on x.ai/session_notification (prefixed `subagent:{id}` in pending_bg).
    // Tracked regardless of wait_for_background so the exit reaper always
    // sees still-running work; the flag only gates waiting.
    // No idle/quiet polling and no wait for server-side auto-wake text — exit
    // when lifecycle sets are empty. Auto-wake may still be in flight at exit.
    let mut pending_bg: HashSet<String> = HashSet::new();
    // task_completed can arrive before task_backgrounded; remember those IDs
    // so a late backgrounded does not re-arm waiting.
    let mut completed_before_bg: HashSet<String> = HashSet::new();
    let mut prompt_result = None;

    // Bound: one user prompt + at most one internal nudge. Never loop further.
    for prompt_attempt in 0u32..2 {
        if prompt_attempt > 0 {
            // Second attempt is the HYPER-1 nudge only.
            emitter.reset_turn_tracking();
            pending_bg.clear();
            completed_before_bg.clear();
        }

        let request =
            acp::PromptRequest::new(session_id.clone(), prompt_blocks.clone()).meta(Some(
                prompt_meta_base.clone(),
            ));
        let t_prompt = Instant::now();
        let mut ttf_logged = false;
        let mut prompt_fut = Box::pin(acp_send(request, &acp_tx));
        let mut this_result = None;
        let mut prompt_done_at: Option<Instant> = None;

        loop {
            // First turn done and no tracked bg/monitor tasks still running.
            // Drain buffered ACP first: PromptResponse can complete while
            // task_backgrounded is still queued on acp_rx (never reached select!).
            if options.wait_for_background && this_result.is_some() && pending_bg.is_empty() {
                while let Ok(msg) = acp_rx.try_recv() {
                    handle_headless_acp_message(
                        msg.boxed(),
                        &mut emitter,
                        t_prompt,
                        &mut ttf_logged,
                        options.yolo,
                        options.permission_mode_flag.as_deref(),
                        options.output_format,
                        &mut pending_bg,
                        &mut completed_before_bg,
                    );
                }
                if pending_bg.is_empty() {
                    tracing::debug!("headless: no pending background tasks, exiting");
                    break;
                }
            }

            // Safety valve so evals don't hang on long-lived monitors or stuck tasks.
            if options.wait_for_background
                && let Some(done_at) = prompt_done_at
                && done_at.elapsed() >= options.background_wait_timeout
            {
                tracing::warn!(
                    pending_bg = pending_bg.len(),
                    timeout_secs = options.background_wait_timeout.as_secs(),
                    "headless: background wait timed out, exiting"
                );
                break;
            }

            // Only needed while waiting on tasks (timeout enforcement); otherwise
            // the loop blocks on ACP until task_completed or PromptResponse.
            let timeout_deadline = if options.wait_for_background
                && this_result.is_some()
                && !pending_bg.is_empty()
                && let Some(done_at) = prompt_done_at
            {
                let remaining = options
                    .background_wait_timeout
                    .saturating_sub(done_at.elapsed());
                if remaining.is_zero() {
                    Duration::from_millis(50)
                } else {
                    remaining
                }
            } else {
                Duration::from_secs(3600)
            };

            tokio::select! {
                biased;
                msg = acp_rx.recv() => {
                    let Some(msg) = msg else {
                        emitter.on_error("Connection closed unexpectedly");
                        anyhow::bail!("Connection closed unexpectedly");
                    };
                    handle_headless_acp_message(
                        msg.boxed(),
                        &mut emitter,
                        t_prompt,
                        &mut ttf_logged,
                        options.yolo,
                        options.permission_mode_flag.as_deref(),
                        options.output_format,
                        &mut pending_bg,
                        &mut completed_before_bg,
                    );
                }
                res = &mut prompt_fut, if this_result.is_none() => {
                    this_result = Some(res);
                    prompt_done_at = Some(Instant::now());
                    if !options.wait_for_background {
                        drain_acp_with_grace(
                            &mut acp_rx,
                            Duration::from_millis(750),
                            &mut emitter,
                            t_prompt,
                            &mut ttf_logged,
                            options.yolo,
                            options.permission_mode_flag.as_deref(),
                            options.output_format,
                            &mut pending_bg,
                            &mut completed_before_bg,
                        )
                        .await;
                        break;
                    }
                    // With wait_for_background: keep draining ACP for task_completed.
                }
                _ = tokio::time::sleep(timeout_deadline), if options.wait_for_background
                    && this_result.is_some()
                    && !pending_bg.is_empty() =>
                {
                    // Wake to re-check background_wait_timeout at the top of the loop.
                }
            }
        }

        // Drain lifecycle notifications still queued at loop exit so the
        // reaper sees them (the timeout path breaks without draining).
        while let Ok(msg) = acp_rx.try_recv() {
            handle_headless_acp_message(
                msg.boxed(),
                &mut emitter,
                t_prompt,
                &mut ttf_logged,
                options.yolo,
                options.permission_mode_flag.as_deref(),
                options.output_format,
                &mut pending_bg,
                &mut completed_before_bg,
            );
        }

        // Kill background tasks/subagents still pending after this prompt so
        // they don't outlive the process (or pollute the auto-continue turn).
        if !pending_bg.is_empty() {
            tracing::warn!(
                pending_bg = pending_bg.len(),
                "headless: killing background work still pending after prompt"
            );
            reap_pending_background_tasks(&pending_bg, &session_id, &acp_tx).await;
            pending_bg.clear();
        }

        prompt_result = this_result;

        // HYPER-1: if the first turn was a pure question with no tools, nudge
        // once and re-enter. Hard bound: never auto-continue a second time.
        let Some(Ok(resp)) = prompt_result.as_ref() else {
            break;
        };
        let stop_reason = format!("{:?}", resp.stop_reason);
        if should_auto_continue_headless_question(
            &stop_reason,
            emitter.turn_tool_calls,
            &emitter.turn_assistant_text,
            options.allow_interactive_questions,
            headless_question_continued,
        ) {
            headless_question_continued = true;
            emitter.on_auto_continue("headless_question", 1);
            tracing::info!(
                "headless: question-ending turn with zero tool calls; auto-continuing once"
            );
            prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
                HEADLESS_QUESTION_NUDGE,
            ))];
            continue;
        }
        // Second turn also made no tool calls after the nudge → honest
        // AwaitingUserInput (never report a clean EndTurn for a run that
        // only asked a question). Harnesses key completed-blind off EndTurn.
        if headless_question_continued
            && is_normal_end_turn(&stop_reason)
            && emitter.turn_tool_calls == 0
        {
            awaiting_user_input = true;
        }
        break;
    }

    // Flush buffered unified log entries before exit.
    crate::unified_log::flush_blocking().await;

    // Handle result
    if track_active {
        // Non-blocking flock so a slow/network ~/.grok can't hang exit.
        let _ = xai_grok_shell::active_sessions::try_unregister(&session_id);
    }
    // Agent cancel + join (SessionEnd flush) runs in AgentShutdownGuard::drop.
    match prompt_result {
        Some(Ok(resp)) => {
            let mut stop_reason = format!("{:?}", resp.stop_reason);
            emitter.set_structured_output_from_meta(resp.meta.as_ref());
            emitter.set_usage_from_meta(resp.meta.as_ref());
            let sid = resp
                .meta
                .as_ref()
                .and_then(|m| m.get("sessionId"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let rid = resp
                .meta
                .as_ref()
                .and_then(|m| m.get("requestId"))
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let is_max_turns = resp
                .meta
                .as_ref()
                .and_then(|m| m.get("cancellationCategory"))
                .and_then(|v| v.as_str())
                == Some("max_turns_reached");
            // --require-changes: treat a productive stop with zero agent
            // tool edits as NoChanges so harnesses can key on stopReason.
            let no_changes = options.require_changes && emitter.files_changed.is_empty();
            if awaiting_user_input {
                // Distinct terminal signal: the run only asked a question
                // (even after one auto-continue). filesChanged.count stays 0.
                stop_reason = "AwaitingUserInput".to_string();
            } else if no_changes && !is_max_turns {
                stop_reason = "NoChanges".to_string();
            }
            if is_max_turns {
                match emitter.format {
                    OutputFormat::Plain => eprintln!("Max turns reached"),
                    OutputFormat::StreamingJson => {
                        let mut ev = serde_json::json!({"type": "max_turns_reached"});
                        emitter.attach_usage_fields(&mut ev, true);
                        println!("{ev}");
                    }
                    OutputFormat::Json => {} // conveyed by stopReason in the final JSON
                }
                emitter.on_end(&stop_reason, sid, rid);
                anyhow::bail!("max turns reached");
            }
            emitter.on_end(&stop_reason, sid, rid);
            if awaiting_user_input {
                anyhow::bail!(
                    "headless: run ended awaiting user input (question-only turn; no tools)"
                );
            }
            if no_changes {
                anyhow::bail!("require-changes: run finished with no agent file edits");
            }
            Ok(())
        }
        Some(Err(err)) => {
            let msg = if i32::from(err.code) == RATE_LIMITED_ERROR_CODE {
                let detail = err.data.as_ref().and_then(error_detail_from_data);
                crate::app::sanitize_user_error(&format_rate_limited_user_message(
                    detail.as_deref(),
                    is_api_key_auth,
                ))
            } else {
                err.to_string()
            };
            if let Some(usage) = xai_grok_shell::sampling::error::prompt_usage_from_error(&err)
                && let Ok(v) = serde_json::to_value(&usage)
            {
                emitter.usage = Some(v);
            }
            emitter.on_error(&msg);
            anyhow::bail!("{msg}")
        }
        None => Ok(()),
    }
}

/// Ext request that kills pending background work `key` (a `pending_bg`
/// entry): `subagent:{id}` cancels the subagent, anything else kills the
/// bash/monitor task with that id.
fn reap_request_for_key(
    key: &str,
    session_id: &acp::SessionId,
) -> serde_json::Result<acp::ExtRequest> {
    let (method, params) = match key.strip_prefix("subagent:") {
        Some(id) => (
            "x.ai/subagent/cancel",
            serde_json::value::to_raw_value(&CancelSubagentRequest {
                subagent_id: id.to_string(),
            })?,
        ),
        None => (
            "x.ai/task/kill",
            serde_json::value::to_raw_value(&KillTaskRequest {
                session_id: session_id.0.to_string(),
                task_id: key.to_string(),
            })?,
        ),
    };
    Ok(acp::ExtRequest::new(method, params.into()))
}

/// Best-effort kill of background work still pending when headless exits
/// (background-wait timeout or `--no-wait-for-background`) so model-spawned
/// processes never outlive the process. Failures are logged, never fatal.
async fn reap_pending_background_tasks(
    pending_bg: &HashSet<String>,
    session_id: &acp::SessionId,
    acp_tx: &AcpAgentTx,
) {
    for key in pending_bg {
        let request = match reap_request_for_key(key, session_id) {
            Ok(request) => request,
            Err(e) => {
                tracing::warn!(key = %key, error = %e, "headless: failed to build reap request");
                continue;
            }
        };
        let method = request.method.clone();
        match tokio::time::timeout(Duration::from_secs(10), acp_send(request, acp_tx)).await {
            Ok(Ok(_)) => {
                tracing::debug!(key = %key, %method, "headless: reaped pending background work")
            }
            Ok(Err(e)) => {
                tracing::warn!(key = %key, %method, error = %e, "headless: failed to reap background work")
            }
            Err(_) => {
                tracing::warn!(key = %key, %method, "headless: timed out reaping background work")
            }
        }
    }
}

/// Track a background lifecycle event in the pending set.
///
/// Tracking is unconditional — independent of `--no-wait-for-background` — so
/// the exit reaper sees everything still running. `wait_for_background` only
/// gates whether the loop waits for this set to drain.
fn track_background_lifecycle(
    event: ExtEvent,
    pending_bg: &mut HashSet<String>,
    completed_before_bg: &mut HashSet<String>,
) {
    match event {
        ExtEvent::TaskBackgrounded {
            task_id,
            is_monitor,
        } => {
            if !completed_before_bg.remove(&task_id) {
                pending_bg.insert(task_id);
                tracing::debug!(
                    pending = pending_bg.len(),
                    is_monitor,
                    "headless: tracking background task"
                );
            }
        }
        ExtEvent::TaskCompleted { task_id } => {
            if pending_bg.remove(&task_id) {
                tracing::debug!(
                    pending = pending_bg.len(),
                    "headless: background task completed"
                );
            } else {
                completed_before_bg.insert(task_id);
            }
        }
        ExtEvent::SubagentSpawned { subagent_id } => {
            let key = format!("subagent:{subagent_id}");
            if !completed_before_bg.remove(&key) {
                pending_bg.insert(key);
                tracing::debug!(
                    pending = pending_bg.len(),
                    "headless: tracking background subagent"
                );
            }
        }
        ExtEvent::SubagentFinished { subagent_id } => {
            let key = format!("subagent:{subagent_id}");
            if pending_bg.remove(&key) {
                tracing::debug!(
                    pending = pending_bg.len(),
                    "headless: background subagent finished"
                );
            } else {
                completed_before_bg.insert(key);
            }
        }
        ExtEvent::MonitorEvent | ExtEvent::None => {}
    }
}

// ── ACP client message handling (select arm + pre-exit drain) ────────────

#[allow(clippy::too_many_arguments)]
async fn drain_acp_with_grace(
    acp_rx: &mut AcpClientRx,
    grace: Duration,
    emitter: &mut HeadlessEmitter,
    t_prompt: Instant,
    ttf_logged: &mut bool,
    yolo: bool,
    permission_mode: Option<&str>,
    output_format: OutputFormat,
    pending_bg: &mut HashSet<String>,
    completed_before_bg: &mut HashSet<String>,
) {
    let deadline = Instant::now() + grace;
    loop {
        while let Ok(msg) = acp_rx.try_recv() {
            handle_headless_acp_message(
                msg.boxed(),
                emitter,
                t_prompt,
                ttf_logged,
                yolo,
                permission_mode,
                output_format,
                pending_bg,
                completed_before_bg,
            );
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::select! {
            biased;
            msg = acp_rx.recv() => {
                let Some(msg) = msg else { break; };
                handle_headless_acp_message(
                    msg.boxed(),
                    emitter,
                    t_prompt,
                    ttf_logged,
                    yolo,
                    permission_mode,
                    output_format,
                    pending_bg,
                    completed_before_bg,
                );
            }
            _ = tokio::time::sleep(remaining) => {
                break;
            }
        }
    }
}

/// Process one inbound ACP client message. Used by both `acp_rx.recv()` and
/// `try_recv()` so buffered `task_backgrounded` is not dropped when
/// `PromptResponse` completes first.
#[allow(clippy::too_many_arguments)]
fn handle_headless_acp_message(
    msg: AcpClientMessageBox,
    emitter: &mut HeadlessEmitter,
    t_prompt: Instant,
    ttf_logged: &mut bool,
    yolo: bool,
    // CLI `--permission-mode` (e.g. `plan`, `auto`); drives headless
    // read-class auto-allow and the denial stream event reason.
    permission_mode: Option<&str>,
    output_format: OutputFormat,
    pending_bg: &mut HashSet<String>,
    completed_before_bg: &mut HashSet<String>,
) {
    match msg {
        AcpClientMessageBox::SessionNotification(boxed) => {
            match &boxed.request.update {
                acp::SessionUpdate::AgentMessageChunk(chunk) => {
                    if let acp::ContentBlock::Text(text) = &chunk.content
                        && !text.text.is_empty()
                    {
                        if !*ttf_logged {
                            *ttf_logged = true;
                            tracing::debug!(
                                elapsed_ms = t_prompt.elapsed().as_millis() as u64,
                                "headless: time-to-first-chunk"
                            );
                        }
                        emitter.on_text_chunk(&text.text);
                    }
                }
                acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                    if let acp::ContentBlock::Text(text) = &chunk.content {
                        if !*ttf_logged {
                            *ttf_logged = true;
                            tracing::debug!(
                                elapsed_ms = t_prompt.elapsed().as_millis() as u64,
                                "headless: time-to-first-thought"
                            );
                        }
                        emitter.on_thought_chunk(&text.text);
                    }
                }
                // Any tool call counts for the HYPER-1 "did work" check; Edit
                // locations still feed filesChanged separately.
                acp::SessionUpdate::ToolCall(tc) => {
                    emitter.note_tool_call();
                    if matches!(tc.kind, acp::ToolKind::Edit)
                        && matches!(
                            tc.status,
                            acp::ToolCallStatus::Completed | acp::ToolCallStatus::InProgress
                        )
                    {
                        // Prefer completed; still record locations when the
                        // initial ToolCall already carries them (InProgress).
                        emitter.note_edit_locations(&tc.locations);
                    }
                }
                acp::SessionUpdate::ToolCallUpdate(tcu) => {
                    // Updates do not re-count as new tool calls (the ToolCall
                    // event already did). Only harvest Edit locations.
                    if matches!(tcu.fields.kind, Some(acp::ToolKind::Edit))
                        && matches!(tcu.fields.status, Some(acp::ToolCallStatus::Completed))
                        && let Some(locs) = tcu.fields.locations.as_ref()
                    {
                        emitter.note_edit_locations(locs);
                    }
                }
                _ => {}
            }
            let _ = boxed.response_tx.send(Ok(()));
        }
        AcpClientMessageBox::RequestPermission(req) => {
            // Headless cannot prompt. Prefer Allow for YOLO / plan-mode
            // read-class tools; otherwise RejectOnce so the shell maps the
            // outcome to a non-empty model-facing error (never silent Cancel
            // alone — Cancelled historically ended the turn with an empty
            // tool body that models interpret as "no matches").
            let allow = yolo || headless_should_auto_allow_read(&req.request, permission_mode);
            if allow {
                if let Some(resp) = auto_respond_to_permissions(
                    &req.request,
                    &[
                        acp::PermissionOptionKind::AllowOnce,
                        acp::PermissionOptionKind::AllowAlways,
                    ],
                ) {
                    let _ = req.response_tx.send(Ok(resp));
                } else {
                    // No allow option offered — fall through to explicit deny.
                    let denied = headless_deny_permission(&req.request, permission_mode, emitter);
                    let _ = req.response_tx.send(Ok(denied));
                }
            } else {
                let denied = headless_deny_permission(&req.request, permission_mode, emitter);
                let _ = req.response_tx.send(Ok(denied));
            }
        }
        AcpClientMessageBox::ExtNotification(notif) => {
            let event = handle_ext_notification(&notif, output_format);
            let _ = notif.response_tx.send(Ok(()));
            track_background_lifecycle(event, pending_bg, completed_before_bg);
        }
        AcpClientMessageBox::WaitForTerminalExit(args) => {
            args.response_tx
                .send(Err(crate::acp::wait_for_exit_not_supported(
                    "headless mode",
                )))
                .ok();
        }
        _ => {}
    }
}

// ── Extension notification handling ──────────────────────────────────────

enum ExtEvent {
    None,
    TaskBackgrounded {
        task_id: String,
        is_monitor: bool,
    },
    TaskCompleted {
        task_id: String,
    },
    SubagentSpawned {
        subagent_id: String,
    },
    SubagentFinished {
        subagent_id: String,
    },
    /// Monitor emitted a line (or ended streaming). Does not complete the task;
    /// completion still arrives via `TaskCompleted`.
    MonitorEvent,
}

fn handle_ext_notification(
    notif: &xai_acp_lib::AcpArgsBox<acp::ExtNotification>,
    format: OutputFormat,
) -> ExtEvent {
    let method = notif.request.method.as_ref();

    // Background task lifecycle uses dedicated methods (not session_notification).
    if method == "x.ai/task_backgrounded" {
        #[derive(serde::Deserialize)]
        struct TaskBgEnvelope {
            update: TaskBgUpdate,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "snake_case", tag = "sessionUpdate")]
        enum TaskBgUpdate {
            TaskBackgrounded {
                task_id: String,
                #[serde(default)]
                monitor_description: Option<String>,
            },
            #[serde(other)]
            Other,
        }
        if let Ok(env) = serde_json::from_str::<TaskBgEnvelope>(notif.request.params.get())
            && let TaskBgUpdate::TaskBackgrounded {
                task_id,
                monitor_description,
            } = env.update
        {
            return ExtEvent::TaskBackgrounded {
                task_id,
                is_monitor: monitor_description.is_some(),
            };
        }
        return ExtEvent::None;
    }

    if method == "x.ai/task_completed" {
        #[derive(serde::Deserialize)]
        struct TaskDoneEnvelope {
            update: TaskDoneUpdate,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "snake_case", tag = "sessionUpdate")]
        enum TaskDoneUpdate {
            TaskCompleted {
                task_snapshot: TaskSnapshotLite,
            },
            #[serde(other)]
            Other,
        }
        #[derive(serde::Deserialize)]
        struct TaskSnapshotLite {
            task_id: String,
        }
        if let Ok(env) = serde_json::from_str::<TaskDoneEnvelope>(notif.request.params.get())
            && let TaskDoneUpdate::TaskCompleted { task_snapshot } = env.update
        {
            return ExtEvent::TaskCompleted {
                task_id: task_snapshot.task_id,
            };
        }
        return ExtEvent::None;
    }

    if method == "x.ai/monitor_event" {
        return ExtEvent::MonitorEvent;
    }

    match method {
        "x.ai/session_notification" | "x.ai/session/update" => {}
        // Announcement / CTA pushes (`x.ai/announcements/update`) are
        // intentionally dropped here. Headless has no UI to paint them, and
        // the shell also skips the push when `is_headless` (HYPER-1 defence).
        // Do not stream them as text or wait for a click.
        _ => return ExtEvent::None,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case", tag = "sessionUpdate")]
    enum XaiUpdate {
        AutoCompactStarted {
            percentage: u8,
        },
        AutoCompactCompleted {},
        AutoCompactFailed {
            error: String,
        },
        AutoCompactCancelled {},
        AutoContinueCompleted {
            total_tokens: u64,
        },
        ImageCompressed {
            message: String,
        },
        SubagentSpawned {
            subagent_id: String,
        },
        SubagentFinished {
            subagent_id: String,
        },
        #[serde(other)]
        Other,
    }
    #[derive(serde::Deserialize)]
    struct XaiNotif {
        update: XaiUpdate,
    }

    let Ok(xai_notif) = serde_json::from_str::<XaiNotif>(notif.request.params.get()) else {
        return ExtEvent::None;
    };

    match xai_notif.update {
        XaiUpdate::AutoCompactStarted { percentage } => match format {
            OutputFormat::StreamingJson => {
                println!(
                    "{}",
                    serde_json::json!({"type": "auto_compact_started", "percentage": percentage})
                );
            }
            OutputFormat::Plain => {
                eprintln!("Auto-compacting conversation ({percentage}% full)...");
            }
            OutputFormat::Json => {}
        },
        XaiUpdate::AutoCompactCompleted {} => match format {
            OutputFormat::StreamingJson => {
                println!("{}", serde_json::json!({"type": "auto_compact_completed"}));
            }
            OutputFormat::Plain => eprintln!("Conversation compacted."),
            OutputFormat::Json => {}
        },
        XaiUpdate::AutoCompactFailed { error } => match format {
            OutputFormat::StreamingJson => {
                println!(
                    "{}",
                    serde_json::json!({"type": "auto_compact_failed", "error": error})
                );
            }
            OutputFormat::Plain => {
                if error.trim().is_empty() {
                    eprintln!("Auto-compact failed.");
                } else {
                    eprintln!("Auto-compact failed: {error}");
                }
            }
            OutputFormat::Json => {}
        },
        XaiUpdate::AutoCompactCancelled {} => match format {
            OutputFormat::StreamingJson => {
                println!("{}", serde_json::json!({"type": "auto_compact_cancelled"}));
            }
            OutputFormat::Plain => eprintln!("Auto-compact cancelled."),
            OutputFormat::Json => {}
        },
        XaiUpdate::AutoContinueCompleted { total_tokens } => match format {
            OutputFormat::StreamingJson => {
                println!(
                    "{}",
                    serde_json::json!({"type": "auto_continue_completed", "total_tokens": total_tokens})
                );
            }
            OutputFormat::Plain => eprintln!("Resumed after compaction."),
            OutputFormat::Json => {}
        },
        XaiUpdate::ImageCompressed { message } => match format {
            OutputFormat::StreamingJson => {
                println!(
                    "{}",
                    serde_json::json!({"type": "image_compressed", "message": message})
                );
            }
            OutputFormat::Plain => eprintln!("{message}"),
            OutputFormat::Json => {}
        },
        XaiUpdate::SubagentSpawned { subagent_id } => {
            return ExtEvent::SubagentSpawned { subagent_id };
        }
        XaiUpdate::SubagentFinished { subagent_id, .. } => {
            return ExtEvent::SubagentFinished { subagent_id };
        }
        XaiUpdate::Other => {}
    }
    ExtEvent::None
}

#[cfg(test)]
mod tests {
    #[test]
    fn lifecycle_tracking_is_independent_of_wait_flag() {
        let mut pending = std::collections::HashSet::new();
        let mut completed = std::collections::HashSet::new();
        super::track_background_lifecycle(
            super::ExtEvent::TaskBackgrounded {
                task_id: "t1".into(),
                is_monitor: false,
            },
            &mut pending,
            &mut completed,
        );
        super::track_background_lifecycle(
            super::ExtEvent::SubagentSpawned {
                subagent_id: "s1".into(),
            },
            &mut pending,
            &mut completed,
        );
        assert!(pending.contains("t1"));
        assert!(pending.contains("subagent:s1"));

        super::track_background_lifecycle(
            super::ExtEvent::TaskCompleted {
                task_id: "t1".into(),
            },
            &mut pending,
            &mut completed,
        );
        super::track_background_lifecycle(
            super::ExtEvent::SubagentFinished {
                subagent_id: "s1".into(),
            },
            &mut pending,
            &mut completed,
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn completion_before_backgrounded_never_rearms_pending() {
        let mut pending = std::collections::HashSet::new();
        let mut completed = std::collections::HashSet::new();
        super::track_background_lifecycle(
            super::ExtEvent::TaskCompleted {
                task_id: "t1".into(),
            },
            &mut pending,
            &mut completed,
        );
        super::track_background_lifecycle(
            super::ExtEvent::TaskBackgrounded {
                task_id: "t1".into(),
                is_monitor: false,
            },
            &mut pending,
            &mut completed,
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn reap_request_for_task_kills_with_session_scope() {
        let session_id = acp::SessionId::new("sess-1");
        let request = super::reap_request_for_key("task-42", &session_id).unwrap();
        assert_eq!(request.method.as_ref(), "x.ai/task/kill");
        let params: serde_json::Value = serde_json::from_str(request.params.get()).unwrap();
        assert_eq!(params["sessionId"], "sess-1");
        assert_eq!(params["taskId"], "task-42");
    }

    #[test]
    fn reap_request_for_subagent_cancels_with_stripped_id() {
        let session_id = acp::SessionId::new("sess-1");
        let request = super::reap_request_for_key("subagent:sub-7", &session_id).unwrap();
        assert_eq!(request.method.as_ref(), "x.ai/subagent/cancel");
        let params: serde_json::Value = serde_json::from_str(request.params.get()).unwrap();
        assert_eq!(params["subagentId"], "sub-7");
    }

    use super::*;
    use xai_grok_workspace::permission::types::{RuleAction, ToolFilter};

    fn s(v: &str) -> String {
        v.to_owned()
    }

    /// Headless materialization is never chat, regardless of worktree flag —
    /// resume targets stay disk/GCS Build sessions. The pre-sandbox pin flag
    /// must carry through so a pinned target is never re-title-selected.
    #[test]
    fn headless_materialize_ctx_stays_non_chat() {
        use crate::app::session_startup::TitleResolution;
        for has_worktree in [false, true] {
            for pinned in [false, true] {
                let ctx = headless_materialize_ctx(has_worktree, pinned);
                assert!(!ctx.chat_mode);
                assert_eq!(ctx.has_worktree, has_worktree);
                assert_eq!(
                    ctx.title_resolution,
                    if pinned {
                        TitleResolution::PinnedPreSandbox
                    } else {
                        TitleResolution::Allowed
                    }
                );
            }
        }
    }

    #[test]
    fn strict_valid_rules_parse_deny_before_allow() {
        let allow = vec![s("Bash(npm*)")];
        let deny = vec![s("Bash(rm*)"), s("Edit(/etc/**)")];
        let rules = parse_permission_rules_strict(&allow, &deny).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].action, RuleAction::Deny);
        assert!(matches!(rules[0].tool, ToolFilter::Bash));
        assert_eq!(rules[1].action, RuleAction::Deny);
        assert!(matches!(rules[1].tool, ToolFilter::Edit));
        assert_eq!(rules[2].action, RuleAction::Allow);
        assert!(matches!(rules[2].tool, ToolFilter::Bash));
    }

    #[test]
    fn strict_invalid_rule_errors() {
        let result = parse_permission_rules_strict(&[], &[s("EnterWorktree(foo)")]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("--deny"));
        assert!(msg.contains("EnterWorktree"));
    }

    #[test]
    fn strict_reports_all_invalid_rules() {
        let result = parse_permission_rules_strict(
            &[s("BadTool(x)")],
            &[s("EnterWorktree(foo)"), s("Bash(rm*)")],
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("EnterWorktree"),
            "should mention first bad deny"
        );
        assert!(msg.contains("BadTool"), "should mention bad allow");
    }

    #[test]
    fn lenient_skips_invalid_keeps_valid() {
        let allow = vec![s("Bash(npm*)")];
        let deny = vec![s("EnterWorktree(foo)"), s("Bash(rm*)")];
        let rules = parse_permission_rules_lenient(&allow, &deny);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].action, RuleAction::Deny);
        assert_eq!(rules[0].pattern.as_deref(), Some("rm*"));
        assert_eq!(rules[1].action, RuleAction::Allow);
        assert_eq!(rules[1].pattern.as_deref(), Some("npm*"));
    }

    #[test]
    fn empty_inputs_produce_empty_rules() {
        let rules = parse_permission_rules_strict(&[], &[]).unwrap();
        assert!(rules.is_empty());
        let rules = parse_permission_rules_lenient(&[], &[]);
        assert!(rules.is_empty());
    }

    #[test]
    fn domain_mode_web_fetch() {
        let rules = parse_permission_rules_strict(&[], &[s("WebFetch(domain:evil.com)")]).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(matches!(rules[0].tool, ToolFilter::WebFetch));
        assert_eq!(
            rules[0].pattern_mode,
            xai_grok_workspace::permission::types::PatternMode::Domain
        );
        assert_eq!(rules[0].pattern.as_deref(), Some("evil.com"));
    }

    #[test]
    fn bash_colon_wildcard_deny_translates_to_prefix() {
        let rules = parse_permission_rules_strict(&[], &[s("Bash(sed:*)")]).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(matches!(rules[0].tool, ToolFilter::Bash));
        assert_eq!(rules[0].pattern.as_deref(), Some("sed"));
    }

    #[test]
    fn structured_output_without_meta_errors_never_parses_text() {
        // No `_meta` structured output (e.g. max-turns/cancel): emit a clean
        // error, never an unvalidated parse of the raw text buffer.
        let mut emitter = HeadlessEmitter::new(OutputFormat::Json, true);
        emitter.text_buffer = r#"{"name":"alice","age":30}"#.into();
        emitter.set_structured_output_from_meta(serde_json::json!({}).as_object());
        let result = emitter.build_json_result("EndTurn", "sess-1", "req-1");
        assert!(result["structuredOutput"].is_null());
        assert_eq!(
            result["structuredOutputError"],
            "model did not produce structured output"
        );
    }

    #[test]
    fn structured_output_from_meta_wins_over_text_buffer() {
        // The agent's validated output (from `_meta`) must override accumulated
        // prose (the multi-round corruption bug).
        let mut emitter = HeadlessEmitter::new(OutputFormat::Json, true);
        emitter.text_buffer = "thinking out loud...".into();
        emitter.set_structured_output_from_meta(
            serde_json::json!({"structuredOutput": {"name": "carol"}}).as_object(),
        );
        let result = emitter.build_json_result("EndTurn", "sess-1", "req-1");
        assert_eq!(result["structuredOutput"]["name"], "carol");
        assert!(result.get("structuredOutputError").is_none());

        let mut emitter = HeadlessEmitter::new(OutputFormat::Json, true);
        emitter.set_structured_output_from_meta(
            serde_json::json!({
                "structuredOutputError": "output does not match the required schema"
            })
            .as_object(),
        );
        let result = emitter.build_json_result("EndTurn", "sess-1", "req-1");
        assert!(result["structuredOutput"].is_null());
        assert_eq!(
            result["structuredOutputError"],
            "output does not match the required schema"
        );
    }

    #[test]
    fn streaming_json_structured_output_emits_from_meta() {
        let mut emitter = HeadlessEmitter::new(OutputFormat::StreamingJson, true);
        emitter.on_text_chunk(r#"{"name":"#);
        emitter.on_text_chunk(r#""bob"}"#);
        assert_eq!(emitter.text_buffer, r#"{"name":"bob"}"#);

        // structuredOutput comes from the prompt-response `_meta`, not the buffer.
        emitter.set_structured_output_from_meta(
            serde_json::json!({"structuredOutput": {"name": "bob"}}).as_object(),
        );
        let mut target = serde_json::json!({});
        emitter.attach_structured_output(&mut target);
        assert_eq!(target["structuredOutput"]["name"], "bob");
        assert!(target.get("structuredOutputError").is_none());
    }

    #[test]
    fn parse_json_schema_rejects_non_objects_and_invalid_json() {
        assert!(super::parse_json_schema(r#"{"type":"object"}"#).is_ok());
        assert!(
            super::parse_json_schema(r#"[1,2,3]"#)
                .unwrap_err()
                .to_string()
                .contains("must be a JSON object")
        );
        assert!(
            super::parse_json_schema(r#"{not json"#)
                .unwrap_err()
                .to_string()
                .contains("invalid JSON")
        );
    }

    #[test]
    fn files_changed_json_counts_and_lists_paths() {
        let mut emitter = HeadlessEmitter::new(OutputFormat::StreamingJson, false);
        emitter.files_changed.insert("src/a.rs".into());
        emitter.files_changed.insert("src/b.rs".into());
        let v = emitter.files_changed_json();
        assert_eq!(v["count"], 2);
        assert_eq!(v["truncated"], false);
        let paths = v["paths"].as_array().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p == "src/a.rs"));
        assert!(paths.iter().any(|p| p == "src/b.rs"));
    }

    #[test]
    fn files_changed_json_empty_when_no_edits() {
        let emitter = HeadlessEmitter::new(OutputFormat::StreamingJson, false);
        let v = emitter.files_changed_json();
        assert_eq!(v["count"], 0);
        assert_eq!(v["truncated"], false);
        assert!(v["paths"].as_array().unwrap().is_empty());
    }

    #[test]
    fn files_changed_json_truncates_path_list() {
        let mut emitter = HeadlessEmitter::new(OutputFormat::StreamingJson, false);
        for i in 0..(FILES_CHANGED_MAX_PATHS + 5) {
            emitter.files_changed.insert(format!("f{i}.rs"));
        }
        let v = emitter.files_changed_json();
        assert_eq!(v["count"], FILES_CHANGED_MAX_PATHS + 5);
        assert_eq!(v["truncated"], true);
        assert_eq!(
            v["paths"].as_array().unwrap().len(),
            FILES_CHANGED_MAX_PATHS
        );
    }

    #[test]
    fn require_changes_flag_detects_empty_set() {
        // Mirrors the headless exit gate: require_changes && no edit paths.
        let emitter = HeadlessEmitter::new(OutputFormat::StreamingJson, false);
        assert!(emitter.files_changed.is_empty());
        let require_changes = true;
        assert!(require_changes && emitter.files_changed.is_empty());
        let mut emitter2 = HeadlessEmitter::new(OutputFormat::StreamingJson, false);
        emitter2.files_changed.insert("x.rs".into());
        assert!(!(require_changes && emitter2.files_changed.is_empty()));
    }

    fn make_ext_notif(
        method: &str,
        update: serde_json::Value,
    ) -> xai_acp_lib::AcpArgsBox<acp::ExtNotification> {
        let payload = serde_json::json!({
            "sessionId": "sess-1",
            "update": update,
        });
        let raw = serde_json::value::to_raw_value(&payload).unwrap();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        xai_acp_lib::AcpArgs {
            request: acp::ExtNotification::new(method, raw.into()),
            response_tx: tx,
        }
        .boxed()
    }

    #[test]
    fn headless_task_backgrounded_parses_task_id() {
        // `make_ext_notif` wraps the arg under `update`, so pass
        // the inner update object (matching the real `x.ai/task_backgrounded`
        // wire shape: `{ "update": { "sessionUpdate": ..., "task_id": ... } }`).
        let notif = make_ext_notif(
            "x.ai/task_backgrounded",
            serde_json::json!({
                "sessionUpdate": "task_backgrounded",
                "task_id": "task-abc",
            }),
        );
        assert!(matches!(
            handle_ext_notification(&notif, OutputFormat::Plain),
            ExtEvent::TaskBackgrounded { task_id, is_monitor: false } if task_id == "task-abc"
        ));
    }

    #[test]
    fn headless_task_backgrounded_with_monitor_description_is_monitor() {
        let notif = make_ext_notif(
            "x.ai/task_backgrounded",
            serde_json::json!({
                "sessionUpdate": "task_backgrounded",
                "task_id": "mon-1",
                "monitor_description": "watching logs",
            }),
        );
        assert!(matches!(
            handle_ext_notification(&notif, OutputFormat::Plain),
            ExtEvent::TaskBackgrounded { task_id, is_monitor: true } if task_id == "mon-1"
        ));
    }

    #[test]
    fn headless_task_completed_parses_task_id() {
        // `task_completed` nests the id under `task_snapshot`. The
        // internally-tagged `rename_all = "snake_case"` renames only the
        // `sessionUpdate` tag, so `task_id` / `task_snapshot` stay snake_case;
        // this test guards against a future `rename_all = "camelCase"` on
        // `TaskSnapshot` silently turning waiting into a no-op.
        let notif = make_ext_notif(
            "x.ai/task_completed",
            serde_json::json!({
                "sessionUpdate": "task_completed",
                "task_snapshot": { "task_id": "task-abc" }
            }),
        );
        assert!(matches!(
            handle_ext_notification(&notif, OutputFormat::Plain),
            ExtEvent::TaskCompleted { task_id } if task_id == "task-abc"
        ));
    }

    #[test]
    fn headless_subagent_spawned_and_finished_parse() {
        let spawned = make_ext_notif(
            "x.ai/session_notification",
            serde_json::json!({
                "sessionUpdate": "subagent_spawned",
                "subagent_id": "sub-1",
                "parent_session_id": "p",
                "child_session_id": "c",
                "subagent_type": "explore",
                "description": "test"
            }),
        );
        assert!(matches!(
            handle_ext_notification(&spawned, OutputFormat::Plain),
            ExtEvent::SubagentSpawned { subagent_id } if subagent_id == "sub-1"
        ));
        let finished = make_ext_notif(
            "x.ai/session_notification",
            serde_json::json!({
                "sessionUpdate": "subagent_finished",
                "subagent_id": "sub-1",
                "child_session_id": "c",
                "status": "completed",
                "tool_calls": 0,
                "turns": 1,
                "duration_ms": 5
            }),
        );
        assert!(matches!(
            handle_ext_notification(&finished, OutputFormat::Plain),
            ExtEvent::SubagentFinished { subagent_id } if subagent_id == "sub-1"
        ));
    }

    #[test]
    fn headless_session_update_unknown_method_is_none() {
        let payload = serde_json::json!({
            "sessionId": "sess-1",
            "update": {
                "sessionUpdate": "subagent_spawned",
                "subagent_id": "sub-1"
            }
        });
        let raw = serde_json::value::to_raw_value(&payload).unwrap();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let notif = xai_acp_lib::AcpArgs {
            request: acp::ExtNotification::new("x.ai/other", raw.into()),
            response_tx: tx,
        }
        .boxed();
        assert!(matches!(
            handle_ext_notification(&notif, OutputFormat::Plain),
            ExtEvent::None
        ));
    }

    // ── HYPER-1: question shape + auto-continue bound ────────────────────

    #[test]
    fn looks_like_user_question_detects_trailing_question() {
        assert!(looks_like_user_question("Want to try it?"));
        assert!(looks_like_user_question(
            "I can put together mockups.\n\nWant to try it?"
        ));
        assert!(looks_like_user_question("Proceed with the default?\n"));
        // Rhetorical mid-body `?` with a real answer after — not a
        // question-ending turn.
        assert!(!looks_like_user_question(
            "Is the bug in foo? Yes — I fixed it in src/main.rs and re-ran tests."
        ));
        assert!(!looks_like_user_question("I edited src/main.rs and ran tests."));
        assert!(!looks_like_user_question(""));
        assert!(!looks_like_user_question("   \n  "));
    }

    #[test]
    fn looks_like_user_question_field_incident_shape() {
        // The real field text: question mid-body, parenthetical after.
        // Detector must catch this — it is the exact HYPER-1 failure.
        let field = "Some of what we're working on might be easier to explain if I can show \
it to you in a web browser. I can put together mockups, diagrams, comparisons, \
and other visuals as we go. This feature is still new and can be token-intensive. \
Want to try it? (Requires opening a local URL)";
        assert!(
            looks_like_user_question(field),
            "field-incident browser opt-in must count as a user question"
        );
    }

    #[test]
    fn should_auto_continue_only_once_on_question_end_turn() {
        let q = "Want to try the browser feature?";
        assert!(should_auto_continue_headless_question(
            "EndTurn", 0, q, false, false
        ));
        // Bound: already continued → never again.
        assert!(!should_auto_continue_headless_question(
            "EndTurn", 0, q, false, true
        ));
        // Opt-out.
        assert!(!should_auto_continue_headless_question(
            "EndTurn", 0, q, true, false
        ));
        // Did work.
        assert!(!should_auto_continue_headless_question(
            "EndTurn", 1, q, false, false
        ));
        // Not a normal end.
        assert!(!should_auto_continue_headless_question(
            "Cancelled", 0, q, false, false
        ));
        // Not a question.
        assert!(!should_auto_continue_headless_question(
            "EndTurn", 0, "Done.", false, false
        ));
    }

    #[test]
    fn build_headless_init_sets_non_interactive_and_opt_out() {
        let req = build_headless_init_request(Some("be brief"), None, false);
        let meta = req.meta.as_ref().expect("meta");
        assert_eq!(meta["startupHints"]["nonInteractive"], true);
        assert!(meta.get("allowInteractiveQuestions").is_none());
        assert_eq!(meta["rules"], "be brief");

        let req2 = build_headless_init_request(None, None, true);
        let meta2 = req2.meta.as_ref().expect("meta");
        assert_eq!(meta2["allowInteractiveQuestions"], true);
    }
}
