//! Typed configuration defaults for workspace tree (Phase 1 subset).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Inject card verbosity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InjectMode {
    /// No inject card.
    Off,
    /// Root + stack + top-level dirs only.
    Minimal,
    /// Source map + collapsed notes + freshness (default).
    #[default]
    Standard,
    /// Richer histograms and entry files.
    Rich,
}

impl InjectMode {
    /// Parse env / CLI tokens: `off|minimal|standard|rich` (also `0`/`1` for off/standard).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" | "none" => Some(Self::Off),
            "minimal" | "min" | "child" | "subagent" => Some(Self::Minimal),
            "standard" | "default" | "1" | "true" | "on" | "yes" => Some(Self::Standard),
            "rich" | "full" => Some(Self::Rich),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Standard => "standard",
            Self::Rich => "rich",
        }
    }
}

/// Phase 1 config subset for walk, collapse, store, and inject.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceTreeConfig {
    /// Master switch.
    pub enabled: bool,
    /// Optional override for `~/.grok/workspace-trees`.
    pub store_dir: Option<PathBuf>,
    /// Walk caps.
    pub walk: WalkConfig,
    /// Collapse policy.
    pub collapse: CollapseConfig,
    /// Inject card policy.
    pub inject: InjectConfig,
    /// Extra hard-exclude basenames (merged with built-ins).
    pub ignore_extra: Vec<String>,
}

impl Default for WorkspaceTreeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            store_dir: None,
            walk: WalkConfig::default(),
            collapse: CollapseConfig::default(),
            inject: InjectConfig::default(),
            ignore_extra: Vec::new(),
        }
    }
}

impl WorkspaceTreeConfig {
    /// Load defaults then apply process env overrides (Phase 1).
    ///
    /// | Env | Effect |
    /// |-----|--------|
    /// | `GROK_WORKSPACE_TREE` / `TURBO_TREE` | `0\|false\|off` disables; `1\|true\|on` enables |
    /// | `GROK_WORKSPACE_TREE_INJECT` / `TURBO_TREE_INJECT` | `off\|minimal\|standard\|rich` |
    /// | `GROK_WORKSPACE_TREE_STORE_DIR` / `TURBO_TREE_STORE_DIR` | store root override |
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        if let Some(v) = env_first(&["GROK_WORKSPACE_TREE", "TURBO_TREE"]) {
            let s = v.trim().to_ascii_lowercase();
            match s.as_str() {
                "0" | "false" | "off" | "no" | "disabled" => cfg.enabled = false,
                "1" | "true" | "on" | "yes" | "enabled" => cfg.enabled = true,
                _ => {}
            }
        }

        if let Some(v) = env_first(&["GROK_WORKSPACE_TREE_INJECT", "TURBO_TREE_INJECT"]) {
            if let Some(mode) = InjectMode::parse(&v) {
                cfg.inject.mode = mode;
            }
        }

        if let Some(v) = env_first(&["GROK_WORKSPACE_TREE_STORE_DIR", "TURBO_TREE_STORE_DIR"]) {
            let p = v.trim();
            if !p.is_empty() {
                cfg.store_dir = Some(PathBuf::from(p));
            }
        }

        cfg
    }

    /// Inject mode for a session audience.
    ///
    /// Explicit `GROK_WORKSPACE_TREE_INJECT` / `TURBO_TREE_INJECT` always wins.
    /// Otherwise subagents prefer **minimal**; primary sessions use config/default
    /// (**standard**).
    pub fn inject_mode_for_audience(&self, is_subagent: bool) -> InjectMode {
        if env_first(&["GROK_WORKSPACE_TREE_INJECT", "TURBO_TREE_INJECT"]).is_some() {
            return self.inject.mode;
        }
        if is_subagent {
            if matches!(self.inject.mode, InjectMode::Off) {
                InjectMode::Off
            } else {
                InjectMode::Minimal
            }
        } else {
            self.inject.mode
        }
    }
}

fn env_first(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Walk budget defaults (design §9.3 / §22).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkConfig {
    pub max_files: u32,
    pub max_dirs: u32,
    pub max_depth: u32,
    pub max_expand_depth: u32,
    pub max_duration_ms: u64,
    pub follow_symlinks: bool,
    pub use_gitignore: bool,
    pub use_global_gitignore: bool,
    pub collect_mtime: bool,
    pub collect_size: bool,
}

impl Default for WalkConfig {
    fn default() -> Self {
        Self {
            max_files: 250_000,
            max_dirs: 100_000,
            max_depth: 32,
            max_expand_depth: 8,
            max_duration_ms: 15_000,
            follow_symlinks: false,
            use_gitignore: true,
            use_global_gitignore: true,
            collect_mtime: true,
            collect_size: false,
        }
    }
}

/// Collapse policy defaults (design §6.3 / §9.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollapseConfig {
    /// Directory basenames that collapse (counts + samples only).
    pub names: Vec<String>,
    /// Relative path globs that force collapse (simple `**` / `*` matching).
    pub globs: Vec<String>,
    /// Collapse when recursive file count under a dir exceeds this.
    pub max_files_per_dir: u32,
    /// Sample names kept on collapsed nodes.
    pub sample_names: u32,
}

impl Default for CollapseConfig {
    fn default() -> Self {
        Self {
            names: default_collapse_names(),
            globs: default_collapse_globs(),
            max_files_per_dir: 80,
            sample_names: 5,
        }
    }
}

/// Inject card defaults (design §8.1 / §9.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectConfig {
    pub mode: InjectMode,
    /// Character budget approximating token budget (`max_tokens * 4`).
    pub max_tokens: u32,
    pub max_top_dirs: u32,
    pub include_entrypoints: bool,
}

impl Default for InjectConfig {
    fn default() -> Self {
        Self {
            mode: InjectMode::Standard,
            max_tokens: 2500,
            max_top_dirs: 24,
            include_entrypoints: true,
        }
    }
}

impl InjectConfig {
    /// Approximate character budget from `max_tokens` (4 chars/token heuristic).
    pub fn max_chars(&self) -> usize {
        (self.max_tokens as usize).saturating_mul(4)
    }
}

/// Built-in hard-exclude directory basenames (design §6.2).
pub fn default_hard_exclude_names() -> Vec<String> {
    [
        ".git",
        ".godot",
        "node_modules",
        "target",
        "dist",
        "build",
        ".venv",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".tox",
        ".idea",
        ".vs",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Built-in hard-exclude file extensions (no leading dot, lowercased).
pub fn default_hard_exclude_exts() -> Vec<String> {
    ["pyc", "pdb", "dll", "exe"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn default_collapse_names() -> Vec<String> {
    // Asset-heavy kit folders often appear by basename; node_modules/target are
    // hard-excluded and never reached. Keep a short list for non-hard names.
    ["vendor", "third_party", "addons"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn default_collapse_globs() -> Vec<String> {
    [
        "assets/models/**",
        "assets/terrain/**",
        "**/*.import",
        ".godot/**",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Effective hard-exclude basenames for a config (built-in + extra).
pub fn effective_hard_exclude_names(config: &WorkspaceTreeConfig) -> Vec<String> {
    let mut names = default_hard_exclude_names();
    for extra in &config.ignore_extra {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(extra)) {
            names.push(extra.clone());
        }
    }
    names
}
