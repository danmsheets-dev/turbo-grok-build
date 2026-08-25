//! Axum server and Leptos SSR integration.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, bail};
use axum::extract::{FromRef, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::get};
use leptos::prelude::*;
use leptos_axum::{LeptosRoutes, generate_route_list};

use crate::api;
use crate::app::App;
use crate::store::DashboardStore;

const DASHBOARD_TOKEN_HEADER: &str = "x-turbo-dashboard-token";

#[derive(Clone)]
pub struct AppState {
    leptos_options: LeptosOptions,
    store: Arc<DashboardStore>,
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

impl FromRef<AppState> for Arc<DashboardStore> {
    fn from_ref(state: &AppState) -> Self {
        state.store.clone()
    }
}

#[derive(Debug, Clone)]
pub struct DashboardServerConfig {
    pub bind: SocketAddr,
    pub open_browser: bool,
    pub grok_home: std::path::PathBuf,
}

impl DashboardServerConfig {
    pub fn new(grok_home: std::path::PathBuf) -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 9090)),
            open_browser: true,
            grok_home,
        }
    }
}

#[derive(Clone)]
struct DashboardGuard {
    port: u16,
    token: Arc<str>,
}

/// Build the complete router. A single state contains both dashboard data and
/// Leptos options, allowing Axum to extract either through `FromRef`.
pub async fn build_router(
    store: Arc<DashboardStore>,
    bind: SocketAddr,
    token: &str,
) -> Result<Router> {
    // This dashboard is embedded SSR, not a cargo-leptos application. Explicit
    // options avoid environment warnings and disable the development reload
    // client while keeping the fallback static root away from the working tree.
    let leptos_options = LeptosOptions::builder()
        .output_name(env!("CARGO_PKG_NAME"))
        .site_root("target/site")
        .env(Env::PROD)
        .build();
    Ok(build_router_with_options(
        store,
        leptos_options,
        bind,
        token,
    ))
}

pub fn build_router_with_options(
    store: Arc<DashboardStore>,
    leptos_options: LeptosOptions,
    bind: SocketAddr,
    token: &str,
) -> Router {
    let state = AppState {
        leptos_options: leptos_options.clone(),
        store: store.clone(),
    };
    let guard = DashboardGuard {
        port: bind.port(),
        token: Arc::from(token),
    };
    let routes = generate_route_list(App);
    let provide_store = {
        let store = store.clone();
        move || provide_context(store.clone())
    };

    Router::new()
        .route("/api/sessions", get(api::list_sessions))
        .route("/api/sessions/{id}", get(api::get_session))
        .route("/api/sessions/{id}/timeline", get(api::get_timeline))
        .route("/api/sessions/{id}/chat", get(api::get_chat_history))
        .route("/api/sessions/{id}/charts", get(api::get_session_charts))
        .route("/api/sessions/{id}/events", get(api::live_events))
        .route("/api/metrics/server", get(api::get_server_metrics))
        .route("/api/metrics/resources", get(api::get_resource_metrics))
        .route("/api/logs", get(api::get_logs))
        .leptos_routes_with_context(&state, routes, provide_store.clone(), {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler_with_context::<
            AppState,
            _,
        >(provide_store, shell))
        .with_state(state)
        .layer(middleware::from_fn_with_state(guard, dashboard_guard))
}

/// Start the dashboard. Remote binding is intentionally rejected: session
/// prompts, tool names and local paths are private machine data.
pub async fn serve(config: DashboardServerConfig) -> Result<()> {
    if !config.bind.ip().is_loopback() {
        bail!(
            "dashboard must bind to a loopback address (received {})",
            config.bind.ip()
        );
    }

    let store = Arc::new(DashboardStore::new(config.grok_home));
    if let Err(error) = store.refresh_active_sessions().await {
        tracing::warn!(%error, "unable to load active sessions; continuing with stored sessions");
    }
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let address = listener.local_addr()?;
    let token = mint_dashboard_token();
    let app = build_router(store, address, &token).await?;
    let url = format!("http://{address}/?token={token}");
    tracing::info!(%url, "Turbo dashboard listening");

    if config.open_browser {
        let url = url.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = webbrowser::open(&url) {
                tracing::warn!(%error, "failed to open dashboard browser");
            }
        });
    }

    axum::serve(listener, app).await?;
    Ok(())
}

fn mint_dashboard_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

async fn dashboard_guard(
    State(guard): State<DashboardGuard>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !host_header_allowed(host, guard.port) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }

    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let origin = origin.to_str().unwrap_or_default();
        if !origin_header_allowed(origin) {
            return (StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }

    if request
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }

    if is_api_path(request.uri().path()) && !request_has_valid_token(&request, &guard.token) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    next.run(request).await
}

