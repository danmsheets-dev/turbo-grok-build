//! Directory walker: gitignore + hard excludes + collapse.

use crate::config::{
    default_hard_exclude_exts, effective_hard_exclude_names, WorkspaceTreeConfig,
};
use crate::error::{Error, Result};
use crate::identity::{canonicalize_root, path_to_identity_key, workspace_id_for_canonical};
use crate::model::{
    detect_workspace_profile, role_tags_for, to_posix_rel, BuildInfo, Freshness, FreshnessState,
    GitInfo, Meta, NodeKind, Stats, TreeIndex, TreeNode, SCHEMA_VERSION,
};
use ignore::WalkBuilder;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Build a full in-memory tree index for `root`.
pub fn build_index(root: &Path, config: &WorkspaceTreeConfig) -> Result<TreeIndex> {
    if !config.enabled {
        return Err(Error::Disabled);
    }

    let started = Instant::now();
    let canonical = canonicalize_root(root)?;
    let workspace_id = workspace_id_for_canonical(&canonical);
    let root_display = path_to_identity_key(&canonical);

    let hard_names = effective_hard_exclude_names(config);
    let hard_exts = default_hard_exclude_exts();
    let hard_name_set: std::collections::HashSet<String> = hard_names
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let hard_ext_set: std::collections::HashSet<String> =
        hard_exts.iter().map(|s| s.to_ascii_lowercase()).collect();

    let mut stats = Stats::default();

    // Collect entries: rel_path -> is_dir, optional metadata.
    // BTreeMap keeps children sorted by name for stable output.
    let mut entries: BTreeMap<String, EntryMeta> = BTreeMap::new();

    let mut builder = WalkBuilder::new(&canonical);
    builder
        .hidden(false)
        .follow_links(config.walk.follow_symlinks)
        .max_depth(Some(config.walk.max_depth as usize))
        .git_ignore(config.walk.use_gitignore)
        .git_global(config.walk.use_global_gitignore)
        .git_exclude(config.walk.use_gitignore)
        .require_git(false);

    // Hard-exclude filter: skip matching directories entirely.
    // RC13 P1 F12: when follow_symlinks is on, skip symlink targets already visited
    // (cycle guard). Default follow_symlinks is false.
    let hard_names_for_filter = hard_name_set.clone();
    let hard_exts_for_filter = hard_ext_set.clone();
    let follow_symlinks = config.walk.follow_symlinks;
    let visited_symlink_targets: Arc<Mutex<HashSet<PathBuf>>> =
        Arc::new(Mutex::new(HashSet::new()));
    let visited_for_filter = Arc::clone(&visited_symlink_targets);
    builder.filter_entry(move |entry| {
        let name = entry.file_name().to_string_lossy();
        let name_l = name.to_ascii_lowercase();
        if hard_names_for_filter.contains(name_l.as_str()) {
            // Note: hard-exclude filter cannot bump Stats (cloneable filter).
            // ignored_dirs for hard excludes is approximate via walk errors only.
            return false;
        }
        // Skip `.grok/worktrees` as a path segment pair when possible: if parent is
        // `.grok` and name is `worktrees`, exclude.
        if name_l == "worktrees" {
            if let Some(parent) = entry.path().parent() {
                if parent
                    .file_name()
                    .map(|n| n.eq_ignore_ascii_case(".grok"))
                    .unwrap_or(false)
                {
                    return false;
                }
            }
        }
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Some(ext) = Path::new(name.as_ref())
                .extension()
                .and_then(|e| e.to_str())
            {
                if hard_exts_for_filter.contains(&ext.to_ascii_lowercase()) {
                    return false;
                }
            }
        }
        if follow_symlinks
            && entry.file_type().map(|t| t.is_symlink()).unwrap_or(false)
        {
            if let Ok(target) = std::fs::canonicalize(entry.path()) {
                let mut seen = visited_for_filter
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if !seen.insert(target) {
                    // Cycle: already walked this symlink target.
                    return false;
                }
            }
        }
        true
    });

    let walk = builder.build();
    let deadline = if config.walk.max_duration_ms == 0 {
        None
    } else {
        Some(started + std::time::Duration::from_millis(config.walk.max_duration_ms))
    };

    for result in walk {
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                stats.truncated = true;
                break;
            }
        }

        let entry = match result {
            Ok(e) => e,
            Err(_err) => {
                // Soft-fail individual entries; count as ignored for doctor/stats.
                stats.ignored_dirs = stats.ignored_dirs.saturating_add(1);
                continue;
            }
        };

        let path = entry.path();
        if path == canonical {
            continue;
        }

        let rel = match path.strip_prefix(&canonical) {
            Ok(r) => to_posix_rel(r),
            Err(_) => continue,
        };
        if rel.is_empty() {
            continue;
        }

        let ft = match entry.file_type() {
            Some(t) => t,
            None => continue,
        };

        // Depth check already applied by WalkBuilder; still track caps.
        if ft.is_dir() {
            if stats.dirs >= config.walk.max_dirs {
                stats.truncated = true;
                break;
            }
            stats.dirs = stats.dirs.saturating_add(1);
            let mut meta = EntryMeta {
                is_dir: true,
                is_symlink: ft.is_symlink(),
                size_bytes: None,
                mtime_ms: None,
            };
            if config.walk.collect_mtime || config.walk.collect_size {
                if let Ok(md) = entry.metadata() {
                    if config.walk.collect_size {
                        meta.size_bytes = Some(md.len());
                    }
                    if config.walk.collect_mtime {
                        meta.mtime_ms = mtime_ms_from(&md);
                    }
                }
            }
            entries.insert(rel, meta);
        } else if ft.is_file() || ft.is_symlink() {
            if stats.files >= config.walk.max_files {
                stats.truncated = true;
                break;
            }
            stats.files = stats.files.saturating_add(1);
            let mut meta = EntryMeta {
                is_dir: false,
                is_symlink: ft.is_symlink(),
                size_bytes: None,
                mtime_ms: None,
            };
            if config.walk.collect_mtime || config.walk.collect_size {
                if let Ok(md) = entry.metadata() {
                    if config.walk.collect_size {
                        meta.size_bytes = Some(md.len());
                        stats.bytes_seen = stats.bytes_seen.saturating_add(md.len());
                    }
                    if config.walk.collect_mtime {
                        meta.mtime_ms = mtime_ms_from(&md);
                    }
                }
            }
            entries.insert(rel, meta);
        }
    }

    // Ensure parent directories exist even if only files were yielded.
    let paths: Vec<String> = entries.keys().cloned().collect();
    for rel in paths {
        ensure_parents(&mut entries, &rel, &mut stats, config.walk.max_dirs);
    }

    let mut root_node = TreeNode::dir("", "");
    root_node.name = canonical
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root_display.clone());

    // Insert all entries into the tree.
    for (rel, meta) in &entries {
        insert_entry(&mut root_node, rel, meta);
    }

    // Compute aggregate counts bottom-up.
    recompute_counts(&mut root_node);

    // RC13 P1 F6: name index from the **full pre-collapse** tree so monorepo
    // packages under large dirs remain resolvable after display collapse.
    let mut name_index: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    build_name_index(&root_node, &mut name_index);

    // Apply collapse rules (display only; name_index already complete).
    let sample_n = config.collapse.sample_names as usize;
    apply_collapse(&mut root_node, 0, config, sample_n, &mut stats);

    // Role tags on nodes.
    annotate_roles(&mut root_node);

    let child_names: Vec<String> = root_node
        .children
        .as_ref()
        .map(|c| c.iter().map(|n| n.name.clone()).collect())
        .unwrap_or_default();
    let profile = detect_workspace_profile(&child_names);

    let duration_ms = started.elapsed().as_millis() as u64;
    // `updated_at` is the build timestamp (`built_at` for Phase 1 freshness).
    let now = chrono::Utc::now().to_rfc3339();

    // Recount stats from final tree for accuracy after collapse.
    let (files, dirs, collapsed) = count_tree(&root_node);
    stats.files = files;
    stats.dirs = dirs;
    stats.collapsed_dirs = collapsed;

    let git = detect_git(&canonical);
    let basis = freshness_basis(&now, &git);

    let meta = Meta {
        schema_version: SCHEMA_VERSION,
        workspace_id,
        root: root_display.clone(),
        canonical_root: root_display,
        git,
        created_at: now.clone(),
        updated_at: now,
        build: BuildInfo {
            mode: "full".to_string(),
            duration_ms,
            walker: "gitignore_v1".to_string(),
            app_version: None,
        },
        stats,
        freshness: Freshness {
            state: FreshnessState::Fresh,
            // Phase 1 always stamps Fresh after a full walk. Record built_at
            // (via meta.updated_at) and git HEAD in `basis` so consumers can
            // reason about staleness later without re-walking.
            basis: Some(basis),
            dirty_paths: 0,
        },
        workspace_profile: profile,
    };

    Ok(TreeIndex {
        meta,
        root: root_node,
        name_index,
    })
}

