//! MCP health overlays and handshake helpers.
//!
//! Blender MCP stdio can handshake successfully while the Blender addon is
//! not listening on TCP 9876. Health reporting must not call that "ready".
//! Handshake helpers keep one stdio failure from looking like a catalog-wide
//! outage, and surface child stderr when initialize dies (Windows 232).

use std::net::{Ipv6Addr, SocketAddr};
use std::time::Duration;

/// Default Blender MCP addon socket (`blender-mcp` / ahujasid).
pub const BLENDER_ADDON_DEFAULT_PORT: u16 = 9876;

/// How long a blender-addon TCP probe waits before treating the port as down.
pub const BLENDER_ADDON_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Model-visible reason when the addon socket is closed.
pub const BLENDER_OFFLINE_REASON: &str = "blender addon not listening on localhost:9876";

/// Remediation for [`BLENDER_OFFLINE_REASON`].
pub const BLENDER_OFFLINE_DIAGNOSTIC: &str = "Start the Blender MCP addon so it listens on localhost:9876 (enable the add-on in Blender, then retry).";

/// Structured hint appended to blender tool errors when the addon is down.
pub const BLENDER_OFFLINE_HINT: &str =
    "[blender_offline] Start the Blender MCP addon (localhost:9876).";

/// Read `BLENDER_PORT` when set, otherwise [`BLENDER_ADDON_DEFAULT_PORT`].
pub fn blender_addon_port() -> u16 {
    std::env::var("BLENDER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|p| *p > 0)
        .unwrap_or(BLENDER_ADDON_DEFAULT_PORT)
}

/// True for MCP server ids that talk to the Blender addon (not every string
/// that happens to contain "blend").
pub fn is_blender_mcp_server(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    n == "blender"
        || n.starts_with("blender-")
        || n.starts_with("blender_")
        || n.ends_with("-blender")
        || n.ends_with("_blender")
        || n.contains("blender-mcp")
        || n.contains("blender_mcp")
}

/// TCP connect with a bounded timeout. `false` on refuse, timeout, or bind errors.
pub fn probe_tcp(addr: SocketAddr, timeout: Duration) -> bool {
    std::net::TcpStream::connect_timeout(&addr, timeout).is_ok()
}

/// Probe the configured Blender addon port on loopback (IPv4 then IPv6).
pub fn blender_addon_listening() -> bool {
    blender_addon_listening_on(blender_addon_port())
}

/// Probe a specific loopback port (test hook / `BLENDER_PORT` override).
pub fn blender_addon_listening_on(port: u16) -> bool {
    let timeout = BLENDER_ADDON_PROBE_TIMEOUT;
    probe_tcp(SocketAddr::from(([127, 0, 0, 1], port)), timeout)
        || probe_tcp(SocketAddr::from((Ipv6Addr::LOCALHOST, port)), timeout)
}

/// Per-server health used by overlays and unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerHealthStatus {
    Ready,
    Partial,
    Failed,
    Unknown,
}

impl ServerHealthStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

/// One MCP server in a health report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHealth {
    pub name: String,
    pub tool_count: usize,
    pub status: ServerHealthStatus,
    pub reason: Option<String>,
    pub diagnostic: Option<String>,
}

/// Transport-open health for one client. Blender additionally requires the
/// addon TCP probe; other servers ignore `blender_probe_ok`.
pub fn client_health_ready(
    server_name: &str,
    transport_open: bool,
    blender_probe_ok: bool,
) -> bool {
    if !transport_open {
        return false;
    }
    if is_blender_mcp_server(server_name) {
        blender_probe_ok
    } else {
        true
    }
}

/// Downgrade blender from `ready` when the addon TCP probe fails.
/// Other servers are left unchanged. `probe_ok` is injectable so tests can
/// simulate bind-refused without a live Blender install.
pub fn overlay_blender_addon_health(servers: &mut [ServerHealth], probe_ok: bool) {
    if probe_ok {
        return;
    }
    let port = blender_addon_port();
    let reason = blender_offline_reason_for_port(port);
    for server in servers.iter_mut() {
        if !is_blender_mcp_server(&server.name) {
            continue;
        }
        server.status = if server.tool_count > 0 {
            ServerHealthStatus::Partial
        } else {
            ServerHealthStatus::Failed
        };
        server.reason = Some(reason.clone());
        server.diagnostic = Some(BLENDER_OFFLINE_DIAGNOSTIC.to_string());
    }
}

/// Overall catalog status: one blender-offline server is `partial`, not a
/// catalog-wide failure.
pub fn overall_health_status(servers: &[ServerHealth], index_ready: bool) -> ServerHealthStatus {
    if !index_ready {
        return ServerHealthStatus::Partial;
    }
    if servers.iter().any(|s| {
        matches!(
            s.status,
            ServerHealthStatus::Failed | ServerHealthStatus::Partial | ServerHealthStatus::Unknown
        )
    }) {
        ServerHealthStatus::Partial
    } else {
        ServerHealthStatus::Ready
    }
}

pub fn blender_offline_reason_for_port(port: u16) -> String {
    if port == BLENDER_ADDON_DEFAULT_PORT {
        BLENDER_OFFLINE_REASON.to_string()
    } else {
        format!("blender addon not listening on localhost:{port}")
    }
}