fn host_header_allowed(host: &str, port: u16) -> bool {
    let host = host.trim();
    let allowed = [
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        format!("localhost:{port}"),
    ];
    allowed
        .iter()
        .any(|candidate| host.eq_ignore_ascii_case(candidate))
}

fn origin_header_allowed(origin: &str) -> bool {
    origin_host(origin).is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
    })
}

fn origin_host(origin: &str) -> Option<&str> {
    let origin = origin.trim();
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() {
        return None;
    }
    if let Some(inner) = authority.strip_prefix('[') {
        return inner.split(']').next().filter(|host| !host.is_empty());
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            Some(host)
        }
        _ => Some(authority),
    }
}

fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn request_has_valid_token(request: &Request, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    if let Some(header) = request.headers().get(DASHBOARD_TOKEN_HEADER)
        && let Ok(value) = header.to_str()
        && token_eq(value.trim(), expected)
    {
        return true;
    }
    query_token(request.uri().query().unwrap_or_default())
        .is_some_and(|value| token_eq(value, expected))
}

fn query_token(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "token" && !value.is_empty()).then_some(value)
    })
}

fn token_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

fn shell(_options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="color-scheme" content="dark"/>
                <title>"Turbo Observability"</title>
                <style>{DASHBOARD_CSS}</style>
            </head>
            <body>
                <App/>
                <script>{DASHBOARD_TOKEN_SCRIPT}</script>
            </body>
        </html>
    }
}

