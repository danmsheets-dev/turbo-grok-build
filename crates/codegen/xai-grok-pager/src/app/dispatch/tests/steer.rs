//! Parse/dispatch tests for `/steer` and `/rollback`.

use super::*;
use crate::app::actions::{Action, Effect, TaskResult};
use crate::app::agent::AgentState;

#[test]
fn steer_idle_sends_as_prompt() {
    let mut app = test_app_with_agent();
    let effects = dispatch(Action::Steer("stay inside crates/foo".into()), &mut app);
    assert!(
        effects.iter().any(
            |e| matches!(e, Effect::SendPrompt { text, .. } if text == "stay inside crates/foo")
        ),
        "idle /steer must send a prompt, got {effects:?}"
    );
}

#[test]
fn steer_running_injects_without_cancel() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents.get_mut(&id).unwrap().session.state = AgentState::TurnRunning;
    let effects = dispatch(Action::Steer("also check Windows paths".into()), &mut app);
    assert!(
        matches!(effects.as_slice(), [Effect::SendInterject { text, .. }] if text == "also check Windows paths"),
        "running /steer must interject, got {effects:?}"
    );
}

#[test]
fn rollback_last_emits_effect() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let effects = dispatch(Action::RollbackLast { receipt_id: None }, &mut app);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::RollbackLast {
                agent_id,
                receipt_id: None,
                ..
            }] if *agent_id == id
        ),
        "expected RollbackLast effect, got {effects:?}"
    );
}

#[test]
fn rollback_named_id_emits_effect() {
    let mut app = test_app_with_agent();
    let effects = dispatch(
        Action::RollbackLast {
            receipt_id: Some("rcpt-abc".into()),
        },
        &mut app,
    );
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::RollbackLast {
                receipt_id: Some(id),
                ..
            }] if id == "rcpt-abc"
        ),
        "expected named RollbackLast, got {effects:?}"
    );
}

#[test]
fn rollback_no_session_toasts() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    app.agents.get_mut(&id).unwrap().session.session_id = None;
    let effects = dispatch(Action::RollbackLast { receipt_id: None }, &mut app);
    assert!(effects.is_empty(), "no session must not emit effects");
    assert_eq!(
        app.agents[&id].toast.as_ref().map(|(s, _)| s.as_str()),
        Some("No active session")
    );
}

#[test]
fn rollback_complete_commits_system_block() {
    let mut app = test_app_with_agent();
    let id = AgentId(0);
    let _ = dispatch(
        Action::TaskComplete(TaskResult::RollbackComplete {
            agent_id: id,
            message: "No undoable edit receipts in this session.".into(),
        }),
        &mut app,
    );
    assert!(
        last_system_text(&app, id).contains("No undoable edit receipts"),
        "rollback result must land as a system block"
    );
}