#[derive(Clone)]
struct EntryMeta {
    is_dir: bool,
    is_symlink: bool,
    size_bytes: Option<u64>,
    mtime_ms: Option<u64>,
}

fn mtime_ms_from(md: &std::fs::Metadata) -> Option<u64> {
    md.modified().ok().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as u64)
    })
}

fn ensure_parents(
    entries: &mut BTreeMap<String, EntryMeta>,
    rel: &str,
    stats: &mut Stats,
    max_dirs: u32,
) {
    let mut acc = String::new();
    for part in rel.split('/') {
        if part.is_empty() {
            continue;
        }
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(part);
        if acc == rel {
            break;
        }
        entries.entry(acc.clone()).or_insert_with(|| {
            if stats.dirs < max_dirs {
                stats.dirs = stats.dirs.saturating_add(1);
            }
            EntryMeta {
                is_dir: true,
                is_symlink: false,
                size_bytes: None,
                mtime_ms: None,
            }
        });
    }
}

fn insert_entry(root: &mut TreeNode, rel: &str, meta: &EntryMeta) {
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    let mut node = root;
    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let child_rel = parts[..=i].join("/");
        if !is_last {
            // Ensure intermediate dir.
            let children = node.children.get_or_insert_with(Vec::new);
            if let Some(pos) = children.iter().position(|c| c.name == *part) {
                node = &mut children[pos];
            } else {
                children.push(TreeNode::dir(child_rel, (*part).to_string()));
                // Sort by name for stability
                children.sort_by(|a, b| a.name.cmp(&b.name));
                let pos = children.iter().position(|c| c.name == *part).unwrap();
                node = &mut children[pos];
            }
            continue;
        }

        // Leaf insertion
        let children = node.children.get_or_insert_with(Vec::new);
        if children.iter().any(|c| c.name == *part) {
            return;
        }
        let mut child = if meta.is_dir {
            let mut d = TreeNode::dir(child_rel, (*part).to_string());
            if meta.is_symlink {
                d.kind = NodeKind::Symlink;
            }
            d
        } else {
            let mut f = TreeNode::file(child_rel, (*part).to_string());
            if meta.is_symlink {
                f.kind = NodeKind::Symlink;
            }
            f.size_bytes = meta.size_bytes;
            f.mtime_ms = meta.mtime_ms;
            f
        };
        child.size_bytes = child.size_bytes.or(meta.size_bytes);
        child.mtime_ms = child.mtime_ms.or(meta.mtime_ms);
        children.push(child);
        children.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

fn recompute_counts(node: &mut TreeNode) -> (u32, u32) {
    if node.kind == NodeKind::CollapsedDir {
        return (node.file_count.unwrap_or(0), node.dir_count.unwrap_or(0));
    }
    if node.kind != NodeKind::Dir {
        return (0, 0);
    }
    let mut files = 0u32;
    let mut dirs = 0u32;
    if let Some(children) = node.children.as_mut() {
        for child in children.iter_mut() {
            match child.kind {
                NodeKind::File | NodeKind::Symlink => {
                    files = files.saturating_add(1);
                }
                NodeKind::Dir | NodeKind::CollapsedDir => {
                    let (f, d) = recompute_counts(child);
                    files = files.saturating_add(f);
                    dirs = dirs.saturating_add(1).saturating_add(d);
                }
            }
        }
    }
    node.file_count = Some(files);
    node.dir_count = Some(dirs);
    (files, dirs)
}

fn apply_collapse(
    node: &mut TreeNode,
    depth: u32,
    config: &WorkspaceTreeConfig,
    sample_n: usize,
    stats: &mut Stats,
) {
    if node.kind != NodeKind::Dir {
        return;
    }

    // First collapse children recursively so counts stabilize.
    if let Some(children) = node.children.as_mut() {
        for child in children.iter_mut() {
            apply_collapse(child, depth + 1, config, sample_n, stats);
        }
    }

    // Never collapse the root node.
    if depth == 0 {
        recompute_counts(node);
        return;
    }

    let file_count = node.file_count.unwrap_or(0);
    let should = should_collapse(node, depth, file_count, config);
    if should {
        let sample = collect_samples(node, sample_n);
        node.kind = NodeKind::CollapsedDir;
        node.children = None;
        node.sample = if sample.is_empty() {
            None
        } else {
            Some(sample)
        };
        stats.collapsed_dirs = stats.collapsed_dirs.saturating_add(1);
    }
}

fn should_collapse(
    node: &TreeNode,
    depth: u32,
    file_count: u32,
    config: &WorkspaceTreeConfig,
) -> bool {
    if depth > config.walk.max_expand_depth {
        return true;
    }
    if file_count > config.collapse.max_files_per_dir {
        return true;
    }
    let name_l = node.name.to_ascii_lowercase();
    if config
        .collapse
        .names
        .iter()
        .any(|n| n.eq_ignore_ascii_case(&name_l))
    {
        return true;
    }
    let rel = node.rel_path.replace('\\', "/");
    for glob in &config.collapse.globs {
        if path_matches_glob(&rel, glob) {
            return true;
        }
        // Also match if this dir is a prefix of a ** glob target, e.g. assets/models
        if let Some(prefix) = glob.strip_suffix("/**") {
            if rel == prefix || rel.starts_with(&format!("{prefix}/")) {
                return true;
            }
        }
    }
    false
}

/// Minimal glob matcher: supports `*` (within segment) and `**` (across segments).
pub fn path_matches_glob(path: &str, pattern: &str) -> bool {
    let path = path.trim_start_matches("./").trim_matches('/');
    let pattern = pattern.trim_start_matches("./");
    match_glob(path, pattern)
}

fn match_glob(path: &str, pattern: &str) -> bool {
    if pattern == "**" || pattern == "*" {
        return true;
    }
    if pattern.is_empty() {
        return path.is_empty();
    }

    // Fast path: prefix/**
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }

    // Split into segments for general matching.
    let path_segs: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    let pat_segs: Vec<&str> = pattern.split('/').collect();
    match_segments(&path_segs, &pat_segs)
}