const DASHBOARD_CSS: &str = r#"
:root {
  --bg: #090d13; --panel: #111821; --panel-2: #171f2a; --line: #273242;
  --text: #e7edf5; --muted: #8b9aad; --blue: #57a6ff; --green: #39d98a;
  --yellow: #f2c94c; --red: #ff6b6b; --purple: #b28dff;
}
* { box-sizing: border-box; }
body { margin: 0; color: var(--text); background: var(--bg); font-family: Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
a { color: var(--blue); text-decoration: none; } a:hover { text-decoration: underline; }
.container { max-width: 1440px; margin: 0 auto; padding: 24px; }
.header { display: flex; justify-content: space-between; align-items: center; gap: 24px; margin-bottom: 28px; border-bottom: 1px solid var(--line); padding-bottom: 18px; }
.brand { display: flex; align-items: baseline; gap: 10px; } .brand h1 { margin: 0; font-size: 22px; } .brand span { color: var(--muted); font-size: 12px; }
.nav { display: flex; flex-wrap: wrap; gap: 16px; } .nav a { color: var(--muted); font-size: 14px; } .nav a:hover { color: var(--text); }
h2 { font-size: 22px; margin: 0 0 20px; } h3 { font-size: 15px; margin: 0 0 12px; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(210px, 1fr)); gap: 14px; margin-bottom: 24px; }
.card { display: block; background: var(--panel); border: 1px solid var(--line); border-radius: 10px; padding: 16px; margin-bottom: 16px; overflow: hidden; }
a.card:hover { border-color: #3e526d; text-decoration: none; }
.metrics { display: flex; gap: 22px; flex-wrap: wrap; } .metric { min-width: 90px; display: flex; flex-direction: column; gap: 3px; }
.metric-value { font-size: 22px; font-weight: 650; color: var(--text); } .metric-label { font-size: 11px; color: var(--muted); text-transform: uppercase; letter-spacing: .06em; }
.muted { color: var(--muted); } .mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
table { width: 100%; border-collapse: collapse; font-size: 13px; } th, td { padding: 11px 10px; text-align: left; border-bottom: 1px solid var(--line); vertical-align: top; }
th { position: sticky; top: 0; color: var(--muted); background: var(--panel); font-size: 11px; text-transform: uppercase; letter-spacing: .05em; }
.table-wrap { overflow: auto; max-height: 70vh; }
.badge { display: inline-flex; align-items: center; padding: 3px 8px; border-radius: 999px; font-size: 11px; background: #273242; color: var(--muted); }
.badge.active { background: #143d2b; color: var(--green); } .badge.error { background: #451f27; color: var(--red); }
.tabs { display: flex; gap: 14px; margin: -6px 0 20px; flex-wrap: wrap; }
.timeline { display: flex; flex-direction: column; gap: 7px; } .timeline-event { display: grid; grid-template-columns: 105px 190px 1fr auto; gap: 10px; padding: 9px 12px; border-left: 3px solid var(--line); background: var(--panel); border-radius: 4px; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 12px; }
.timeline-event.tool { border-left-color: var(--blue); } .timeline-event.turn { border-left-color: var(--green); } .timeline-event.error { border-left-color: var(--red); }
.chat-message { white-space: pre-wrap; overflow-wrap: anywhere; } .chat-role { color: var(--purple); font-weight: 650; text-transform: uppercase; font-size: 11px; letter-spacing: .06em; }
.log-row { display: grid; grid-template-columns: 180px 60px 85px 1fr; gap: 10px; padding: 7px 9px; border-bottom: 1px solid var(--line); font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; }
.log-row.error, .log-row.warn { color: var(--red); }
.progress { height: 10px; background: #202a37; border-radius: 999px; overflow: hidden; } .progress > span { display: block; height: 100%; background: linear-gradient(90deg, var(--blue), var(--purple)); }
.chart { display: flex; flex-direction: column; gap: 9px; } .bar-row { display: grid; grid-template-columns: minmax(120px, 240px) 1fr 70px; gap: 10px; align-items: center; font-size: 12px; } .bar { height: 12px; background: #202a37; border-radius: 3px; overflow: hidden; } .bar span { display: block; height: 100%; background: var(--blue); }
.filters { display: flex; gap: 10px; flex-wrap: wrap; margin-bottom: 16px; } input, select { background: var(--panel-2); color: var(--text); border: 1px solid var(--line); border-radius: 6px; padding: 8px 10px; }
.empty { padding: 48px; text-align: center; color: var(--muted); }
.error-box { border: 1px solid #67303b; background: #2b171c; color: #ff9b9b; padding: 12px; border-radius: 8px; }
@media (max-width: 760px) { .container { padding: 14px; } .header { align-items: flex-start; flex-direction: column; } .timeline-event { grid-template-columns: 90px 1fr; } .log-row { grid-template-columns: 1fr; } }
"#;

const DASHBOARD_TOKEN_SCRIPT: &str = r#"
(function(){
  var k='turbo-dashboard-token';
  var q=new URLSearchParams(location.search).get('token');
  if(q){try{sessionStorage.setItem(k,q);}catch(e){}}
  var t=q;if(!t){try{t=sessionStorage.getItem(k);}catch(e){}}
  if(!t)return;
  document.querySelectorAll('a[href^="/api/"]').forEach(function(a){
    var h=a.getAttribute('href');
    if(!h||h.indexOf('token=')>=0)return;
    a.setAttribute('href', h+(h.indexOf('?')>=0?'&':'?')+'token='+encodeURIComponent(t));
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const TOKEN: &str = "test-dashboard-token-aaaaaaaa";

    fn test_bind() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 9090))
    }

    async fn test_router() -> Router {
        let temp = tempfile::TempDir::new().unwrap();
        let store = Arc::new(DashboardStore::new(temp.path().to_owned()));
        build_router(store, test_bind(), TOKEN).await.unwrap()
    }

    fn api_request(host: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/api/sessions").header("host", host);
        if let Some(token) = token {
            builder = builder.header(DASHBOARD_TOKEN_HEADER, token);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn host_allowlist_matches_loopback_forms() {
        assert!(host_header_allowed("127.0.0.1:9090", 9090));
        assert!(host_header_allowed("localhost:9090", 9090));
        assert!(host_header_allowed("[::1]:9090", 9090));
        assert!(host_header_allowed("LOCALHOST:9090", 9090));
        assert!(!host_header_allowed("evil.test:9090", 9090));
        assert!(!host_header_allowed("127.0.0.1:9091", 9090));
        assert!(!host_header_allowed("127.0.0.1", 9090));
    }

    #[test]
    fn origin_allowlist_rejects_cross_origin() {
        assert!(origin_header_allowed("http://127.0.0.1:9090"));
        assert!(origin_header_allowed("http://localhost:9090"));
        assert!(origin_header_allowed("http://[::1]:9090"));
        assert!(!origin_header_allowed("https://evil.test"));
        assert!(!origin_header_allowed("null"));
        assert!(!origin_header_allowed("https://evil.test:9090"));
    }

    #[tokio::test]
    async fn host_evil_test_rejected() {
        let app = test_router().await;
        let response = app
            .oneshot(api_request("evil.test:9090", Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn origin_evil_test_rejected() {
        let app = test_router().await;
        let request = Request::builder()
            .uri("/api/sessions")
            .header("host", "127.0.0.1:9090")
            .header("origin", "https://evil.test")
            .header(DASHBOARD_TOKEN_HEADER, TOKEN)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn missing_token_on_api_rejected() {
        let app = test_router().await;
        let response = app
            .oneshot(api_request("127.0.0.1:9090", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn loopback_host_with_matching_token_allowed() {
        let app = test_router().await;
        let response = app
            .oneshot(api_request("127.0.0.1:9090", Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn query_token_on_api_allowed() {
        let app = test_router().await;
        let request = Request::builder()
            .uri(format!("/api/sessions?token={TOKEN}"))
            .header("host", "127.0.0.1:9090")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn cross_site_fetch_rejected() {
        let app = test_router().await;
        let request = Request::builder()
            .uri("/api/sessions")
            .header("host", "127.0.0.1:9090")
            .header("sec-fetch-site", "cross-site")
            .header(DASHBOARD_TOKEN_HEADER, TOKEN)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
