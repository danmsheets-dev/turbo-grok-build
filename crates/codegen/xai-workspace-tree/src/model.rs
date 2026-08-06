//! Core data model for the workspace tree atlas.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Schema version written to meta and tree payloads.
pub const SCHEMA_VERSION: u32 = 1;

/// Kind of a tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Dir,
    CollapsedDir,
    Symlink,
}

/// A single node in the workspace tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// POSIX-style relative path from workspace root (no leading `./`).
    /// Empty string for the root node.
    pub rel_path: String,
    pub kind: NodeKind,
    /// Basename.
    pub name: String,
    /// Lowercased extension without dot (files only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    /// Children for expanded dirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<TreeNode>>,
    /// Recursive non-ignored file count (dirs / collapsed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u32>,
    /// Recursive non-ignored dir count under this node (not including self).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir_count: Option<u32>,
    /// Sample basenames for collapsed dirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<String>>,
    /// Cheap role tags (source, test, docs, asset, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_tags: Option<Vec<String>>,
    /// Optional size in bytes (config gated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Optional mtime in unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<u64>,
}

impl TreeNode {
    /// Create a file node.
    pub fn file(rel_path: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        let ext = extension_of(&name);
        Self {
            rel_path: rel_path.into(),
            kind: NodeKind::File,
            name,
            ext,
            children: None,
            file_count: None,
            dir_count: None,
            sample: None,
            role_tags: None,
            size_bytes: None,
            mtime_ms: None,
        }
    }

    /// Create an expanded directory node.
    pub fn dir(rel_path: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            rel_path: rel_path.into(),
            kind: NodeKind::Dir,
            name: name.into(),
            ext: None,
            children: Some(Vec::new()),
            file_count: Some(0),
            dir_count: Some(0),
            sample: None,
            role_tags: None,
            size_bytes: None,
            mtime_ms: None,
        }
    }

    /// Create a collapsed directory node.
    pub fn collapsed(
        rel_path: impl Into<String>,
        name: impl Into<String>,
        file_count: u32,
        sample: Vec<String>,
    ) -> Self {
        Self {
            rel_path: rel_path.into(),
            kind: NodeKind::CollapsedDir,
            name: name.into(),
            ext: None,
            children: None,
            file_count: Some(file_count),
            dir_count: None,
            sample: if sample.is_empty() {
                None
            } else {
                Some(sample)
            },
            role_tags: None,
            size_bytes: None,
            mtime_ms: None,
        }
    }

    /// Whether this node is a directory-like kind.
    pub fn is_dir_like(&self) -> bool {
        matches!(self.kind, NodeKind::Dir | NodeKind::CollapsedDir)
    }
}

/// Aggregate stats for a tree build.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stats {
    pub dirs: u32,
    pub files: u32,
    pub ignored_dirs: u32,
    pub collapsed_dirs: u32,
    pub bytes_seen: u64,
    pub truncated: bool,
}

/// Freshness of an index snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessState {
    #[default]
    Fresh,
    LikelyFresh,
    Stale,
    Building,
    Error,
    Missing,
}

/// Freshness payload stored in meta and returned from queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Freshness {
    pub state: FreshnessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    #[serde(default)]
    pub dirty_paths: u32,
}

impl Default for Freshness {
    fn default() -> Self {
        Self {
            state: FreshnessState::Fresh,
            basis: Some("full_walk".to_string()),
            dirty_paths: 0,
        }
    }
}

/// Optional git identity snapshot (Phase 1: best-effort, may be empty).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitInfo {
    pub present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_dir: Option<String>,
}

/// Build metadata for the last index run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    pub mode: String,
    pub duration_ms: u64,
    pub walker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

/// `meta.json` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub schema_version: u32,
    pub workspace_id: String,
    pub root: String,
    pub canonical_root: String,
    #[serde(default)]
    pub git: GitInfo,
    pub created_at: String,
    pub updated_at: String,
    pub build: BuildInfo,
    pub stats: Stats,
    pub freshness: Freshness,
    /// Detected stack markers (godot, rust, node, python, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_profile: Vec<String>,
}

/// On-disk / in-memory tree payload (`tree.v1.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreePayload {
    pub schema_version: u32,
    pub workspace_id: String,
    pub root: TreeNode,
    /// Basename â†’ relative paths (for resolve/search).
    #[serde(default)]
    pub name_index: HashMap<String, Vec<String>>,
}

/// Full in-memory index (meta + tree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeIndex {
    pub meta: Meta,
    pub root: TreeNode,
    #[serde(default)]
    pub name_index: HashMap<String, Vec<String>>,
}