fn match_segments(path: &[&str], pat: &[&str]) -> bool {
    if pat.is_empty() {
        return path.is_empty();
    }
    if pat[0] == "**" {
        // Match zero or more path segments.
        if pat.len() == 1 {
            return true;
        }
        // Try consuming 0..path.len() segments for **.
        for i in 0..=path.len() {
            if match_segments(&path[i..], &pat[1..]) {
                return true;
            }
        }
        return false;
    }
    if path.is_empty() {
        return false;
    }
    if !segment_matches(path[0], pat[0]) {
        return false;
    }
    match_segments(&path[1..], &pat[1..])
}

fn segment_matches(seg: &str, pat: &str) -> bool {
    if pat == "*" {
        return true;
    }
    if !pat.contains('*') {
        return seg == pat;
    }
    // Simple star glob within one segment
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.is_empty() {
        return true;
    }
    if !seg.starts_with(parts[0]) {
        return false;
    }
    if !seg.ends_with(parts[parts.len() - 1]) {
        return false;
    }
    let mut cursor = parts[0].len();
    for p in &parts[1..parts.len() - 1] {
        if p.is_empty() {
            continue;
        }
        if let Some(pos) = seg[cursor..].find(p) {
            cursor += pos + p.len();
        } else {
            return false;
        }
    }
    true
}

