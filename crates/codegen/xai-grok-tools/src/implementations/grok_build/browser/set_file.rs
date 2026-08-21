//! `browser_set_file` — set `<input type=file>` from a workspace or session path.

use std::path::{Path, PathBuf};

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
        description = "Workspace-relative or absolute path of an existing file under the \
            workspace, confine root, or session folder. Does not submit the form."
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
        let path = canonical.to_string_lossy().into_owned();
        handle.set_file(input.uid.clone(), path.clone()).await?;
        Ok(super::text_output(format!(
            "Set file input {} to {}",
            input.uid, path
        )))
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
