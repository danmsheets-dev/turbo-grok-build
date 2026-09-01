//! `/folder add|remove|list` dispatchers.

use std::path::{Path, PathBuf};

use crate::app::actions::Effect;
use crate::app::app_view::AppView;
use crate::app::dispatch::ctx::get_active_agent_mut;
use crate::scrollback::block::RenderBlock;

pub(super) fn dispatch_folder_list(app: &mut AppView) -> Vec<Effect> {
    let msg = format_folder_list(&app.additional_directories);
    push_folder_message(app, &msg);
    vec![]
}

pub(super) fn dispatch_folder_add(app: &mut AppView, path: PathBuf) -> Vec<Effect> {
    if app
        .additional_directories
        .iter()
        .any(|e| paths_equal(e, &path))
    {
        push_folder_message(
            app,
            &format!("Already attached: {}", path.display()),
        );
        return vec![];
    }
    if paths_equal(&app.cwd, &path) || path.starts_with(&app.cwd) {
        push_folder_message(
            app,
            &format!(
                "Skipped (same as or inside primary cwd): {}",
                path.display()
            ),
        );
        return vec![];
    }
    app.additional_directories.push(path.clone());
    let msg = format!("Attached folder: {}", path.display());
    push_folder_message(app, &msg);
    live_sync_effect(app)
}

pub(super) fn dispatch_folder_remove(app: &mut AppView, query: String) -> Vec<Effect> {
    let q = query.trim();
    let q_path = PathBuf::from(q);
    let before = app.additional_directories.len();
    app.additional_directories.retain(|p| {
        !paths_equal(p, &q_path)
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|n| !n.eq_ignore_ascii_case(q))
    });
    if app.additional_directories.len() == before {
        push_folder_message(app, &format!("No attached folder matching `{q}`"));
        return vec![];
    }
    push_folder_message(app, &format!("Detached folder: {q}"));
    live_sync_effect(app)
}

fn live_sync_effect(app: &AppView) -> Vec<Effect> {
    let Some(session_id) = crate::app::dispatch::ctx::active_agent_session_id(app) else {
        return vec![];
    };
    vec![Effect::SetAdditionalDirectories {
        session_id,
        directories: app.additional_directories.clone(),
    }]
}

fn push_folder_message(app: &mut AppView, msg: &str) {
    if let Some(agent) = get_active_agent_mut(app) {
        agent
            .scrollback
            .push_block(RenderBlock::system(msg.to_string()));
        return;
    }
    if let Some(dashboard) = app.dashboard.as_mut() {
        dashboard.set_error_toast(msg);
        return;
    }
    app.show_toast(msg);
}

pub(crate) fn format_folder_list(dirs: &[PathBuf]) -> String {
    if dirs.is_empty() {
        return "No extra folders attached. Use `/folder add <path>` or `--add-dir`."
            .to_string();
    }
    let mut lines = vec![format!("Attached folders ({})", dirs.len())];
    for dir in dirs {
        lines.push(format!("  - {}", dir.display()));
    }
    lines.join("\n")
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    a == b
        || dunce::canonicalize(a).ok().zip(dunce::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

pub(super) fn handle_additional_directories_updated(
    app: &mut AppView,
    directories: Vec<PathBuf>,
    error: Option<String>,
) -> Vec<Effect> {
    if let Some(err) = error {
        push_folder_message(app, &format!("Could not apply extra folders: {err}"));
        return vec![];
    }
    app.additional_directories = directories;
    vec![]
}