/// Collect sample paths for collapsed-dir display.
///
/// RC13 P1 F7: store **relative paths under the collapsed node** (posix), not
/// bare basenames — so name-index / display never invents `parent/file` when
/// the real file is `parent/nested/file`.
fn collect_samples(node: &TreeNode, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let base = node.rel_path.as_str();
    fn walk(n: &TreeNode, base: &str, out: &mut Vec<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        match n.kind {
            NodeKind::File | NodeKind::Symlink => {
                let rel = if base.is_empty() {
                    n.rel_path.clone()
                } else if let Some(rest) = n.rel_path.strip_prefix(base) {
                    rest.trim_start_matches('/').to_string()
                } else {
                    n.name.clone()
                };
                if !rel.is_empty() {
                    out.push(rel);
                }
            }
            NodeKind::Dir => {
                if let Some(children) = &n.children {
                    for c in children {
                        walk(c, base, out, limit);
                        if out.len() >= limit {
                            return;
                        }
                    }
                }
            }
            NodeKind::CollapsedDir => {
                if let Some(sample) = &n.sample {
                    for s in sample {
                        if out.len() >= limit {
                            return;
                        }
                        out.push(s.clone());
                    }
                }
            }
        }
    }
    walk(node, base, &mut out, limit);
    out
}

