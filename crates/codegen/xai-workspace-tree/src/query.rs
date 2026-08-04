//! Query API: summary, list, search, resolve_path.

use crate::error::{Error, Result};
use crate::model::{Freshness, NodeKind, TreeIndex, TreeNode};
use serde::{Deserialize, Serialize};

/// Shared envelope fields for query results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryMeta {
    pub freshness: Freshness,
    pub root: String,
    pub truncated: bool,
}

impl QueryMeta {
    fn from_index(index: &TreeIndex) -> Self {
        Self {
            freshness: index.meta.freshness.clone(),
            root: index.meta.canonical_root.clone(),
            truncated: index.meta.stats.truncated,
        }
    }
}

/// Summary action result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryResult {
    #[serde(flatten)]
    pub meta: QueryMeta,
    pub workspace_id: String,
    pub stats: crate::model::Stats,
    pub workspace_profile: Vec<String>,
    pub top_level: Vec<ListEntry>,
    pub build_duration_ms: u64,
}

/// One entry from list/search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEntry {
    pub rel_path: String,
    pub name: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_tags: Option<Vec<String>>,
}

impl ListEntry {
    fn from_node(n: &TreeNode) -> Self {
        Self {
            rel_path: n.rel_path.clone(),
            name: n.name.clone(),
            kind: n.kind,
            file_count: n.file_count,
            ext: n.ext.clone(),
            sample: n.sample.clone(),
            role_tags: n.role_tags.clone(),
        }
    }
}

/// List action result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResult {
    #[serde(flatten)]
    pub meta: QueryMeta,
    pub path: String,
    pub entries: Vec<ListEntry>,
}

/// Search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub rel_path: String,
    pub name: String,
    pub kind: NodeKind,
    pub score: f32,
}

/// Search action result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(flatten)]
    pub meta: QueryMeta,
    pub query: String,
    pub hits: Vec<SearchHit>,
}

/// Resolve-path candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveHit {
    pub rel_path: String,
    pub name: String,
    pub score: f32,
    pub reason: String,
}

/// Resolve-path result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveResult {
    #[serde(flatten)]
    pub meta: QueryMeta,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint_path: Option<String>,
    pub hits: Vec<ResolveHit>,
}

/// Budgeted summary of the index (tool `summary` action).
pub fn summary(index: &TreeIndex, limit: usize) -> SummaryResult {
    let top_level = index
        .root
        .children
        .as_ref()
        .map(|c| {
            c.iter()
                .take(limit)
                .map(ListEntry::from_node)
                .collect()
        })
        .unwrap_or_default();
    SummaryResult {
        meta: QueryMeta::from_index(index),
        workspace_id: index.meta.workspace_id.clone(),
        stats: index.meta.stats.clone(),
        workspace_profile: index.meta.workspace_profile.clone(),
        top_level,
        build_duration_ms: index.meta.build.duration_ms,
    }
}

/// List children under `path` (relative, POSIX). Empty path = root.
///
/// `depth` 1 = immediate children only; higher values expand nested dirs
/// up to that relative depth (collapsed nodes never expand).
pub fn list(index: &TreeIndex, path: &str, depth: u32, limit: usize) -> Result<ListResult> {
    let path = normalize_rel(path);
    let node = find_node(&index.root, &path).ok_or_else(|| Error::NotFound {
        path: path.clone(),
    })?;
    let depth = depth.max(1);
    let mut entries = Vec::new();
    collect_list(node, 1, depth, limit, &mut entries);
    Ok(ListResult {
        meta: QueryMeta::from_index(index),
        path,
        entries,
    })
}