/// Append [`BLENDER_OFFLINE_HINT`] once.
pub fn with_blender_offline_hint(error: &str) -> String {
    if error.contains("[blender_offline]") {
        error.to_string()
    } else {
        format!("{error}\n{BLENDER_OFFLINE_HINT}")
    }
}

/// Windows 232 / EPIPE / broken pipe — stdio child closed before initialize.
pub fn is_stdio_pipe_closed(msg: &str) -> bool {
    let l = msg.to_ascii_lowercase();
    l.contains("pipe is being closed")
        || l.contains("os error 232")
        || l.contains("error_no_data")
        || l.contains("epipe")
        || l.contains("broken pipe")
}

/// Handshake error text for stdio. Pipe-closed names the Windows 232 / pin
/// workaround. Non-empty child stderr is appended (truncated).
pub fn format_stdio_handshake_failure(name: &str, msg: &str, stderr: &str) -> String {
    let mut out = if is_stdio_pipe_closed(msg) {
        format!(
            "MCP server '{name}' handshake failed: {msg}. \
             The stdio child exited before initialize (common with \
             `docker run --rm -i` on Windows). Pin a local `uvx`/`python` \
             command, or `turbo mcp restart {name}` after `docker pull`."
        )
    } else {
        format!("MCP server '{name}' handshake failed: {msg}")
    };
    let trimmed = stderr.trim();
    if !trimmed.is_empty() {
        let sample: String = trimmed.chars().take(1500).collect();
        out.push_str("\nstderr: ");
        out.push_str(&sample);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(name: &str, tool_count: usize, status: ServerHealthStatus) -> ServerHealth {
        ServerHealth {
            name: name.to_string(),
            tool_count,
            status,
            reason: None,
            diagnostic: None,
        }
    }

    #[test]
    fn client_health_not_ready_when_blender_tcp_probe_fails() {
        assert!(
            !client_health_ready("blender", true, false),
            "open MCP stdio is not ready if the addon port is closed"
        );
        assert!(
            client_health_ready("linear", true, false),
            "non-blender servers must not fail when the blender probe fails"
        );
        assert!(client_health_ready("blender", true, true));
        assert!(!client_health_ready("blender", false, true));
    }

    #[test]
    fn blender_health_not_ready_when_tcp_probe_fails() {
        let mut servers = [
            health("linear", 5, ServerHealthStatus::Ready),
            health("blender", 12, ServerHealthStatus::Ready),
            health("tasks", 3, ServerHealthStatus::Ready),
        ];
        overlay_blender_addon_health(&mut servers, false);
        assert_eq!(servers[0].status, ServerHealthStatus::Ready);
        assert_eq!(servers[2].status, ServerHealthStatus::Ready);
        assert_ne!(
            servers[1].status,
            ServerHealthStatus::Ready,
            "blender must not stay ready when the addon TCP probe fails"
        );
        assert_eq!(servers[1].status, ServerHealthStatus::Partial);
        assert!(
            servers[1]
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("localhost:9876")
        );
        assert_eq!(
            overall_health_status(&servers, true),
            ServerHealthStatus::Partial
        );
    }

    #[test]
    fn blender_health_bind_refused_is_not_ready() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        assert!(
            !probe_tcp(addr, BLENDER_ADDON_PROBE_TIMEOUT),
            "dropped listener must probe as refused/failed"
        );
        let mut servers = [health("blender-mcp", 0, ServerHealthStatus::Ready)];
        overlay_blender_addon_health(&mut servers, probe_tcp(addr, BLENDER_ADDON_PROBE_TIMEOUT));
        assert_eq!(servers[0].status, ServerHealthStatus::Failed);
        assert_eq!(
            overall_health_status(&servers, true),
            ServerHealthStatus::Partial
        );
    }

    #[test]
    fn blender_health_stays_ready_when_probe_ok() {
        let mut servers = [health("blender", 4, ServerHealthStatus::Ready)];
        overlay_blender_addon_health(&mut servers, true);
        assert_eq!(servers[0].status, ServerHealthStatus::Ready);
        assert!(servers[0].reason.is_none());
        assert_eq!(
            overall_health_status(&servers, true),
            ServerHealthStatus::Ready
        );
    }

    #[test]
    fn pipe_closed_232_is_classified() {
        assert!(is_stdio_pipe_closed(
            "The pipe is being closed. (os error 232)"
        ));
        assert!(is_stdio_pipe_closed("broken pipe"));
        assert!(is_stdio_pipe_closed("EPIPE"));
        assert!(!is_stdio_pipe_closed("connection refused"));
    }

    #[test]
    fn handshake_failure_includes_stderr_and_is_per_server() {
        let text = format_stdio_handshake_failure(
            "godot-docs-mcp",
            "The pipe is being closed. (os error 232)",
            "docker: failed to connect to the docker API\n",
        );
        assert!(text.contains("godot-docs-mcp"));
        assert!(text.contains("os error 232"));
        assert!(text.contains("stderr:"));
        assert!(text.contains("docker: failed to connect"));
        assert!(
            !text.to_ascii_lowercase().contains("all mcp"),
            "one server failure must not claim the whole catalog died"
        );
    }

    #[test]
    fn blender_name_matcher_is_tight() {
        assert!(is_blender_mcp_server("blender"));
        assert!(is_blender_mcp_server("Blender-MCP"));
        assert!(is_blender_mcp_server("ahujasid-blender"));
        assert!(!is_blender_mcp_server("linear"));
        assert!(!is_blender_mcp_server("godot-docs-mcp"));
    }
}
