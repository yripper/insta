//! `insta start`: the long-running MCP server pm2 keeps alive.

use std::sync::Arc;

use anyhow::{Context, Result};
use rand::RngCore;

use crate::config::{Config, Paths};
use crate::domain::clock::{SystemTimer, ThreadRng, Timer};
use crate::domain::send_queue::SendQueue;
use crate::mcp::http::serve;
use crate::mcp::server::InstaServer;
use crate::mcp::state::{AppState, Deps, HttpConnector};
use crate::secrets::{ACCOUNT_BEARER, SecretStore, default_store};
use crate::store::Store;
use crate::whisper::OpenAiWhisper;
use crate::whisper::openai::OPENAI_BASE;

/// Hex, not base64: a token starting with '-' could be read as a flag inside
/// the `security -i` command line (same reason as paperplane).
pub async fn ensure_bearer_token(secrets: &dyn SecretStore) -> Result<String> {
    if let Some(t) = secrets.get(ACCOUNT_BEARER).await? {
        return Ok(t);
    }
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    secrets.set(ACCOUNT_BEARER, &token).await?;
    Ok(token)
}

pub async fn start() -> Result<()> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths)?;
    let store = Arc::new(Store::open(&paths.db)?);
    let secrets: Arc<dyn SecretStore> = Arc::from(default_store()?);
    let token = ensure_bearer_token(secrets.as_ref()).await?;

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", config.port))
        .await
        .with_context(|| format!("port {} is busy: insta is probably already running (pm2 status)", config.port))?;

    let timer: Arc<dyn Timer> = Arc::new(SystemTimer);
    let queue = Arc::new(SendQueue::new(store.clone(), timer.clone(), Arc::new(ThreadRng)));
    let whisper = Arc::new(OpenAiWhisper::new(OPENAI_BASE, secrets.clone())?);
    let state = Arc::new(AppState::new(Deps {
        store,
        secrets,
        connector: Arc::new(HttpConnector),
        queue: queue.clone(),
        whisper,
        timer,
    }));
    tokio::spawn(queue.clone().run());
    match state.actions().await {
        Ok(a) => tracing::info!(user_id = %a.me().user_id, "logged in"),
        Err(e) => tracing::warn!("not ready yet: {e}"),
    }
    tracing::info!(port = config.port, "insta listening on 127.0.0.1");
    serve(InstaServer::new(state, Some(paths.style)), listener, token).await?;
    // Queued text lives only in memory, so it's gone now. Count only, never content.
    let dropped = queue.queued_count();
    if dropped > 0 {
        tracing::warn!(dropped, "unsent messages dropped on shutdown");
    }
    Ok(())
}