/// Search basenames / path substrings (case-insensitive).
///
/// RC13 P2 F14: collect **all** candidates first, score, sort deterministically
/// (score desc, then path), then truncate — never stop mid-HashMap iteration.
pub fn search(index: &TreeIndex, query: &str, limit: usize) -> SearchResult {
    let q = query.trim().to_ascii_lowercase();
    let mut hits: Vec<SearchHit> = Vec::new();
    if q.is_empty() {
        return SearchResult {
            meta: QueryMeta::from_index(index),
            query: query.to_string(),
            hits,
        };
    }

    let push = |hits: &mut Vec<SearchHit>, p: &str, score: f32| {
        if hits.iter().any(|h| h.rel_path == p) {
            return;
        }
        let name = basename(p);
        let kind = find_node(&index.root, p)
            .map(|n| n.kind)
            .unwrap_or(NodeKind::File);
        hits.push(SearchHit {
            rel_path: p.to_string(),
            name,
            kind,
            score,
        });
    };

    // Prefer name_index exact / stem hits first (score 1.0).
    if let Some(paths) = index.name_index.get(&q) {
        for p in paths {
            push(&mut hits, p, 1.0);
        }
    }

    // Substring scan of every name_index key and path (full collect).
    for (key, paths) in &index.name_index {
        let key_hit = key.contains(&q);
        for p in paths {
            let path_hit = p.to_ascii_lowercase().contains(&q);
            if !key_hit && !path_hit {
                continue;
            }
            let score = if key == &q {
                1.0
            } else if key.starts_with(&q) {
                0.85
            } else if key_hit {
                0.7
            } else {
                0.5
            };
            push(&mut hits, p, score);
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    hits.truncate(limit);

    SearchResult {
        meta: QueryMeta::from_index(index),
        query: query.to_string(),
        hits,
    }
}

/// Resolve a free-form name or guessed path to ranked real paths.
pub fn resolve_path(
    index: &TreeIndex,
    name: &str,
    hint_path: Option<&str>,
    limit: usize,
) -> ResolveResult {
    let raw = name.trim();
    let q = raw.to_ascii_lowercase();
    // Strip directories from name if user passed a path-like string.
    let base = basename(&q);
    let stem = PathStem::new(&base);

    let mut hits: Vec<ResolveHit> = Vec::new();

    let push_hit = |hits: &mut Vec<ResolveHit>, rel: &str, score: f32, reason: &str| {
        if hits.iter().any(|h| h.rel_path == rel) {
            return;
        }
        hits.push(ResolveHit {
            rel_path: rel.to_string(),
            name: basename(rel),
            score,
            reason: reason.to_string(),
        });
    };

    // Exact basename / stem from index.
    for key in [base.as_str(), stem.as_str()] {
        if key.is_empty() {
            continue;
        }
        if let Some(paths) = index.name_index.get(key) {
            for p in paths {
                let mut score = if basename(p).eq_ignore_ascii_case(raw) {
                    1.0
                } else if stem_of(p).eq_ignore_ascii_case(stem.as_str()) {
                    0.92
                } else {
                    0.85
                };
                let mut reason = if score >= 0.99 {
                    "exact_basename"
                } else {
                    "stem_match"
                };
                if let Some(hint) = hint_path {
                    let bonus = path_similarity(p, hint);
                    if bonus > 0.0 {
                        score = (score + bonus * 0.15).min(1.0);
                        reason = "stem_match+hint";
                    }
                }
                push_hit(&mut hits, p, score, reason);
            }
        }
    }

    // Substring fallback.
    if hits.len() < limit && base.len() >= 2 {
        for (key, paths) in &index.name_index {
            if key.contains(&base) {
                for p in paths {
                    let mut score = 0.55;
                    if let Some(hint) = hint_path {
                        score += path_similarity(p, hint) * 0.2;
                    }
                    push_hit(&mut hits, p, score.min(0.9), "substring");
                }
            }
        }
    }

    // If name looks like a path, also try find_node and nearest.
    let norm = normalize_rel(raw);
    if norm.contains('/') {
        if find_node(&index.root, &norm).is_some() {
            push_hit(&mut hits, &norm, 1.0, "exact_path");
        }
    }

    hits.sort_by(|a, b| match b.score.partial_cmp(&a.score) {
        Some(ord) => ord.then_with(|| a.rel_path.cmp(&b.rel_path)),
        None => a.rel_path.cmp(&b.rel_path),
    });
    hits.truncate(limit.max(1));

    ResolveResult {
        meta: QueryMeta::from_index(index),
        name: name.to_string(),
        hint_path: hint_path.map(|s| s.to_string()),
        hits,
    }
}

fn collect_list(node: &TreeNode, level: u32, max_depth: u32, limit: usize, out: &mut Vec<ListEntry>) {
    if out.len() >= limit {
        return;
    }
    let Some(children) = node.children.as_ref() else {
        return;
    };
    for child in children {
        if out.len() >= limit {
            return;
        }
        out.push(ListEntry::from_node(child));
        if level < max_depth && child.kind == NodeKind::Dir {
            collect_list(child, level + 1, max_depth, limit, out);
        }
    }
}

/// Find a node by relative POSIX path. Empty path returns root.
///
/// RC13 P2 F13: on Windows, path segment matching is case-insensitive.
pub fn find_node<'a>(root: &'a TreeNode, rel: &str) -> Option<&'a TreeNode> {
    let rel = normalize_rel(rel);
    if rel.is_empty() {
        return Some(root);
    }
    let mut node = root;
    for part in rel.split('/') {
        if part.is_empty() {
            continue;
        }
        let children = node.children.as_ref()?;
        node = children.iter().find(|c| name_eq(&c.name, part))?;
    }
    Some(node)
}