fn build_name_index(node: &TreeNode, index: &mut std::collections::HashMap<String, Vec<String>>) {
    if !node.rel_path.is_empty() && !node.name.is_empty() {
        let key = node.name.to_ascii_lowercase();
        index.entry(key).or_default().push(node.rel_path.clone());
        // Also index stem for files
        if node.kind == NodeKind::File {
            if let Some(stem) = Path::new(&node.name)
                .file_stem()
                .and_then(|s| s.to_str())
            {
                let sk = stem.to_ascii_lowercase();
                if sk != node.name.to_ascii_lowercase() {
                    index.entry(sk).or_default().push(node.rel_path.clone());
                }
            }
        }
    }
    if let Some(children) = &node.children {
        for c in children {
            build_name_index(c, index);
        }
    }
    // Samples are paths relative to this node (or absolute-from-root if pre-collapse
    // already filled name_index). Index full workspace-relative paths.
    if let Some(sample) = &node.sample {
        for s in sample {
            let basename = Path::new(s)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(s.as_str());
            let key = basename.to_ascii_lowercase();
            let rel = if s.contains('/') || s.contains('\\') {
                // Sample already relative to collapsed node or full-ish path.
                if node.rel_path.is_empty() {
                    s.replace('\\', "/")
                } else if s.starts_with(&node.rel_path) {
                    s.replace('\\', "/")
                } else {
                    format!("{}/{}", node.rel_path, s.replace('\\', "/"))
                }
            } else if node.rel_path.is_empty() {
                s.clone()
            } else {
                format!("{}/{}", node.rel_path, s)
            };
            index.entry(key).or_default().push(rel.clone());
            if let Some(stem) = Path::new(basename).file_stem().and_then(|x| x.to_str()) {
                let sk = stem.to_ascii_lowercase();
                index.entry(sk).or_default().push(rel);
            }
        }
    }
}

