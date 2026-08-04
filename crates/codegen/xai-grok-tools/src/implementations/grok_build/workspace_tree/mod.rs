//! `workspace_tree` â€' query the workspace directory atlas (summary/list/search/stats/refresh).
//!
//! Wraps `xai_workspace_tree`. Never dumps the full tree into the model context.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::{resolve_cwd, shared_resources};
use crate::util::workspace_tree_cache;

pub const WORKSPACE_TREE_TOOL_NAME: &str = "workspace_tree";

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceTreeInput {
    #[schemars(
        description = "Action: summary | list | search | stats | refresh. Default summary."
    )]
    #[serde(default = "default_action")]
    pub action: String,

    #[serde(default)]
    #[schemars(description = "Relative path under workspace root (list action).")]
    pub path: Option<String>,

    #[serde(default)]
    #[schemars(description = "Basename / path substring query (search action).")]
    pub query: Option<String>,

    #[serde(default)]
    #[schemars(description = "List depth (default 1). Collapsed dirs never expand.")]
    pub depth: Option<u32>,

    #[serde(default)]
    #[schemars(description = "Max entries to return (default 50, hard cap 200).")]
    pub limit: Option<usize>,
}

fn default_action() -> String {
    "summary".to_string()
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceTreeOutput {
    /// Pre-formatted model-facing text (never a full tree dump).
    pub message: String,
    /// Action that was executed.
    pub action: String,
    /// Freshness state string from the index.
    pub freshness: String,
    /// Workspace root (canonical).
    pub root: String,
}

impl xai_tool_runtime::ToolOutput for WorkspaceTreeOutput {
    fn model_output(&self) -> Vec<xai_tool_runtime::ContentBlock> {
        vec![xai_tool_runtime::ContentBlock::Text {
            text: self.message.clone(),
        }]
    }
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct WorkspaceTreeTool;

impl crate::types::tool_metadata::ToolMetadata for WorkspaceTreeTool {
    fn kind(&self) -> ToolKind {
        // Dedicated atlas tool â€' keep List reserved for live list_dir.
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Query the workspace directory atlas (not a live disk walk of everything).

Use this instead of inventing folder paths. Actions:
- `summary` â€' budgeted top-level map + stats + freshness (default)
- `list` â€' children under `path` (optional `depth`, `limit`)
- `search` â€' basename / path substring hits for `query`
- `stats` â€' file/dir counts and build timing
- `refresh` â€' rebuild the durable index then return summary

Do **not** dump or request the full tree. Prefer `resolve_path` for unique basenames.
Index is loaded on first use (or via session kickoff); `refresh` forces rebuild."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        // Durable store under ~/.grok/workspace-trees; not the user workspace.
        true
    }
}

impl xai_tool_runtime::Tool for WorkspaceTreeTool {
    type Args = WorkspaceTreeInput;
    type Output = WorkspaceTreeOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(WORKSPACE_TREE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            WORKSPACE_TREE_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    #[tracing::instrument(
        name = "tool.workspace_tree",
        skip_all,
        fields(action = %input.action)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: WorkspaceTreeInput,
    ) -> Result<WorkspaceTreeOutput, xai_tool_runtime::ToolError> {
        // Prefer per-call Cwd override, else session Cwd in SharedResources
        // (subagents often have Resources Cwd but no extension override).
        let resources = shared_resources(&ctx)?;
        workspace_tree_cache::ensure_indexing_allowed(&resources).await?;
        let cwd = resolve_cwd(&ctx, &resources).await?;

        let action = normalize_action(&input.action);
        let limit = input.limit.unwrap_or(50).clamp(1, 200);
        let depth = input.depth.unwrap_or(1).max(1);
        let path = input.path.clone().unwrap_or_default();
        let query = input.query.clone().unwrap_or_default();

        let result = tokio::task::spawn_blocking(move || {
            run_action(&cwd, &action, &path, &query, depth, limit)
        })
        .await
        .map_err(|e| {
            xai_tool_runtime::ToolError::custom(
                "workspace_tree_join",
                format!("workspace_tree task failed: {e}"),
            )
        })??;

        Ok(result)
    }
}

fn normalize_action(raw: &str) -> String {
    let a = raw.trim().to_ascii_lowercase();
    match a.as_str() {
        "summary" | "list" | "search" | "stats" | "refresh" => a,
        "" => "summary".to_string(),
        // Design also mentions "subtree" â€' map to list with deeper default later.
        "subtree" => "list".to_string(),
        other => other.to_string(),
    }
}

fn run_action(
    cwd: &std::path::Path,
    action: &str,
    path: &str,
    query: &str,
    depth: u32,
    limit: usize,
) -> Result<WorkspaceTreeOutput, xai_tool_runtime::ToolError> {
    use xai_workspace_tree::{list, search, summary, WorkspaceTreeConfig};

    let config = WorkspaceTreeConfig::from_env();
    if !config.enabled {
        return Err(xai_tool_runtime::ToolError::custom(
            "workspace_tree_disabled",
            "workspace tree is disabled (GROK_WORKSPACE_TREE=0 / TURBO_TREE=0)",
        ));
    }

    let index = if action == "refresh" {
        workspace_tree_cache::refresh(cwd, &config)
    } else {
        workspace_tree_cache::get_or_load(cwd, &config)
    }
    .map_err(|e| xai_tool_runtime::ToolError::custom("workspace_tree_load", e))?;

    let freshness = format!("{:?}", index.meta.freshness.state).to_ascii_lowercase();
    let root = index.meta.canonical_root.clone();

    let message = match action {
        "summary" => {
            let s = summary(&index, limit.min(48));
            format_summary(&s)
        }
        "stats" => format_stats(&index),
        "list" => {
            let result = list(&index, path, depth, limit).map_err(|e| {
                xai_tool_runtime::ToolError::custom("workspace_tree_list", e.to_string())
            })?;
            format_list(&result)
        }
        "search" => {
            if query.trim().is_empty() {
                return Err(xai_tool_runtime::ToolError::custom(
                    "workspace_tree_search",
                    "search action requires non-empty `query`",
                ));
            }
            let result = search(&index, query, limit);
            format_search(&result)
        }
        "refresh" => {
            let s = summary(&index, limit.min(48));
            format!("Index refreshed.\n\n{}", format_summary(&s))
        }
        other => {
            return Err(xai_tool_runtime::ToolError::custom(
                "workspace_tree_action",
                format!(
                    "unknown action `{other}`; expected summary | list | search | stats | refresh"
                ),
            ));
        }
    };

    Ok(WorkspaceTreeOutput {
        message,
        action: action.to_string(),
        freshness,
        root,
    })
}

fn format_summary(s: &xai_workspace_tree::SummaryResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Workspace tree summary (freshness={:?}, {} files, {} dirs, build {:.1}s)",
        s.meta.freshness.state,
        s.stats.files,
        s.stats.dirs,
        s.build_duration_ms as f64 / 1000.0
    ));
    lines.push(format!("Root: {}", s.meta.root));
    if !s.workspace_profile.is_empty() {
        lines.push(format!("Stack: {}", s.workspace_profile.join(" | ")));
    }
    if s.meta.truncated {
        lines.push("Note: walk was truncated (budget caps).".to_string());
    }
    lines.push(String::new());
    lines.push("Top-level:".to_string());
    for e in &s.top_level {
        let kind = format!("{:?}", e.kind).to_ascii_lowercase();
        let count = e
            .file_count
            .map(|c| format!(" ({c} files)"))
            .unwrap_or_default();
        lines.push(format!("  {}/  [{kind}]{count}", e.name));
    }
    lines.push(String::new());
    lines.push("Tip: resolve_path for unique basenames; list/search for exploration.".to_string());
    lines.join("\n")
}

