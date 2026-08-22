//! Dispatch coverage for turbo-only session_admin ACP methods.
//!
//! The pager sends `x.ai/internal/set_platform_api_key` (`/providers`) and
//! `x.ai/internal/reload_subagent_models` (`/agents` pin + config watcher).
//! Those names are **not** [`InternalMethod`] variants, so `MvpAgent::ext_method`
//! must match them by string or they fall through to
//! `unknown ACP extension method`.

use super::build_minimal_agent_for_tests;
use crate::extensions::session_admin::{
    PAGER_SENT_TURBO_INTERNAL_METHODS, is_turbo_only_internal_method,
};
use crate::leader::protocol::InternalMethod;
use agent_client_protocol as acp;
use serde_json::json;

fn ext_request(method: &str, params: serde_json::Value) -> acp::ExtRequest {
    acp::ExtRequest::new(
        method,
        std::sync::Arc::from(serde_json::value::to_raw_value(&params).unwrap()),
    )
}

fn unknown_method_data(err: &acp::Error) -> bool {
    err.data
        .as_ref()
        .and_then(crate::sampling::error::error_detail_from_data)
        .or_else(|| Some(err.to_string()))
        .is_some_and(|msg| msg.contains("unknown ACP extension method"))
}

#[test]
fn pager_sent_internal_methods_table_matches_agent_session_admin_match() {
    for method in PAGER_SENT_TURBO_INTERNAL_METHODS {
        assert!(
            is_turbo_only_internal_method(method),
            "{method} must be routed by name in session_admin / ext_method"
        );
        assert!(
            InternalMethod::from_name(method).is_none(),
            "{method} must stay out of InternalMethod (see protocol.rs)"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
#[serial_test::serial]
async fn ext_method_routes_pager_sent_turbo_internal_methods() {
    use acp::Agent as _;
    use xai_grok_test_support::EnvGuard;

    let grok_home = tempfile::tempdir().unwrap();
    let _home = EnvGuard::set("GROK_HOME", grok_home.path());
    let _auth_path = EnvGuard::unset("GROK_AUTH_PATH");
    let _byok = xai_grok_test_support::unset_all_byok_platform_api_key_envs();

    let agent = build_minimal_agent_for_tests();

    let reload = agent
        .ext_method(ext_request(
            "x.ai/internal/reload_subagent_models",
            json!({}),
        ))
        .await
        .expect("reload_subagent_models must dispatch to session_admin");
    let reload_body: serde_json::Value = serde_json::from_str(reload.0.get()).unwrap();
    let reload_result = reload_body
        .get("result")
        .cloned()
        .unwrap_or(reload_body.clone());
    assert_eq!(reload_result.get("reloaded"), Some(&json!(true)));

    let saved = agent
        .ext_method(ext_request(
            "x.ai/internal/set_platform_api_key",
            json!({
                "platform": "openrouter",
                "apiKey": "sk-or-test",
            }),
        ))
        .await
        .expect("set_platform_api_key must dispatch to session_admin, not unknown method");
    let saved_body: serde_json::Value = serde_json::from_str(saved.0.get()).unwrap();
    let saved_result = saved_body
        .get("result")
        .cloned()
        .unwrap_or(saved_body.clone());
    assert_eq!(saved_result.get("platform"), Some(&json!("openrouter")));
    assert_eq!(saved_result.get("cleared"), Some(&json!(false)));
    assert_eq!(
        crate::auth::read_platform_api_key(grok_home.path(), "openrouter").as_deref(),
        Some("sk-or-test")
    );

    let cleared = agent
        .ext_method(ext_request(
            "x.ai/internal/set_platform_api_key",
            json!({
                "platform": "openrouter",
                "apiKey": "",
            }),
        ))
        .await
        .expect("empty apiKey must dispatch as a clear");
    let cleared_body: serde_json::Value = serde_json::from_str(cleared.0.get()).unwrap();
    let cleared_result = cleared_body
        .get("result")
        .cloned()
        .unwrap_or(cleared_body.clone());
    assert_eq!(cleared_result.get("cleared"), Some(&json!(true)));
    assert!(crate::auth::read_platform_api_key(grok_home.path(), "openrouter").is_none());

    let unknown = agent
        .ext_method(ext_request(
            "x.ai/internal/definitely_not_a_method",
            json!({}),
        ))
        .await
        .expect_err("unknown internals still method_not_found");
    assert_eq!(unknown.code, acp::Error::method_not_found().code);
    assert!(
        unknown_method_data(&unknown),
        "unknown method must keep the dispatch-miss detail, got {unknown:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ext_method_set_platform_api_key_rejects_unknown_provider_not_unknown_method() {
    use acp::Agent as _;
    let agent = build_minimal_agent_for_tests();
    let err = agent
        .ext_method(ext_request(
            "x.ai/internal/set_platform_api_key",
            json!({
                "platform": "not-a-provider",
                "apiKey": "sk-or-test",
            }),
        ))
        .await
        .expect_err("unknown provider must be invalid_params after dispatch");
    assert_eq!(err.code, acp::Error::invalid_params().code);
    assert!(
        !unknown_method_data(&err),
        "must not fall through to unknown ACP extension method: {err:?}"
    );
    let detail = err
        .data
        .as_ref()
        .and_then(crate::sampling::error::error_detail_from_data)
        .unwrap_or_default();
    assert!(
        detail.contains("unknown provider"),
        "real handler error must surface, got {detail:?}"
    );
}