fn annotate_roles(node: &mut TreeNode) {
    let is_dir = node.is_dir_like();
    let tags = role_tags_for(&node.rel_path, node.ext.as_deref(), is_dir);
    if !tags.is_empty() {
        node.role_tags = Some(tags);
    }
    if let Some(children) = node.children.as_mut() {
        for c in children {
            annotate_roles(c);
        }
    }
}

fn count_tree(node: &TreeNode) -> (u32, u32, u32) {
    let mut files = 0u32;
    let mut dirs = 0u32;
    let mut collapsed = 0u32;
    fn walk(n: &TreeNode, files: &mut u32, dirs: &mut u32, collapsed: &mut u32) {
        match n.kind {
            NodeKind::File => *files = files.saturating_add(1),
            NodeKind::Symlink => *files = files.saturating_add(1),
            NodeKind::Dir => {
                if !n.rel_path.is_empty() {
                    *dirs = dirs.saturating_add(1);
                }
                if let Some(children) = &n.children {
                    for c in children {
                        walk(c, files, dirs, collapsed);
                    }
                }
            }
            NodeKind::CollapsedDir => {
                *collapsed = collapsed.saturating_add(1);
                *dirs = dirs.saturating_add(1);
                *files = files.saturating_add(n.file_count.unwrap_or(0));
            }
        }
    }
    walk(node, &mut files, &mut dirs, &mut collapsed);
    (files, dirs, collapsed)
}

/// Format freshness basis: `full_walk+built_at=<ts>[+git_head=<short>]`.
fn freshness_basis(built_at: &str, git: &GitInfo) -> String {
    let mut basis = format!("full_walk+built_at={built_at}");
    if let Some(ref head) = git.head {
        let short = if head.len() > 8 { &head[..8] } else { head.as_str() };
        basis.push_str("+git_head=");
        basis.push_str(short);
        if let Some(ref branch) = git.branch {
            basis.push('@');
            basis.push_str(branch);
        }
    }
    basis
}

/// Reassess freshness of a loaded index against the live workspace root.
///
/// RC13 P0 F4 — never leave an unchecked durable snapshot labeled `Fresh`:
/// - stored git HEAD ≠ current HEAD → [`FreshnessState::Stale`]
/// - git present now but not stored (or vice versa) → [`FreshnessState::LikelyFresh`]
/// - same HEAD (or no git both sides) → keep [`FreshnessState::Fresh`] (or prior non-fresh)
///
/// Updates `meta.freshness` and refreshes `meta.git` snapshot to the live value
/// when available (so cards report current HEAD after reassessment).
pub fn reassess_freshness(root: &Path, meta: &mut Meta) -> FreshnessState {
    let live = detect_git(root);
    let state = match (meta.git.head.as_deref(), live.head.as_deref()) {
        (Some(stored), Some(current)) if stored != current => {
            let short = |h: &str| {
                if h.len() > 8 {
                    h[..8].to_string()
                } else {
                    h.to_string()
                }
            };
            meta.freshness.basis = Some(format!(
                "stale_git_head stored={} current={}",
                short(stored),
                short(current)
            ));
            FreshnessState::Stale
        }
        (Some(_), None) | (None, Some(_)) => {
            meta.freshness.basis = Some(
                "likely_fresh: git head presence changed (or unreadable)".into(),
            );
            FreshnessState::LikelyFresh
        }
        (Some(_), Some(_)) => {
            // Same HEAD — still trustworthy for Phase 1 full-walk semantics.
            if !matches!(
                meta.freshness.state,
                FreshnessState::Stale | FreshnessState::Error | FreshnessState::Missing
            ) {
                FreshnessState::Fresh
            } else {
                meta.freshness.state
            }
        }
        (None, None) => {
            // No git identity either side — cannot prove staleness from HEAD.
            // Leave prior state unless it was default Fresh from an old stamp.
            meta.freshness.state
        }
    };
    meta.freshness.state = state;
    if live.present {
        meta.git = live;
    }
    state
}

