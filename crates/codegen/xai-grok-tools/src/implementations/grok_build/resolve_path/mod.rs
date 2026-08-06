//! `resolve_path` â€” map a free-form name / guessed path to ranked real paths.
//!
//! Wraps `xai_workspace_tree::resolve_path`. Prefer this before inventing folders.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::{resolve_cwd, shared_resources};
use crate::util::workspace_tree_cache;

pub const RESOLVE_PATH_TOOL_NAME: &str = "resolve_path";

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePathInput {
    #[schemars(
        description = "Basename, stem, or guessed path to resolve (e.g. ship_roster or scripts/ship/ship_roster.gd)."
    )]
    pub name: String,

    #[serde(default)]
    #[schemars(
        description = "Optional path hint to bias ranking (failed read path, guessed folder)."
    )]
    pub hint_path: Option<String>,

    #[serde(default)]
    #[schemars(description = "Max candidates (default 8, hard cap 32).")]
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePathHitOut {
    pub rel_path: String,
    pub name: String,
    pub score: f32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolvePathOutput {
    pub message: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best: Option<String>,
    pub matches: Vec<ResolvePathHitOut>,
    pub freshness: String,
    pub root: String,
}

impl xai_tool_runtime::ToolOutput for ResolvePathOutput {
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
pub struct ResolvePathTool;

impl crate::types::tool_metadata::ToolMetadata for ResolvePathTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        r#"Resolve a free-form file/dir name or guessed path to ranked real workspace paths.

Use when you are unsure of the exact path (e.g. `ship_roster` might live under
`scripts/core/` not `scripts/ship/`). Prefer this over inventing folders or
serial list_dir walks.

Returns ranked matches with scores; pick a path and then `read_file` it.
Does not auto-read. Uses the workspace tree atlas (load-on-first-use)."#
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for ResolvePathTool {
    type Args = ResolvePathInput;
    type Output = ResolvePathOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(RESOLVE_PATH_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            RESOLVE_PATH_TOOL_NAME,
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
        name = "tool.resolve_path",
        skip_all,
        fields(name = %input.name)
    )]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: ResolvePathInput,
    ) -> Result<ResolvePathOutput, xai_tool_runtime::ToolError> {
        // Prefer per-call Cwd override, else session Cwd in SharedResources
        // (subagents often have Resources Cwd but no extension override).
        let resources = shared_resources(&ctx)?;
        crate::util::workspace_tree_cache::ensure_indexing_allowed(&resources).await?;
        let cwd = resolve_cwd(&ctx, &resources).await?;

        let name = input.name.clone();
        let hint = input.hint_path.clone();
        let limit = input.limit.unwrap_or(8).clamp(1, 32);

        let result = tokio::task::spawn_blocking(move || run_resolve(&cwd, &name, hint.as_deref(), limit))
            .await
            .map_err(|e| {
                xai_tool_runtime::ToolError::custom(
                    "resolve_path_join",
                    format!("resolve_path task failed: {e}"),
                )
            })??;

        Ok(result)
    }
}

fn run_resolve(
    cwd: &std::path::Path,
    name: &str,
    hint_path: Option<&str>,
    limit: usize,
) -> Result<ResolvePathOutput, xai_tool_runtime::ToolError> {
    use xai_workspace_tree::{resolve_path, WorkspaceTreeConfig};

    let config = WorkspaceTreeConfig::from_env();
    if !config.enabled {
        return Err(xai_tool_runtime::ToolError::custom(
            "workspace_tree_disabled",
            "workspace tree is disabled (GROK_WORKSPACE_TREE=0 / TURBO_TREE=0)",
        ));
    }

    let index = workspace_tree_cache::get_or_load(cwd, &config)
        .map_err(|e| xai_tool_runtime::ToolError::custom("workspace_tree_load", e))?;

    let result = resolve_path(&index, name, hint_path, limit);
    let freshness = format!("{:?}", result.meta.freshness.state).to_ascii_lowercase();
    let root = result.meta.root.clone();

    let matches: Vec<ResolvePathHitOut> = result
        .hits
        .iter()
        .map(|h| ResolvePathHitOut {
            rel_path: h.rel_path.clone(),
            name: h.name.clone(),
            score: h.score,
            reason: h.reason.clone(),
        })
        .collect();

    let best = matches.first().map(|m| m.rel_path.clone());
    let message = format_message(name, hint_path, &matches, &freshness);

    Ok(ResolvePathOutput {
        message,
        name: name.to_string(),
        best,
        matches,
        freshness,
        root,
    })
}

fn format_message(
    name: &str,
    hint_path: Option<&str>,
    matches: &[ResolvePathHitOut],
    freshness: &str,
) -> String {
    let mut lines = Vec::new();
    match hint_path {
        Some(h) => lines.push(format!(
            "resolve_path `{name}` (hint=`{h}`, freshness={freshness})"
        )),
        None => lines.push(format!("resolve_path `{name}` (freshness={freshness})")),
    }
    if matches.is_empty() {
        lines.push("No matches in workspace tree atlas.".to_string());
        lines.push("Try workspace_tree action=search or list_dir on a known root.".to_string());
    } else {
        if let Some(best) = matches.first() {
            lines.push(format!("Best: {}  (score {:.2}, {})", best.rel_path, best.score, best.reason));
        }
        lines.push("Matches:".to_string());
        for (i, m) in matches.iter().enumerate() {
            lines.push(format!(
                "  {}. {}  (score {:.2}, {})",
                i + 1,
                m.rel_path,
                m.score,
                m.reason
            ));
        }
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

    #[test]
    fn format_includes_best_and_matches() {
        let matches = vec![
            ResolvePathHitOut {
                rel_path: "scripts/core/ship_roster.gd".into(),
                name: "ship_roster.gd".into(),
                score: 0.96,
                reason: "stem_match".into(),
            },
            ResolvePathHitOut {
                rel_path: "scripts/core/ship_roster.gd.uid".into(),
                name: "ship_roster.gd.uid".into(),
                score: 0.55,
                reason: "substring".into(),
            },
        ];
        let msg = format_message("ship_roster", Some("scripts/ship/"), &matches, "fresh");
        assert!(msg.contains("Best: scripts/core/ship_roster.gd"), "{msg}");
        assert!(msg.contains("1. scripts/core/ship_roster.gd"), "{msg}");
    }

    /// Same failure mode as live RC12 Q&A: subagent has Resources Cwd only.
    #[tokio::test]
    async fn resolve_uses_resources_cwd_without_extension_override() {
        workspace_tree_cache::clear_cache_for_tests();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/boot_card.rs"), b"// probe").unwrap();

        let mut resources = crate::types::resources::Resources::new();
        resources.insert(Cwd(root.clone()));
        let ctx = test_ctx(resources.into_shared());
        let out = ResolvePathTool
            .run(
                ctx,
                ResolvePathInput {
                    name: "boot_card.rs".into(),
                    hint_path: None,
                    limit: Some(8),
                },
            )
            .await
            .expect("resolve_path should resolve Cwd from SharedResources");
        assert!(
            out.best
                .as_deref()
                .is_some_and(|p| p.contains("boot_card.rs")),
            "unexpected resolve output: {:?}",
            out
        );
    }
}

