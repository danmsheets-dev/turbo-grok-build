//! Budgeted markdown inject card for session start.

use crate::config::{InjectMode, WorkspaceTreeConfig};
use crate::model::{NodeKind, TreeIndex, TreeNode};
use crate::query::summary;

/// Short notice when the atlas is still building (index not ready yet).
///
/// Used at session start so agents know tools will become useful soon without
/// blocking boot on a full walk.
pub fn inject_building_notice() -> String {
    "## Workspace tree (building...)\n\
     Index not ready yet. Tools `workspace_tree` / `resolve_path` load on first use.\n\
     Tip: prefer `resolve_path` over inventing folders once the atlas is warm."
        .to_string()
}

/// Honest notice when inject mode is off (not the same as 'building').
pub fn inject_disabled_notice() -> String {
    "## Workspace tree (inject off)\n\
     Inject mode is off. Tools `workspace_tree` / `resolve_path` still work."
        .to_string()
}

/// Render a budgeted workspace tree card for agent injection.
///
/// Respects `config.inject.mode` and truncates to `max_tokens * 4` characters.
pub fn inject_card(index: &TreeIndex, config: &WorkspaceTreeConfig) -> String {
    if matches!(config.inject.mode, InjectMode::Off) {
        return String::new();
    }

    let max_chars = config.inject.max_chars();
    let top_n = config.inject.max_top_dirs as usize;
    let summ = summary(index, top_n);

    let freshness = format!("{:?}", index.meta.freshness.state).to_ascii_lowercase();
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!(
        "## Workspace tree ({freshness} | {:.1}s | {} files)",
        summ.build_duration_ms as f64 / 1000.0,
        summ.stats.files
    ));
    lines.push(String::new());
    lines.push(format!("Root: {}", index.meta.canonical_root));

    if !summ.workspace_profile.is_empty() {
        lines.push(format!("Stack: {}", summ.workspace_profile.join(" | ")));
    }

    if index.meta.git.present {
        let branch = index
            .meta
            .git
            .branch
            .as_deref()
            .unwrap_or("?");
        let head = index
            .meta
            .git
            .head
            .as_deref()
            .map(|h| {
                if h.len() > 8 {
                    &h[..8]
                } else {
                    h
                }
            })
            .unwrap_or("?");
        lines.push(format!("Git: {branch}@{head}"));
    }

    match config.inject.mode {
        InjectMode::Off => return String::new(),
        InjectMode::Minimal => {
            lines.push(String::new());
            lines.push("Top-level:".to_string());
            for e in &summ.top_level {
                let kind = match e.kind {
                    NodeKind::Dir => "dir",
                    NodeKind::CollapsedDir => "collapsed",
                    NodeKind::File => "file",
                    NodeKind::Symlink => "link",
                };
                lines.push(format!("  {}/  ({kind})", e.name));
            }
        }
        InjectMode::Standard | InjectMode::Rich => {
            lines.push(String::new());
            lines.push("Source map:".to_string());
            let map_lines = source_map_lines(&index.root, top_n);
            if map_lines.is_empty() {
                for e in &summ.top_level {
                    let extra = e
                        .file_count
                        .map(|c| format!("  {c} files"))
                        .unwrap_or_default();
                    let marker = if e.kind == NodeKind::CollapsedDir {
                        " (collapsed)"
                    } else {
                        ""
                    };
                    lines.push(format!("  {:<20}{extra}{marker}", format!("{}/", e.name)));
                }
            } else {
                lines.extend(map_lines);
            }

            let collapsed_note = collapsed_notes(&index.root, 6);
            if !collapsed_note.is_empty() {
                lines.push(String::new());
                lines.push(format!("Collapsed: {}", collapsed_note.join(", ")));
            }

            if config.inject.include_entrypoints {
                let entries = entrypoints(&index.root);
                if !entries.is_empty() {
                    lines.push(String::new());
                    lines.push(format!("Entrypoints: {}", entries.join(", ")));
                }
            }

            if matches!(config.inject.mode, InjectMode::Rich) {
                lines.push(String::new());
                lines.push(format!(
                    "Stats: {} files | {} dirs | {} collapsed | truncated={}",
                    summ.stats.files,
                    summ.stats.dirs,
                    summ.stats.collapsed_dirs,
                    summ.stats.truncated
                ));
            }
        }
    }

    lines.push(String::new());
    lines.push("Tools: workspace_tree, resolve_path".to_string());
    lines.push("Tip: resolve_path before inventing folders.".to_string());

    let mut card = lines.join("\n");
    if card.len() > max_chars {
        // Floor to UTF-8 char boundary (RC13: mid-char truncate panics).
        let mut end = max_chars.saturating_sub(20);
        while end > 0 && !card.is_char_boundary(end) {
            end -= 1;
        }
        card.truncate(end);
        card.push_str("\n... (truncated)");
    }
    card
}

