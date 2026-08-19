use std::path::PathBuf;

/// Resolve the wire version, in priority order:
///
/// 1. `GROK_VERSION` from the environment (CI sets this from the release tag).
/// 2. The workspace `VERSION` file — the single source of truth for a release.
/// 3. Nothing, leaving `lib.rs` to fall back to `CARGO_PKG_VERSION`.
///
/// Step 2 exists because this crate's own `CARGO_PKG_VERSION` silently drifted:
/// `VERSION` and `xai-grok-pager-bin` moved to 1.0.0-rc.2 while this crate stayed
/// at rc.1, so `turbo --version` and the Agent Boot Card disagreed about which
/// build the user was running.
fn workspace_version() -> Option<String> {
    // build.rs runs with CWD = this crate's dir: crates/codegen/xai-grok-version.
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let version_file = PathBuf::from(manifest)
        .parent()? // crates/codegen
        .parent()? // crates
        .parent()? // workspace root
        .join("VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());
    let raw = std::fs::read_to_string(&version_file).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=GROK_VERSION");
    // Forward into rustc so `option_env!("GROK_VERSION")` in lib.rs sees the
    // release-tag version set by CI (`GROK_VERSION=… cargo build …`).
    if let Ok(v) = std::env::var("GROK_VERSION") {
        println!("cargo:rustc-env=GROK_VERSION={v}");
    } else if let Some(v) = workspace_version() {
        println!("cargo:rustc-env=GROK_VERSION={v}");
    }
}