impl TreeIndex {
    /// Build a [`TreePayload`] for serialization.
    pub fn to_payload(&self) -> TreePayload {
        TreePayload {
            schema_version: SCHEMA_VERSION,
            workspace_id: self.meta.workspace_id.clone(),
            root: self.root.clone(),
            name_index: self.name_index.clone(),
        }
    }

    /// Reconstruct from meta + payload.
    pub fn from_parts(meta: Meta, payload: TreePayload) -> Self {
        Self {
            meta,
            root: payload.root,
            name_index: payload.name_index,
        }
    }
}

/// Lowercased extension without dot, if any.
pub fn extension_of(name: &str) -> Option<String> {
    let path = std::path::Path::new(name);
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// Heuristic role tags from relative path + extension.
pub fn role_tags_for(rel_path: &str, ext: Option<&str>, is_dir: bool) -> Vec<String> {
    let lower = rel_path.replace('\\', "/").to_ascii_lowercase();
    let mut tags = Vec::new();

    let push = |tags: &mut Vec<String>, t: &str| {
        if !tags.iter().any(|x| x == t) {
            tags.push(t.to_string());
        }
    };

    if lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("_test.")
        || lower.contains("/test_")
        || lower.starts_with("test_")
    {
        push(&mut tags, "test");
    }
    if lower.starts_with("docs/")
        || lower.contains("/docs/")
        || ext == Some("md")
        || ext == Some("rst")
    {
        push(&mut tags, "docs");
    }
    if lower.starts_with("assets/")
        || lower.contains("/assets/")
        || matches!(
            ext,
            Some("glb")
                | Some("gltf")
                | Some("png")
                | Some("jpg")
                | Some("jpeg")
                | Some("wav")
                | Some("ogg")
                | Some("mp3")
                | Some("svg")
        )
    {
        push(&mut tags, "asset");
    }
    if lower.starts_with("scenes/") || lower.contains("/scenes/") || ext == Some("tscn") {
        push(&mut tags, "scene");
    }
    if lower.starts_with("third_party/")
        || lower.contains("/third_party/")
        || lower.starts_with("vendor/")
        || lower.contains("/vendor/")
        || lower.starts_with("addons/")
    {
        push(&mut tags, "vendor");
    }
    if lower.ends_with(".uid")
        || lower.ends_with(".import")
        || lower.contains(".generated.")
        || lower.contains("/generated/")
    {
        push(&mut tags, "generated");
    }
    if matches!(
        ext,
        Some("toml") | Some("json") | Some("yaml") | Some("yml") | Some("ini")
    ) || name_is_config_basename(rel_path)
    {
        push(&mut tags, "config");
    }
    if lower.starts_with("tools/") || lower.contains("/tools/") {
        push(&mut tags, "tool");
    }
    if matches!(
        ext,
        Some("rs")
            | Some("gd")
            | Some("ts")
            | Some("tsx")
            | Some("js")
            | Some("jsx")
            | Some("py")
            | Some("go")
            | Some("c")
            | Some("cpp")
            | Some("h")
            | Some("hpp")
            | Some("java")
            | Some("kt")
            | Some("swift")
    ) || lower.starts_with("src/")
        || lower.starts_with("scripts/")
        || lower.starts_with("crates/")
        || lower.contains("/src/")
        || lower.contains("/scripts/")
    {
        push(&mut tags, "source");
    }

    if is_dir && tags.is_empty() {
        // leave empty for generic dirs
    }
    tags
}

fn name_is_config_basename(rel_path: &str) -> bool {
    let name = rel_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(rel_path)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cargo.toml"
            | "package.json"
            | "project.godot"
            | "pyproject.toml"
            | "requirements.txt"
            | "tsconfig.json"
            | "makefile"
            | "dockerfile"
            | "rust-toolchain.toml"
            | "clippy.toml"
            | "rustfmt.toml"
    )
}

/// Detect workspace stack markers from root children names.
pub fn detect_workspace_profile(root_child_names: &[String]) -> Vec<String> {
    let lower: Vec<String> = root_child_names
        .iter()
        .map(|n| n.to_ascii_lowercase())
        .collect();
    let has = |n: &str| lower.iter().any(|x| x == n);
    let mut profile = Vec::new();
    if has("project.godot") {
        profile.push("godot".to_string());
    }
    if has("cargo.toml") {
        profile.push("rust".to_string());
    }
    if has("package.json") {
        profile.push("node".to_string());
    }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
        profile.push("python".to_string());
    }
    if has("go.mod") {
        profile.push("go".to_string());
    }
    profile
}

/// Convert a platform path to POSIX relative form.
pub fn to_posix_rel(path: &std::path::Path) -> String {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