fn format_stats(index: &xai_workspace_tree::TreeIndex) -> String {
    let m = &index.meta;
    format!(
        "Workspace tree stats\n\
         root: {}\n\
         workspace_id: {}\n\
         freshness: {:?}\n\
         files: {}\n\
         dirs: {}\n\
         collapsed_dirs: {}\n\
         ignored_dirs: {}\n\
         truncated: {}\n\
         build_mode: {}\n\
         build_duration_ms: {}\n\
         profile: {}\n",
        m.canonical_root,
        m.workspace_id,
        m.freshness.state,
        m.stats.files,
        m.stats.dirs,
        m.stats.collapsed_dirs,
        m.stats.ignored_dirs,
        m.stats.truncated,
        m.build.mode,
        m.build.duration_ms,
        m.workspace_profile.join(", ")
    )
}

fn format_list(r: &xai_workspace_tree::ListResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "List `{}` (freshness={:?}, {} entries)",
        if r.path.is_empty() { "." } else { &r.path },
        r.meta.freshness.state,
        r.entries.len()
    ));
    for e in &r.entries {
        let kind = format!("{:?}", e.kind).to_ascii_lowercase();
        let extra = match (e.file_count, e.ext.as_deref()) {
            (Some(c), _) => format!(" files={c}"),
            (None, Some(ext)) => format!(" .{ext}"),
            _ => String::new(),
        };
        lines.push(format!("  {}  [{kind}]{extra}", e.rel_path));
    }
    if r.entries.is_empty() {
        lines.push("  (empty or collapsed)".to_string());
    }
    lines.join("\n")
}

