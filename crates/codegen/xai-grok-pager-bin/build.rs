use std::process::Command;

/// The workspace `VERSION` file — single source of truth for the wire version.
fn workspace_version() -> Option<String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let version_file = std::path::PathBuf::from(manifest)
        .parent()?
        .parent()?
        .parent()?
        .join("VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());
    let raw = std::fs::read_to_string(&version_file).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed=GROK_VERSION");

    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Same precedence as xai-grok-version's build.rs: CI tag, then the workspace
    // VERSION file, then this crate's own version. Reading VERSION keeps
    // `turbo --version` and the Agent Boot Card from drifting apart.
    let version = std::env::var("GROK_VERSION")
        .ok()
        .or_else(workspace_version)
        .or_else(|| std::env::var("CARGO_PKG_VERSION").ok())
        .unwrap_or_else(|| "0.0.0".to_string());

    println!(
        "cargo:rustc-env=VERSION_WITH_COMMIT={} ({})",
        version, commit
    );
}
