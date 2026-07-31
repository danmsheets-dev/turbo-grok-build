//! Defense-in-depth: `connect_or_spawn` must refuse when a process confine
//! root (`--confine`) is active, before any socket discovery or leader spawn.
//!
//! Mirrors `test_leader_sandbox_confinement.rs` for the path-prefix confine
//! boundary (sandbox profile is a separate veto).

use xai_grok_shell::leader::{
    ClientCapabilities, ClientMode, ConnectionError, LeaderEnvUrls, connect_or_spawn,
};

#[tokio::test]
async fn connect_or_spawn_refuses_when_process_confine_root_set() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = dunce::canonicalize(tmp.path()).expect("canonicalize");
    xai_grok_tools::types::resources::set_process_confine_root(root.clone());

    let env_urls = LeaderEnvUrls {
        // Guard returns before LeaderLock / socket paths touch the filesystem.
        grok_ws_url: "wss://test.invalid/process-confinement".into(),
        grok_ws_origin: "https://test.invalid".into(),
    };

    let err = match connect_or_spawn(
        "test-process-confinement",
        ClientMode::Stdio,
        &env_urls,
        ClientCapabilities::default(),
    )
    .await
    {
        Ok(_) => panic!(
            "confined client must not adopt or spawn a leader (connect_or_spawn returned Ok)"
        ),
        Err(err) => err,
    };

    match err {
        ConnectionError::ProcessConfinement(path) => {
            assert!(
                path.contains(&root.display().to_string())
                    || path_is_same_root(&path, &root),
                "error must name the confine root, got: {path}"
            );
        }
        other => panic!("expected ProcessConfinement, got {other:?}"),
    }
}

fn path_is_same_root(reported: &str, root: &std::path::Path) -> bool {
    let reported = std::path::Path::new(reported);
    xai_grok_tools::types::resources::path_is_under_confine_root(reported, root)
        && xai_grok_tools::types::resources::path_is_under_confine_root(root, reported)
}
