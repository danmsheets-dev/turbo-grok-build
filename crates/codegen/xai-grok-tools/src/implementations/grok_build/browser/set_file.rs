//! `browser_set_file` — set `<input type=file>` from a workspace or session path.

use std::path::{Path, PathBuf};

use xai_grok_browser::path_is_under_session_folder;

use crate::types::output::ToolOutput;
use crate::types::requirements::{Expr, ToolRequirement};
use crate::types::resources::{ConfineRoot, path_is_under_confine_root};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_metadata::{resolve_cwd, shared_resources};

pub const BROWSER_SET_FILE_TOOL_NAME: &str = "browser_set_file";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BrowserSetFileInput {
    #[schemars(description = "Snapshot uid of a file input (e.g. \"4-17\").")]
    pub uid: String,
    #[schemars(
        description = "Workspace-relative or absolute path of an existing file. Workspace and \
            confine-root files are copied into the session uploads/ folder first; the host only \
            accepts session-folder paths. Paths outside those roots are refused here (not after a \
            host reject)."
    )]
    pub path: String,
}

#[derive(Debug, Default)]
pub struct BrowserSetFileTool;

impl crate::types::tool_metadata::ToolMetadata for BrowserSetFileTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Set a file input in the Turbo Agent WebView by snapshot uid. Path must be an existing file under the workspace, confine root, or session folder. Does not auto-submit. Page downloads are brokered into the session folder."
    }

    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}

impl xai_tool_runtime::Tool for BrowserSetFileTool {
    type Args = BrowserSetFileInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(BROWSER_SET_FILE_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            BROWSER_SET_FILE_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: false,
            tool_scope: Some(xai_tool_protocol::ToolScope::Write),
            ..Default::default()
        }
    }

    #[tracing::instrument(name = "tool.browser_set_file", skip_all, fields(uid = %input.uid))]
    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        input: BrowserSetFileInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        let handle = super::require_handle(&ctx).await?;
        let resources = shared_resources(&ctx)?;
        let cwd = resolve_cwd(&ctx, &resources).await?;
        let (confine, session_folder) = {
            let res = resources.lock().await;
            (
                res.get::<ConfineRoot>().map(|c| c.0.clone()),
                handle.session_folder().map(PathBuf::from),
            )
        };
        let canonical = resolve_upload_path(&cwd, &input.path)?;
        if !canonical.is_file() {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "browser_set_file: `{}` is not an existing file",
                canonical.display()
            )));
        }
        let mut roots: Vec<&Path> = vec![cwd.as_path()];
        if let Some(ref root) = confine {
            roots.push(root);
        }
        if let Some(ref folder) = session_folder {
            roots.push(folder);
        }
        if !roots
            .iter()
            .any(|root| path_is_under_confine_root(&canonical, root))
        {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "browser_set_file: `{}` is outside the workspace, confine root, and session folder",
                canonical.display()
            )));
        }
        let Some(folder) = session_folder.as_ref() else {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(
                "browser_set_file: no session folder is configured; workspace paths cannot be sent to the host",
            ));
        };
        let session_path = broker_into_session(folder, &canonical)?;
        if !path_is_under_session_folder(&session_path, folder) {
            return Err(xai_tool_runtime::ToolError::invalid_arguments(format!(
                "browser_set_file: `{}` is not under the session folder after broker; \
                 the host only accepts session-folder paths",
                session_path.display()
            )));
        }
        let path = session_path.to_string_lossy().into_owned();
        handle.set_file(input.uid.clone(), path.clone()).await?;
        Ok(super::text_output(format!(
            "Set file input {} to {}",
            input.uid, path
        )))
    }
}

fn broker_into_session(
    session_folder: &Path,
    src: &Path,
) -> Result<PathBuf, xai_tool_runtime::ToolError> {
    let session_canon =
        dunce::canonicalize(session_folder).unwrap_or_else(|_| session_folder.to_path_buf());
    if path_is_under_confine_root(src, &session_canon) {
        return Ok(src.to_path_buf());
    }
    let uploads = session_folder.join("uploads");
    std::fs::create_dir_all(&uploads).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_set_file: cannot create {}: {e}", uploads.display()),
        )
    })?;
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload.bin");
    let dest = unique_upload_path(&uploads, name);
    let src_meta = src.symlink_metadata().map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_set_file: cannot inspect {}: {e}", src.display()),
        )
    })?;
    if src_meta.file_type().is_symlink() || !src_meta.is_file() {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(
            "browser_set_file: source must be a regular file (symlinks are refused)",
        ));
    }
    std::fs::copy(src, &dest).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!(
                "browser_set_file: cannot broker {} into session: {e}",
                src.display()
            ),
        )
    })?;
    dunce::canonicalize(&dest).map_err(|e| {
        xai_tool_runtime::ToolError::custom(
            "browser_error",
            format!("browser_set_file: cannot resolve {}: {e}", dest.display()),
        )
    })
}

fn unique_upload_path(folder: &Path, filename: &str) -> PathBuf {
    let candidate = folder.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("upload");
    let extension = path.extension().and_then(|s| s.to_str());
    for index in 1..=10_000u32 {
        let name = match extension {
            Some(ext) => format!("{stem} ({index}).{ext}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = folder.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    folder.join("upload.bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_copies_workspace_file_into_session_uploads() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        let session = tmp.path().join("session");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&session).unwrap();
        let src = workspace.join("resume.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        let dest = broker_into_session(&session, &src).unwrap();
        assert!(dest.starts_with(&session));
        assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF");
        assert_ne!(dest, src);
        assert!(path_is_under_session_folder(&dest, &session));
    }

    #[test]
    fn workspace_path_outside_session_is_not_under_host_allowlist() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        let session = tmp.path().join("session");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&session).unwrap();
        let src = workspace.join("resume.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        assert!(
            !path_is_under_session_folder(&src, &session),
            "raw workspace path must fail the host allowlist so the tool brokers first"
        );
    }
}

fn resolve_upload_path(cwd: &Path, raw: &str) -> Result<PathBuf, xai_tool_runtime::ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(xai_tool_runtime::ToolError::invalid_arguments(
            "browser_set_file: path is empty",
        ));
    }
    let path = PathBuf::from(trimmed);
    let joined = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    dunce::canonicalize(&joined).map_err(|e| {
        xai_tool_runtime::ToolError::invalid_arguments(format!(
            "browser_set_file: cannot resolve `{}`: {e}",
            joined.display()
        ))
    })
}
