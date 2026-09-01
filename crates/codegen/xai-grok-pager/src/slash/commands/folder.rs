//! `/folder add|remove|list` — attach extra workspace roots.
//!
//! `cwd` stays the primary workspace. Extra folders expand the write confine
//! set (ACP `additionalDirectories`). Alias of CLI `--add-dir`.

use std::path::{Path, PathBuf};

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct FolderCommand;

impl SlashCommand for FolderCommand {
    fn name(&self) -> &str {
        "folder"
    }

    fn aliases(&self) -> &[&str] {
        &["folders"]
    }

    fn description(&self) -> &str {
        "Attach extra folders (add, remove, list)"
    }

    fn usage(&self) -> &str {
        "/folder [list|add <path>|remove <path>]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("[list|add <path>|remove <path>]")
    }

    fn suggest_args(&self, _ctx: &AppCtx, args_query: &str) -> Option<Vec<ArgItem>> {
        let q = args_query.trim().to_ascii_lowercase();
        let options = [
            ("list", "Show attached extra folders"),
            ("add", "Attach a folder"),
            ("remove", "Detach a folder"),
        ];
        let items: Vec<ArgItem> = options
            .into_iter()
            .filter(|(name, _)| q.is_empty() || name.starts_with(&q) || name.contains(&q))
            .map(|(name, desc)| ArgItem::new(name, name, name, desc))
            .collect();
        (!items.is_empty()).then_some(items)
    }

    fn offered_when_session_less(&self) -> bool {
        true
    }

    fn run(&self, ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        let (verb, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((v, r)) => (v.to_ascii_lowercase(), r.trim()),
            None => {
                if trimmed.is_empty() {
                    ("list".to_string(), "")
                } else {
                    (trimmed.to_ascii_lowercase(), "")
                }
            }
        };
        match verb.as_str() {
            "list" | "ls" => CommandResult::Action(Action::FolderList),
            "add" => {
                if rest.is_empty() {
                    return CommandResult::Error(
                        "Usage: /folder add <path>".into(),
                    );
                }
                match resolve_folder_path(rest, ctx.session_cwd) {
                    Ok(path) => CommandResult::Action(Action::FolderAdd { path }),
                    Err(e) => CommandResult::Error(e),
                }
            }
            "remove" | "rm" | "detach" => {
                if rest.is_empty() {
                    return CommandResult::Error(
                        "Usage: /folder remove <path>".into(),
                    );
                }
                CommandResult::Action(Action::FolderRemove {
                    path: rest.to_string(),
                })
            }
            other => CommandResult::Error(format!(
                "Unknown /folder verb `{other}`. Use list, add <path>, or remove <path>."
            )),
        }
    }
}

fn resolve_folder_path(raw: &str, session_cwd: Option<&Path>) -> Result<PathBuf, String> {
    let expanded = expand_tilde(raw);
    let path = PathBuf::from(&expanded);
    let abs = if path.is_absolute() {
        path
    } else {
        let base = session_cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| "cannot resolve relative path: no cwd".to_string())?;
        base.join(path)
    };
    let meta = std::fs::metadata(&abs).map_err(|e| {
        format!("path `{}` does not exist or is inaccessible: {e}", abs.display())
    })?;
    if !meta.is_dir() {
        return Err(format!("path `{}` is not a directory", abs.display()));
    }
    dunce::canonicalize(&abs)
        .map_err(|e| format!("failed to canonicalize `{}`: {e}", abs.display()))
}

fn expand_tilde(raw: &str) -> String {
    if raw == "~" {
        return dirs::home_dir()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    if let Some(rest) = raw.strip_prefix("~\\") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;

    fn ctx<'a>(models: &'a ModelState, bundle: &'a BundleState) -> CommandExecCtx<'a> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: bundle,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            session_cwd: None,
            pager_state: crate::settings::PagerLocalSnapshot {
                multiline_mode: false,
                yolo_mode: false,
                ..crate::settings::PagerLocalSnapshot::default()
            },
        }
    }

    #[test]
    fn empty_args_lists() {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        assert!(matches!(
            FolderCommand.run(&mut c, ""),
            CommandResult::Action(Action::FolderList)
        ));
    }

    #[test]
    fn list_verb() {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        assert!(matches!(
            FolderCommand.run(&mut c, "list"),
            CommandResult::Action(Action::FolderList)
        ));
    }

    #[test]
    fn add_missing_path_errors() {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match FolderCommand.run(&mut c, "add") {
            CommandResult::Error(msg) => assert!(msg.contains("Usage")),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_verb_errors() {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        match FolderCommand.run(&mut c, "explode") {
            CommandResult::Error(msg) => assert!(msg.contains("Unknown")),
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn add_existing_dir() {
        let (models, bundle) = (ModelState::default(), BundleState::default());
        let mut c = ctx(&models, &bundle);
        let dir = tempfile::tempdir().unwrap();
        match FolderCommand.run(&mut c, &format!("add {}", dir.path().display())) {
            CommandResult::Action(Action::FolderAdd { path }) => {
                assert!(path.is_absolute());
            }
            other => panic!("expected FolderAdd, got {other:?}"),
        }
    }
}