fn format_search(r: &xai_workspace_tree::SearchResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Search `{}` (freshness={:?}, {} hits)",
        r.query,
        r.meta.freshness.state,
        r.hits.len()
    ));
    for h in &r.hits {
        lines.push(format!("  {}  (score {:.2})", h.rel_path, h.score));
    }
    if r.hits.is_empty() {
        lines.push("  (no hits)".to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::Cwd;
    use crate::types::tool_metadata::test_ctx;
    use crate::util::workspace_tree_cache;
    use tempfile::TempDir;
    use xai_tool_runtime::Tool;
    use xai_workspace_tree::{build_and_save, WorkspaceTreeConfig};

    #[test]
    fn summary_action_formats() {
        workspace_tree_cache::clear_cache_for_tests();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main(){}").unwrap();
        let store = tmp.path().join("store");
        let mut cfg = WorkspaceTreeConfig::default();
        cfg.store_dir = Some(store.clone());
        // Pre-seed via default store path used by tools (default ~/.grok) is
        // awkward in unit tests; exercise format helpers via public crate API.
        let index = build_and_save(&root, &cfg).unwrap();
        let s = xai_workspace_tree::summary(&index, 10);
        let text = format_summary(&s);
        assert!(text.contains("Top-level"), "{text}");
        assert!(text.contains("src"), "{text}");
    }

    #[test]
    fn normalize_maps_subtree_to_list() {
        assert_eq!(normalize_action("subtree"), "list");
        assert_eq!(normalize_action("SUMMARY"), "summary");
    }

    /// Subagents often have session `Cwd` only in SharedResources, not as a
    /// `xai_tool_runtime::Cwd` extension override. Tools must fall back.
    #[tokio::test]
    async fn summary_uses_resources_cwd_without_extension_override() {
        workspace_tree_cache::clear_cache_for_tests();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main(){}").unwrap();

        let mut resources = crate::types::resources::Resources::new();
        resources.insert(Cwd(root.clone()));
        // Intentionally do NOT insert xai_tool_runtime::Cwd on the context.
        let ctx = test_ctx(resources.into_shared());
        let out = WorkspaceTreeTool
            .run(
                ctx,
                WorkspaceTreeInput {
                    action: "summary".into(),
                    path: None,
                    query: None,
                    depth: None,
                    limit: Some(20),
                },
            )
            .await
            .expect("workspace_tree should resolve Cwd from SharedResources");
        assert!(
            out.message.contains("Top-level") || out.message.contains("src"),
            "unexpected summary: {}",
            out.message
        );
    }
}

