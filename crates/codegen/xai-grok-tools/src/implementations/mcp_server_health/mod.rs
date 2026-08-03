//! `mcp_server_health` — read-only summary of configured MCP servers.
//!
//! Uses [`ToolIndex`] (server summaries + empty-query `search_snapshot` for
//! readiness). Full per-server `init_failed` detail may only appear after the
//! MCP handshake via system-reminders; this tool reports best-effort status.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::output::{TextOutput, ToolOutput};
use crate::types::tool::{ToolKind, ToolNamespace};
use crate::types::tool_index::ToolIndex;

pub const MCP_SERVER_HEALTH_TOOL_NAME: &str = "mcp_server_health";

/// Input for `mcp_server_health` (no required parameters).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct McpServerHealthInput {}

#[derive(Debug, Default)]
pub struct McpServerHealthTool;

impl crate::types::tool_metadata::ToolMetadata for McpServerHealthTool {
    fn kind(&self) -> ToolKind {
        // Dedicated meta-tool without its own ToolKind; avoid sharing
        // SearchTool so `tools.by_kind.search_tool` stays mapped correctly.
        ToolKind::Other
    }

    fn tool_namespace(&self) -> ToolNamespace {
        ToolNamespace::GrokBuild
    }

    fn description_template(&self) -> &str {
        "Report readiness of configured MCP servers.\n\n\
         Returns JSON: for each server, name, tool_count, and status \
         (`ready` | `partial` | `unknown`). Overall status is `ready` when the \
         tool index is fully initialized, else `partial`.\n\n\
         Note: failed handshakes may only appear in system-reminders; use \
         `search_tool` to discover tools. This tool does not call MCP servers."
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl xai_tool_runtime::Tool for McpServerHealthTool {
    type Args = McpServerHealthInput;
    type Output = ToolOutput;

    fn id(&self) -> xai_tool_protocol::ToolId {
        xai_tool_protocol::ToolId::new(MCP_SERVER_HEALTH_TOOL_NAME).expect("valid tool id")
    }

    fn description(
        &self,
        _ctx: &::xai_tool_runtime::ListToolsContext,
    ) -> xai_tool_types::ToolDescription {
        xai_tool_types::ToolDescription::new(
            MCP_SERVER_HEALTH_TOOL_NAME,
            crate::types::tool_metadata::ToolMetadata::sanitized_description_template(self),
        )
    }

    fn capabilities(&self) -> xai_tool_protocol::ToolCapabilities {
        xai_tool_protocol::ToolCapabilities {
            is_read_only: true,
            tool_scope: Some(xai_tool_protocol::ToolScope::Read),
            ..Default::default()
        }
    }

    async fn run(
        &self,
        ctx: xai_tool_runtime::ToolCallContext,
        _input: McpServerHealthInput,
    ) -> Result<ToolOutput, xai_tool_runtime::ToolError> {
        use crate::types::tool_metadata::shared_resources;
        let resources = shared_resources(&ctx)?;

        let Some(tool_index) = resources.lock().await.get::<ToolIndex>().cloned() else {
            let body = serde_json::json!({
                "status": "unknown",
                "servers": [],
                "note": "No integration tools are configured. MCP servers are not connected. Use search_tool after connecting servers."
            });
            return Ok(ToolOutput::Text(TextOutput::from(
                serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
            )));
        };
        let tool_index = tool_index.0.clone();

        // Empty query: results unused; is_ready / failed_servers reflect index state.
        let snapshot = tool_index.search_snapshot("", 1);
        let summaries = tool_index.list_server_summaries();

        let failed_names: std::collections::HashSet<&str> = snapshot
            .failed_servers
            .iter()
            .map(|f| f.name.as_str())
            .collect();

        let overall = if snapshot.is_ready && snapshot.failed_servers.is_empty() {
            "ready"
        } else {
            "partial"
        };

        let mut servers: Vec<serde_json::Value> = summaries
            .iter()
            .map(|s| {
                let status = if failed_names.contains(s.name.as_str()) {
                    "failed"
                } else if snapshot.is_ready {
                    "ready"
                } else if s.tool_count > 0 {
                    // Index still warming, but this server already contributed tools.
                    "partial"
                } else {
                    "unknown"
                };
                serde_json::json!({
                    "name": s.name,
                    "tool_count": s.tool_count,
                    "status": status,
                })
            })
            .collect();

        // Include failed servers that may not appear in list_server_summaries.
        for failed in &snapshot.failed_servers {
            if !summaries.iter().any(|s| s.name == failed.name) {
                servers.push(serde_json::json!({
                    "name": failed.name,
                    "tool_count": 0,
                    "status": "failed",
                    "reason": failed.reason,
                }));
            }
        }

        let failed_json: Vec<serde_json::Value> = snapshot
            .failed_servers
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name,
                    "reason": f.reason,
                })
            })
            .collect();

        let mut note = if !snapshot.is_ready {
            Some(
                "Some MCP servers may still be connecting. Use search_tool to discover tools."
                    .to_string(),
            )
        } else if servers.is_empty() && failed_json.is_empty() {
            Some(
                "No MCP servers are listed in the tool index. Connect MCP servers here, or if this is a subagent, check the agent's mcpInheritance.".to_string(),
            )
        } else {
            Some(
                "Use search_tool to discover tools; failed servers appear after handshake in system-reminder.".to_string(),
            )
        };

        if !snapshot.failed_servers.is_empty() {
            let failed_line = snapshot
                .failed_servers
                .iter()
                .map(|f| format!("{} ({})", f.name, f.reason))
                .collect::<Vec<_>>()
                .join("; ");
            let extra = format!(
                "Failed MCP servers: {failed_line}. Run `grok mcp doctor` or check /mcps."
            );
            note = Some(match note {
                Some(n) => format!("{n} {extra}"),
                None => extra,
            });
        }

        let body = serde_json::json!({
            "status": overall,
            "servers": servers,
            "failed_servers": failed_json,
            "note": note,
        });

        Ok(ToolOutput::Text(TextOutput::from(
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tool_index::{
        SearchSnapshot, ServerSummary, ToolIndex, ToolSearchIndex, ToolSearchResult,
    };
    use xai_tool_runtime::Tool;

    struct StaticToolIndex {
        snapshot: SearchSnapshot,
        servers: Vec<ServerSummary>,
    }

    impl ToolSearchIndex for StaticToolIndex {
        fn search_snapshot(&self, _query: &str, _limit: usize) -> SearchSnapshot {
            self.snapshot.clone()
        }

        fn list_server_summaries(&self) -> Vec<ServerSummary> {
            self.servers.clone()
        }
    }

    #[tokio::test]
    async fn mcp_server_health_lists_ready_servers() {
        let resources = crate::types::resources::Resources::default().into_shared();
        resources
            .lock()
            .await
            .insert(ToolIndex(std::sync::Arc::new(StaticToolIndex {
                snapshot: SearchSnapshot {
                    results: vec![ToolSearchResult {
                        tool_name: "linear__save_issue".into(),
                        server_name: "linear".into(),
                        description: "save".into(),
                        score: 1.0,
                        parameters: vec![],
                        input_schema: serde_json::json!({}),
                    }],
                    total_hidden_tools: 0,
                    is_ready: true,
                    failed_servers: Vec::new(),
                },
                servers: vec![ServerSummary {
                    name: "linear".into(),
                    description: Some("PM".into()),
                    tool_count: 5,
                    tool_names: vec!["save_issue".into()],
                }],
            })));
        let mut ctx =
            xai_tool_runtime::ToolCallContext::new(xai_tool_protocol::ToolCallId::new_v7());
        ctx.extensions.insert(resources);

        let output = McpServerHealthTool
            .run(ctx, McpServerHealthInput {})
            .await
            .unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected Text output");
        };
        let json: serde_json::Value = serde_json::from_str(&text.text).unwrap();
        assert_eq!(json["status"], "ready");
        assert_eq!(json["servers"][0]["name"], "linear");
        assert_eq!(json["servers"][0]["tool_count"], 5);
        assert_eq!(json["servers"][0]["status"], "ready");
        assert!(json["note"].as_str().unwrap().contains("search_tool"));
    }

    #[tokio::test]
    async fn mcp_server_health_partial_when_index_not_ready() {
        let resources = crate::types::resources::Resources::default().into_shared();
        resources
            .lock()
            .await
            .insert(ToolIndex(std::sync::Arc::new(StaticToolIndex {
                snapshot: SearchSnapshot {
                    results: vec![],
                    total_hidden_tools: 0,
                    is_ready: false,
                    failed_servers: Vec::new(),
                },
                servers: vec![ServerSummary {
                    name: "slack".into(),
                    description: None,
                    tool_count: 0,
                    tool_names: vec![],
                }],
            })));
        let mut ctx =
            xai_tool_runtime::ToolCallContext::new(xai_tool_protocol::ToolCallId::new_v7());
        ctx.extensions.insert(resources);

        let output = McpServerHealthTool
            .run(ctx, McpServerHealthInput {})
            .await
            .unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected Text output");
        };
        let json: serde_json::Value = serde_json::from_str(&text.text).unwrap();
        assert_eq!(json["status"], "partial");
        assert_eq!(json["servers"][0]["status"], "unknown");
    }

    #[tokio::test]
    async fn mcp_server_health_includes_failed_servers() {
        use crate::types::tool_index::FailedServerInfo;
        let resources = crate::types::resources::Resources::default().into_shared();
        resources
            .lock()
            .await
            .insert(ToolIndex(std::sync::Arc::new(StaticToolIndex {
                snapshot: SearchSnapshot {
                    results: vec![],
                    total_hidden_tools: 0,
                    is_ready: true,
                    failed_servers: vec![FailedServerInfo {
                        name: "broken".into(),
                        reason: "handshake timeout".into(),
                    }],
                },
                servers: vec![],
            })));
        let mut ctx =
            xai_tool_runtime::ToolCallContext::new(xai_tool_protocol::ToolCallId::new_v7());
        ctx.extensions.insert(resources);

        let output = McpServerHealthTool
            .run(ctx, McpServerHealthInput {})
            .await
            .unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected Text output");
        };
        let json: serde_json::Value = serde_json::from_str(&text.text).unwrap();
        assert_eq!(json["status"], "partial");
        assert_eq!(json["failed_servers"][0]["name"], "broken");
        assert_eq!(json["servers"][0]["name"], "broken");
        assert_eq!(json["servers"][0]["status"], "failed");
    }

    #[tokio::test]
    async fn mcp_server_health_no_tool_index() {
        let resources = crate::types::resources::Resources::default().into_shared();
        let mut ctx =
            xai_tool_runtime::ToolCallContext::new(xai_tool_protocol::ToolCallId::new_v7());
        ctx.extensions.insert(resources);

        let output = McpServerHealthTool
            .run(ctx, McpServerHealthInput {})
            .await
            .unwrap();
        let ToolOutput::Text(text) = output else {
            panic!("expected Text output");
        };
        let json: serde_json::Value = serde_json::from_str(&text.text).unwrap();
        assert_eq!(json["status"], "unknown");
        assert!(json["servers"].as_array().unwrap().is_empty());
    }
}
