//! `/steer` and `/rollback` dispatch.

use super::ctx::get_active_agent_mut;
use super::interject::dispatch_interject;
use super::prompt::dispatch_send_prompt;
use super::queue::push_and_page_flip;
use crate::app::actions::Effect;
use crate::app::agent::AgentId;
use crate::app::app_view::{ActiveView, AppView};
use crate::scrollback::block::RenderBlock;

/// Inject into a running turn; otherwise send as a normal prompt.
///
/// Send-now (`Action::SendPromptNow`) already exists as cancel-and-send.
/// `/steer` does not cancel: a running turn gets [`dispatch_interject`].
pub(super) fn dispatch_steer(app: &mut AppView, text: String) -> Vec<Effect> {
    let running = match app.active_view {
        ActiveView::Agent(id) => app
            .agents
            .get(&id)
            .is_some_and(|agent| agent.session.state.is_turn_running()),
        _ => false,
    };
    if running {
        dispatch_interject(app, text, Vec::new())
    } else {
        dispatch_send_prompt(app, text)
    }
}

/// Queue an async restore of the last (or named) undoable edit receipt.
pub(super) fn dispatch_rollback_last(app: &mut AppView, receipt_id: Option<String>) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        agent.show_toast("No active session");
        return vec![];
    };
    vec![Effect::RollbackLast {
        agent_id: id,
        session_id,
        cwd: agent.session.cwd.clone(),
        receipt_id,
    }]
}

pub(super) fn handle_rollback_complete(
    app: &mut AppView,
    agent_id: AgentId,
    message: String,
) -> Vec<Effect> {
    if let Some(agent) = app.agents.get_mut(&agent_id) {
        push_and_page_flip(&mut agent.scrollback, RenderBlock::system(message));
    } else if let Some(agent) = get_active_agent_mut(app) {
        push_and_page_flip(&mut agent.scrollback, RenderBlock::system(message));
    }
    vec![]
}