pub(crate) fn detect_git(root: &Path) -> GitInfo {
    let git_path = root.join(".git");
    if !git_path.exists() {
        return GitInfo::default();
    }

    // Regular repo (`.git/` dir) or linked worktree (`.git` file with `gitdir:`).
    // RC13 P2 F19: resolve `commondir` for linked worktrees so HEAD SHA is found
    // under the real object store, not only the worktree gitdir.
    let (common_dir, git_dir) = if git_path.is_file() {
        match std::fs::read_to_string(&git_path) {
            Ok(contents) => {
                let gitdir_line = contents
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("gitdir:"))
                    .map(|s| s.trim().to_string());
                match gitdir_line {
                    Some(gd) => {
                        let gd_path = {
                            let p = PathBuf::from(&gd);
                            if p.is_absolute() {
                                p
                            } else {
                                root.join(p)
                            }
                        };
                        let common = resolve_git_common_dir(&gd_path);
                        (
                            Some(path_to_identity_key(common.as_ref().unwrap_or(&gd_path))),
                            gd_path,
                        )
                    }
                    None => {
                        return GitInfo {
                            present: true,
                            head: None,
                            branch: None,
                            common_dir: None,
                        };
                    }
                }
            }
            Err(_) => {
                return GitInfo {
                    present: true,
                    head: None,
                    branch: None,
                    common_dir: None,
                };
            }
        }
    } else {
        (
            Some(path_to_identity_key(&git_path)),
            git_path.clone(),
        )
    };

    // Re-resolve common dir for ref lookup (identity string is for meta only).
    let common_path =
        resolve_git_common_dir(&git_dir).unwrap_or_else(|| git_dir.clone());

    let (head, branch) = read_git_head_and_branch(&git_dir, &common_path);
    GitInfo {
        present: true,
        head,
        branch,
        common_dir,
    }
}

/// Resolve the shared git object store for a worktree gitdir (`commondir` file).
fn resolve_git_common_dir(git_dir: &Path) -> Option<PathBuf> {
    let commondir = git_dir.join("commondir");
    if let Ok(contents) = std::fs::read_to_string(&commondir) {
        let rel = contents.trim();
        if !rel.is_empty() {
            let p = git_dir.join(rel);
            if let Ok(canon) = dunce::canonicalize(&p) {
                return Some(canon);
            }
            return Some(p);
        }
    }
    // Non-worktree: common dir is the git dir itself.
    if git_dir.join("objects").is_dir() || git_dir.join("HEAD").is_file() {
        return Some(git_dir.to_path_buf());
    }
    None
}

/// Best-effort HEAD sha + branch name from a worktree git dir + common dir.
///
/// Linked worktrees keep HEAD in the worktree gitdir but branch refs often live
/// under the common dir (`commondir`).
fn read_git_head_and_branch(
    git_dir: &Path,
    common_dir: &Path,
) -> (Option<String>, Option<String>) {
    let head_path = git_dir.join("HEAD");
    let Ok(head_contents) = std::fs::read_to_string(&head_path) else {
        // Fallback: try common dir HEAD
        let head_path = common_dir.join("HEAD");
        let Ok(head_contents) = std::fs::read_to_string(&head_path) else {
            return (None, None);
        };
        return parse_head_contents(head_contents.trim(), git_dir, common_dir);
    };
    parse_head_contents(head_contents.trim(), git_dir, common_dir)
}