fn source_map_lines(root: &TreeNode, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let Some(children) = root.children.as_ref() else {
        return out;
    };

    // Prefer source-ish dirs.
    let mut candidates: Vec<&TreeNode> = children
        .iter()
        .filter(|c| c.is_dir_like())
        .collect();
    candidates.sort_by_key(|c| {
        let source_boost = c
            .role_tags
            .as_ref()
            .map(|t| t.iter().any(|x| x == "source" || x == "scene" || x == "docs"))
            .unwrap_or(false);
        (!source_boost, std::cmp::Reverse(c.file_count.unwrap_or(0)))
    });

    for c in candidates.into_iter().take(limit) {
        // Expand one level of interesting children for standard map.
        if c.kind == NodeKind::Dir {
            if let Some(grand) = c.children.as_ref() {
                let dirs: Vec<&TreeNode> = grand.iter().filter(|g| g.is_dir_like()).collect();
                if !dirs.is_empty() && dirs.len() <= 12 {
                    for g in dirs.into_iter().take(8) {
                        out.push(format_map_line(g));
                    }
                    continue;
                }
            }
        }
        out.push(format_map_line(c));
        if out.len() >= limit {
            break;
        }
    }
    out.truncate(limit);
    out
}

fn format_map_line(n: &TreeNode) -> String {
    let path = if n.rel_path.is_empty() {
        format!("{}/", n.name)
    } else {
        format!("{}/", n.rel_path)
    };
    let count = n.file_count.unwrap_or(0);
    let collapsed = if n.kind == NodeKind::CollapsedDir {
        "  (collapsed)"
    } else {
        ""
    };
    let tags = n
        .role_tags
        .as_ref()
        .map(|t| format!("  [{}]", t.join(",")))
        .unwrap_or_default();
    format!("  {path:<22} {count} files{collapsed}{tags}")
}

fn collapsed_notes(root: &TreeNode, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(n: &TreeNode, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        if n.kind == NodeKind::CollapsedDir {
            out.push(format!(
                "{} ({} files)",
                n.rel_path,
                n.file_count.unwrap_or(0)
            ));
        }
        if let Some(children) = &n.children {
            for c in children {
                walk(c, out, limit);
            }
        }
    }
    walk(root, &mut out, limit);
    out
}

fn entrypoints(root: &TreeNode) -> Vec<String> {
    const NAMES: &[&str] = &[
        "project.godot",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "main.gd",
        "main.rs",
        "lib.rs",
        "index.ts",
        "index.js",
        "README.md",
    ];
    let mut out = Vec::new();
    if let Some(children) = root.children.as_ref() {
        for c in children {
            if NAMES.iter().any(|n| c.name.eq_ignore_ascii_case(n)) {
                out.push(c.rel_path.clone());
            }
            // one level into src/
            if c.name.eq_ignore_ascii_case("src") {
                if let Some(grand) = c.children.as_ref() {
                    for g in grand {
                        if NAMES.iter().any(|n| g.name.eq_ignore_ascii_case(n)) {
                            out.push(g.rel_path.clone());
                        }
                    }
                }
            }
        }
    }
    out
}