#[inline]
fn name_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn normalize_rel(path: &str) -> String {
    let p = path.trim().trim_start_matches("./").replace('\\', "/");
    p.trim_matches('/').to_string()
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn stem_of(path: &str) -> String {
    let name = basename(path);
    PathStem::new(&name).as_str().to_string()
}

struct PathStem<'a> {
    s: &'a str,
}

impl<'a> PathStem<'a> {
    fn new(name: &'a str) -> Self {
        if let Some(i) = name.rfind('.') {
            if i > 0 {
                return Self { s: &name[..i] };
            }
        }
        Self { s: name }
    }
    fn as_str(&self) -> &str {
        self.s
    }
}

/// Segment-overlap similarity in 0..1, weighted by shared **prefix** length.
///
/// RC13: deeper shared prefixes beat scattered segment matches (hint bias).
fn path_similarity(a: &str, b: &str) -> f32 {
    let a_norm = normalize_rel(a);
    let b_norm = normalize_rel(b);
    let a_segs: Vec<&str> = a_norm.split('/').filter(|s| !s.is_empty()).collect();
    let b_segs: Vec<&str> = b_norm.split('/').filter(|s| !s.is_empty()).collect();
    if a_segs.is_empty() || b_segs.is_empty() {
        return 0.0;
    }
    let mut prefix = 0usize;
    for (i, seg) in a_segs.iter().enumerate() {
        if b_segs.get(i).is_some_and(|x| x.eq_ignore_ascii_case(seg)) {
            prefix += 1;
        } else {
            break;
        }
    }
    let mut bag = 0usize;
    for seg in &a_segs {
        if b_segs.iter().any(|x| x.eq_ignore_ascii_case(seg)) {
            bag += 1;
        }
    }
    let denom = a_segs.len().max(b_segs.len()) as f32;
    // Prefer longest shared prefix (0.7) over bag-of-segments (0.3).
    (prefix as f32 / denom) * 0.7 + (bag as f32 / denom) * 0.3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_similarity_prefers_shared_segments() {
        let s = path_similarity("scripts/core/ship_roster.gd", "scripts/ship/ship_roster.gd");
        assert!(s > 0.3);
    }

    #[test]
    fn path_similarity_prefers_deeper_prefix() {
        let deep = path_similarity("scripts/ship/a.gd", "scripts/ship/b.gd");
        let shallow = path_similarity("scripts/core/a.gd", "scripts/ship/b.gd");
        assert!(
            deep > shallow,
            "deeper shared prefix should score higher: deep={deep} shallow={shallow}"
        );
    }

    #[test]
    fn find_node_case_insensitive_on_windows() {
        use crate::model::{NodeKind, TreeNode};
        let mut root = TreeNode::dir("", "");
        let mut scripts = TreeNode::dir("Scripts", "Scripts");
        scripts.children = Some(vec![TreeNode::file("Scripts/Main.rs", "Main.rs")]);
        root.children = Some(vec![scripts]);
        // On Windows, mixed case should resolve; on Unix only exact match.
        let hit = find_node(&root, "scripts/main.rs");
        if cfg!(windows) {
            assert!(hit.is_some());
            assert_eq!(hit.unwrap().kind, NodeKind::File);
        } else {
            // exact still works
            assert!(find_node(&root, "Scripts/Main.rs").is_some());
        }
    }
}