fn parse_head_contents(
    head_contents: &str,
    git_dir: &Path,
    common_dir: &Path,
) -> (Option<String>, Option<String>) {
    if let Some(ref_path) = head_contents.strip_prefix("ref: ") {
        let ref_path = ref_path.trim();
        let branch = ref_path
            .strip_prefix("refs/heads/")
            .map(|b| b.to_string())
            .or_else(|| Some(ref_path.to_string()));
        // Prefer worktree-local ref, then common dir (packed-refs last).
        let sha = std::fs::read_to_string(git_dir.join(ref_path))
            .ok()
            .or_else(|| std::fs::read_to_string(common_dir.join(ref_path)).ok())
            .or_else(|| read_packed_ref(common_dir, ref_path))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        (sha, branch)
    } else if head_contents.len() >= 7 && head_contents.chars().all(|c| c.is_ascii_hexdigit()) {
        // Detached HEAD
        (Some(head_contents.to_string()), None)
    } else {
        (None, None)
    }
}

fn read_packed_ref(common_dir: &Path, ref_path: &str) -> Option<String> {
    let packed = std::fs::read_to_string(common_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let sha = parts.next()?;
        let name = parts.next()?;
        if name == ref_path {
            return Some(sha.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_double_star_prefix() {
        assert!(path_matches_glob("assets/models/hull", "assets/models/**"));
        assert!(path_matches_glob(
            "assets/models/hull/a.glb",
            "assets/models/**"
        ));
        assert!(!path_matches_glob("scripts/core", "assets/models/**"));
    }

    #[test]
    fn glob_star_ext() {
        assert!(path_matches_glob("foo/bar.import", "**/*.import"));
        assert!(!path_matches_glob("foo/bar.gd", "**/*.import"));
    }

    #[test]
    fn reassess_marks_stale_when_git_head_moves() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Fake .git with HEAD pointing at a SHA
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::fs::write(root.join(".git/refs/heads/main"), b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n")
            .unwrap();

        let mut meta = Meta {
            schema_version: SCHEMA_VERSION,
            workspace_id: "ws_test".into(),
            root: root.display().to_string(),
            canonical_root: root.display().to_string(),
            git: GitInfo {
                present: true,
                head: Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()),
                branch: Some("main".into()),
                common_dir: None,
            },
            created_at: "t".into(),
            updated_at: "t".into(),
            build: BuildInfo {
                mode: "full".into(),
                duration_ms: 1,
                walker: "test".into(),
                app_version: None,
            },
            stats: Stats::default(),
            freshness: Freshness {
                state: FreshnessState::Fresh,
                basis: Some("full_walk".into()),
                dirty_paths: 0,
            },
            workspace_profile: vec![],
        };
        let state = reassess_freshness(root, &mut meta);
        assert_eq!(state, FreshnessState::Stale);
        assert_eq!(meta.freshness.state, FreshnessState::Stale);
        // Live HEAD should be reflected after reassess.
        assert_eq!(
            meta.git.head.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn reassess_same_head_stays_fresh() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        let sha = "cccccccccccccccccccccccccccccccccccccccc";
        std::fs::write(root.join(".git/refs/heads/main"), format!("{sha}\n")).unwrap();

        let mut meta = Meta {
            schema_version: SCHEMA_VERSION,
            workspace_id: "ws_test".into(),
            root: root.display().to_string(),
            canonical_root: root.display().to_string(),
            git: GitInfo {
                present: true,
                head: Some(sha.into()),
                branch: Some("main".into()),
                common_dir: None,
            },
            created_at: "t".into(),
            updated_at: "t".into(),
            build: BuildInfo {
                mode: "full".into(),
                duration_ms: 1,
                walker: "test".into(),
                app_version: None,
            },
            stats: Stats::default(),
            freshness: Freshness {
                state: FreshnessState::Fresh,
                basis: Some("full_walk".into()),
                dirty_paths: 0,
            },
            workspace_profile: vec![],
        };
        assert_eq!(reassess_freshness(root, &mut meta), FreshnessState::Fresh);
    }
}

