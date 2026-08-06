//! Integration tests against `tests/fixtures/basic`.

use std::path::{Path, PathBuf};
use xai_workspace_tree::{
    build_and_save, build_index, inject_card, list, load_index_for_root, resolve_path, search,
    summary, CollapseConfig, InjectMode, NodeKind, WorkspaceTreeConfig,
};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
}

fn test_config(store: &Path) -> WorkspaceTreeConfig {
    let mut cfg = WorkspaceTreeConfig::default();
    cfg.store_dir = Some(store.to_path_buf());
    // Force collapse of bulk/ via max_files_per_dir.
    cfg.collapse.max_files_per_dir = 50;
    // Collapse assets/models via glob (default).
    cfg
}

#[test]
fn builds_index_and_honors_hard_excludes() {
    let root = fixture_root();
    assert!(root.join("project.godot").exists());

    let cfg = WorkspaceTreeConfig::default();
    let index = build_index(&root, &cfg).expect("build_index");

    // Hard excludes: node_modules and target must not appear as children.
    let top = index.root.children.as_ref().expect("children");
    let names: Vec<&str> = top.iter().map(|n| n.name.as_str()).collect();
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("node_modules")),
        "node_modules should be hard-excluded, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("target")),
        "target should be hard-excluded, got {names:?}"
    );

    // Expected present dirs/files.
    assert!(names.iter().any(|n| *n == "scripts" || *n == "src" || *n == "docs"));
    assert!(index.meta.workspace_profile.contains(&"godot".to_string()));
    assert!(index.meta.workspace_profile.contains(&"rust".to_string()));
    assert!(index.meta.stats.files > 0);
    assert!(!index.meta.workspace_id.is_empty());
}

#[test]
fn collapse_by_max_files_and_glob() {
    let root = fixture_root();
    let mut cfg = WorkspaceTreeConfig::default();
    cfg.collapse.max_files_per_dir = 50;
    let index = build_index(&root, &cfg).expect("build");

    // bulk/ has 100 files â†’ collapsed
    let bulk = index
        .root
        .children
        .as_ref()
        .and_then(|c| c.iter().find(|n| n.name == "bulk"))
        .expect("bulk dir");
    assert_eq!(bulk.kind, NodeKind::CollapsedDir);
    assert!(bulk.file_count.unwrap_or(0) >= 50);
    assert!(bulk.sample.as_ref().map(|s| !s.is_empty()).unwrap_or(false));

    // assets/models should collapse via default glob
    let models = index
        .root
        .children
        .as_ref()
        .and_then(|c| c.iter().find(|n| n.name == "assets"))
        .and_then(|a| a.children.as_ref())
        .and_then(|c| c.iter().find(|n| n.name == "models"))
        .expect("assets/models should exist");
    assert_eq!(
        models.kind,
        NodeKind::CollapsedDir,
        "assets/models should be collapsed"
    );
}

#[test]
fn resolve_path_finds_ship_roster() {
    let root = fixture_root();
    let index = build_index(&root, &WorkspaceTreeConfig::default()).unwrap();
    let res = resolve_path(&index, "ship_roster", Some("scripts/ship/ship_roster.gd"), 8);
    assert!(
        !res.hits.is_empty(),
        "expected hits for ship_roster, name_index keys sample: {:?}",
        index.name_index.keys().take(20).collect::<Vec<_>>()
    );
    assert!(
        res.hits
            .iter()
            .any(|h| h.rel_path.contains("ship_roster")),
        "hits={:?}",
        res.hits
    );
    assert!(res.hits[0].score >= 0.85);
}

#[test]
fn search_and_list_work() {
    let root = fixture_root();
    let index = build_index(&root, &WorkspaceTreeConfig::default()).unwrap();

    let s = search(&index, "util", 10);
    assert!(
        s.hits.iter().any(|h| h.rel_path.contains("util")),
        "hits={:?}",
        s.hits
    );

    let listed = list(&index, "scripts", 1, 50).expect("list scripts");
    assert!(
        listed.entries.iter().any(|e| e.name == "core" || e.name.contains("ship")),
        "entries={:?}",
        listed.entries
    );

    let summ = summary(&index, 24);
    assert_eq!(summ.workspace_id, index.meta.workspace_id);
    assert!(!summ.top_level.is_empty());
}

#[test]
fn save_load_roundtrip() {
    let root = fixture_root();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = test_config(tmp.path());

    let built = build_and_save(&root, &cfg).expect("build_and_save");
    let loaded = load_index_for_root(&root, &cfg).expect("load");
    assert_eq!(built.meta.workspace_id, loaded.meta.workspace_id);
    assert_eq!(built.meta.stats.files, loaded.meta.stats.files);
    assert_eq!(built.root.children.as_ref().map(|c| c.len()), loaded.root.children.as_ref().map(|c| c.len()));

    // store files exist
    let store_dir = tmp.path().join(&built.meta.workspace_id);
    assert!(store_dir.join("meta.json").exists());
    assert!(store_dir.join("tree.v1.json").exists());
}

#[test]
fn inject_card_is_budgeted() {
    let root = fixture_root();
    let mut cfg = WorkspaceTreeConfig::default();
    cfg.inject.mode = InjectMode::Standard;
    cfg.inject.max_tokens = 200; // ~800 chars
    let index = build_index(&root, &cfg).unwrap();
    let card = inject_card(&index, &cfg);
    assert!(card.contains("Workspace tree"));
    assert!(card.contains("resolve_path"));
    assert!(card.len() <= cfg.inject.max_chars() + 40);
}

#[test]
fn vendor_collapse_by_name() {
    let root = fixture_root();
    let cfg = WorkspaceTreeConfig::default();
    // ensure vendor is in collapse names (default)
    assert!(cfg.collapse.names.iter().any(|n| n == "vendor"));
    let index = build_index(&root, &cfg).unwrap();
    let vendor = index
        .root
        .children
        .as_ref()
        .and_then(|c| c.iter().find(|n| n.name == "vendor"));
    if let Some(v) = vendor {
        assert_eq!(v.kind, NodeKind::CollapsedDir);
    }
}

#[test]
fn disabled_config_errors() {
    let root = fixture_root();
    let mut cfg = WorkspaceTreeConfig::default();
    cfg.enabled = false;
    let err = build_index(&root, &cfg).unwrap_err();
    assert!(matches!(err, xai_workspace_tree::Error::Disabled));
}

/// RC13 P1 F6/F7: files under a directory that collapses for display must still
/// be in the name index (index is built pre-collapse).
#[test]
fn name_index_survives_display_collapse() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    // Create > max_files_per_dir files under scripts/ so it collapses.
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    let mut cfg = WorkspaceTreeConfig::default();
    cfg.collapse.max_files_per_dir = 5;
    cfg.collapse.sample_names = 3;
    for i in 0..20 {
        std::fs::write(scripts.join(format!("mod_{i}.gd")), b"x").unwrap();
    }
    // Nested unique name under scripts/
    std::fs::create_dir_all(scripts.join("core")).unwrap();
    std::fs::write(scripts.join("core/ship_roster.gd"), b"pass").unwrap();

    let index = build_index(root, &cfg).unwrap();
    // Nested file must resolve even if scripts/ is collapsed in the display tree.
    let hits = resolve_path(&index, "ship_roster", None, 8);
    assert!(
        hits.hits.iter().any(|h| h.rel_path.contains("ship_roster")),
        "expected ship_roster in name index after collapse; hits={:?}",
        hits.hits
    );
}

// silence unused import in some rustc versions
#[allow(dead_code)]
fn _collapse_type(_: CollapseConfig) {}

