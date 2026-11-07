//! Streamable HTTP on 127.0.0.1 behind a Host check and a bearer token
//! (same hardening as paperplane's `src/mcp/http.ts`).

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use super::server::InstaServer;

#[derive(Clone)]
struct Guard {
    hosts: [String; 2],
    expected_auth: Arc<str>,
}

/// Equal-time comparison so the token can't be guessed byte by byte from timing.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn guard(State(g): State<Guard>, req: Request, next: Next) -> Result<Response, StatusCode> {
    // A malicious page can rebind its own hostname to 127.0.0.1, but the
    // browser still sends that hostname in Host, so only loopback Host values pass.
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if !g.hosts.iter().any(|h| h == host) {
        return Err(StatusCode::FORBIDDEN);
    }
    let auth = req.headers().get(header::AUTHORIZATION).map(|h| h.as_bytes()).unwrap_or(b"");
    // Fail closed: an empty token would accept the literal header "Bearer ".
    if g.expected_auth.len() <= "Bearer ".len() || !same(auth, g.expected_auth.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

pub fn guard_router(router: Router, port: u16, token: String) -> Router {
    let g = Guard {
        hosts: [format!("127.0.0.1:{port}"), format!("localhost:{port}")],
        expected_auth: format!("Bearer {token}").into(),
    };
    router.layer(middleware::from_fn_with_state(g, guard))
}

pub async fn serve(server: InstaServer, listener: tokio::net::TcpListener, token: String) -> anyhow::Result<()> {
    let port = listener.local_addr()?.port();
    // Stateless: no session state can leak between Claude clients.
    let cfg = StreamableHttpServerConfig::default().with_legacy_session_mode(false).with_json_response(true);
    let svc = StreamableHttpService::new(move || Ok(server.clone()), Arc::new(NeverSessionManager::default()), cfg);
    let app = guard_router(Router::new().nest_service("/mcp", svc), port, token);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {},
            }
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
