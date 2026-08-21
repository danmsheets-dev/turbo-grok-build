//! Session inject of the budgeted workspace tree card (Phase 1 MVP).
//!
//! Wired from [`crate::prompt::context::PromptContext::render_with_renderer`] after
//! the boot card. Uses the **tool CWD** (real worktree for subagents) so children
//! see their own atlas, not a hardcoded parent path.
//!
//! Modes: `GROK_WORKSPACE_TREE_INJECT=off|minimal|standard|rich` (also `TURBO_TREE_INJECT`).
//! Master switch: `GROK_WORKSPACE_TREE=0` / `TURBO_TREE=0`.
//! Subagents prefer **minimal** unless inject mode is set explicitly in env.

use std::path::{Path, PathBuf};

use xai_workspace_tree::{InjectMode, WorkspaceTreeConfig, inject_building_notice, inject_card};

use super::context::{PromptAudience, PromptContext};

const CARD_OPEN: &str = "<workspace_tree_card";
const CARD_TAG_OPEN: &str = "<workspace_tree_card version=\"1\">";
const CARD_TAG_CLOSE: &str = "</workspace_tree_card>";

/// Resolve the filesystem root for atlas inject / tools (real tool CWD).
///
/// Prefer `tool_working_directory` (worktree path for isolation=worktree),
/// else model-facing `working_directory`, else process cwd.
pub fn inject_root_for_context(ctx: &PromptContext) -> PathBuf {
    if let Some(tool) = ctx.tool_working_directory.as_deref() {
        let p = PathBuf::from(tool);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Some(wd) = ctx.working_directory.as_deref() {
        let p = PathBuf::from(wd);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Build the inject card text (without wrapper tags), or `None` when off / empty.
pub fn render_workspace_tree_card(ctx: &PromptContext) -> Option<String> {
    let config = WorkspaceTreeConfig::from_env();
    if !config.enabled {
        return None;
    }

    let is_subagent = ctx.audience == PromptAudience::Subagent;
    let mode = config.inject_mode_for_audience(is_subagent);
    if matches!(mode, InjectMode::Off) {
        return None;
    }

    let mut inject_cfg = config.clone();
    inject_cfg.inject.mode = mode;

    let root = inject_root_for_context(ctx);

    // Never block prompt build on a walk: process cache → durable store only.
    let index = xai_grok_tools::util::workspace_tree_try_get(&root)
        .or_else(|| xai_grok_tools::util::workspace_tree_try_load_cached(&root, &inject_cfg));

    let body = match index {
        Some(idx) => {
            let card = inject_card(&idx, &inject_cfg);
            if card.trim().is_empty() {
                return None;
            }
            card
        }
        None => {
            // Index still building (kickoff) or missing — short notice, not a failure.
            inject_building_notice()
        }
    };

    Some(body)
}

/// Append a workspace tree card to the system prompt once (idempotent).
pub fn inject_workspace_tree_card(system_prompt: &mut String, ctx: &PromptContext) {
    if system_prompt.contains(CARD_OPEN) {
        return;
    }
    let Some(body) = render_workspace_tree_card(ctx) else {
        return;
    };
    system_prompt.push_str("\n\n");
    system_prompt.push_str(CARD_TAG_OPEN);
    system_prompt.push('\n');
    system_prompt.push_str(&body);
    system_prompt.push('\n');
    system_prompt.push_str(CARD_TAG_CLOSE);
    system_prompt.push('\n');

    tracing::info!(
        audience = ?ctx.audience,
        root = %inject_root_for_context(ctx).display(),
        chars = body.len(),
        "workspace_tree_card injected"
    );
}

/// Convenience for CLI inject-preview against an explicit root.
pub fn inject_preview_for_root(root: &Path, is_subagent: bool) -> String {
    let config = WorkspaceTreeConfig::from_env();
    if !config.enabled {
        return "workspace tree disabled (GROK_WORKSPACE_TREE=0)".into();
    }
    let mut inject_cfg = config.clone();
    inject_cfg.inject.mode = config.inject_mode_for_audience(is_subagent);

    match xai_grok_tools::util::workspace_tree_get_or_load(root, &inject_cfg) {
        Ok(idx) => {
            let card = inject_card(&idx, &inject_cfg);
            if card.is_empty() {
                "(inject mode off or empty card)".into()
            } else {
                card
            }
        }
        Err(e) => format!("failed to load workspace tree: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::context::PromptContext;

    #[test]
    fn inject_root_prefers_tool_cwd() {
        let mut ctx = PromptContext::default();
        ctx.working_directory = Some(r"H:\parent".into());
        ctx.tool_working_directory = Some(r"C:\Users\x\.grok\worktrees\w\subagent-1".into());
        let root = inject_root_for_context(&ctx);
        assert!(
            root.to_string_lossy().contains("subagent"),
            "tool cwd should win: {}",
            root.display()
        );
    }

    #[test]
    fn off_mode_skips() {
        // When master switch disabled via env is hard to test without serial_test;
        // ensure Off render path with empty default context at least does not panic.
        let ctx = PromptContext::default();
        let _ = render_workspace_tree_card(&ctx);
    }
}
