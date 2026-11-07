use super::http::guard_router;
use super::instructions::{BASE_INSTRUCTIONS, build_instructions, current_instructions, read_style_guide};
use super::server::{InstaServer, SendArgs, parse_time};
use super::state::{AppState, Connector, Deps};
use crate::domain::clock::fake::{FakeTimer, SeqGaps};
use crate::domain::send_queue::SendQueue;
use crate::ig::InstagramApi;
use crate::ig::fake::{FakeData, FakeInstagram, dm, text, user};
use crate::ig::types::{Credentials, IgError, Me};
use crate::secrets::tests_support::MemSecrets;
use crate::secrets::{ACCOUNT_CSRFTOKEN, ACCOUNT_DS_USER_ID, ACCOUNT_SESSIONID, SecretStore};
use crate::store::Store;
use crate::whisper::fake::FakeWhisper;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use rmcp::ServerHandler;
use std::sync::Arc;
use tower::ServiceExt;

struct FakeConnector {
    api: Arc<FakeInstagram>,
}

#[async_trait]
impl Connector for FakeConnector {
    async fn connect(&self, _c: Credentials) -> Result<(Arc<dyn InstagramApi>, Me), IgError> {
        let me = self.api.whoami().await?;
        Ok((self.api.clone(), me))
    }
}

fn text_of(r: &rmcp::model::CallToolResult) -> String {
    serde_json::to_string(&r.content).unwrap()
}

struct Rig {
    api: Arc<FakeInstagram>,
    secrets: Arc<MemSecrets>,
    store: Arc<Store>,
    server: InstaServer,
}

fn rig() -> Rig {
    let mut d = FakeData { threads: vec![dm("TH001", user("U001", "ana", ""), 10, false)], ..Default::default() };
    d.messages.insert("TH001".into(), vec![text("I001", 10, "U001", "fake")]);
    let api = Arc::new(FakeInstagram::new(d));
    let secrets = Arc::new(MemSecrets::default());
    let store = Arc::new(Store::open_in_memory().unwrap());
    let timer = Arc::new(FakeTimer::at(1_800_000_000_000));
    let queue = Arc::new(SendQueue::new(store.clone(), timer.clone(), Arc::new(SeqGaps::new(&[20_000]))));
    let state = Arc::new(AppState::new(Deps {
        store: store.clone(),
        secrets: secrets.clone(),
        connector: Arc::new(FakeConnector { api: api.clone() }),
        queue,
        whisper: Arc::new(FakeWhisper::new(Ok(String::new()))),
        timer,
    }));
    Rig { api, secrets, store, server: InstaServer::new(state, None) }
}

async fn log_in(s: &MemSecrets) {
    s.set(ACCOUNT_SESSIONID, "S1").await.unwrap();
    s.set(ACCOUNT_CSRFTOKEN, "C1").await.unwrap();
    s.set(ACCOUNT_DS_USER_ID, "U000").await.unwrap();
}

#[tokio::test]
async fn guard_rejects_foreign_host_and_bad_tokens() {
    let app =
        guard_router(axum::Router::new().route("/mcp", axum::routing::post(|| async { "ok" })), 7780, "secret".into());
    let req = |host: &str, auth: Option<&str>| {
        let mut b = Request::post("/mcp").header("host", host);
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(Body::empty()).unwrap()
    };
    let status = |r: axum::response::Response| r.status();
    assert_eq!(
        status(app.clone().oneshot(req("evil.com:7780", Some("Bearer secret"))).await.unwrap()),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(app.clone().oneshot(req("127.0.0.1:7779", Some("Bearer secret"))).await.unwrap()),
        StatusCode::FORBIDDEN
    );
    assert_eq!(status(app.clone().oneshot(req("127.0.0.1:7780", None)).await.unwrap()), StatusCode::UNAUTHORIZED);
    assert_eq!(
        status(app.clone().oneshot(req("127.0.0.1:7780", Some("Bearer nope"))).await.unwrap()),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(app.clone().oneshot(req("localhost:7780", Some("Bearer secret"))).await.unwrap()),
        StatusCode::OK
    );
    assert_eq!(status(app.oneshot(req("127.0.0.1:7780", Some("Bearer secret"))).await.unwrap()), StatusCode::OK);
}

#[tokio::test]
async fn guard_fails_closed_on_empty_token() {
    let app =
        guard_router(axum::Router::new().route("/mcp", axum::routing::post(|| async { "ok" })), 7780, String::new());
    let req = Request::post("/mcp")
        .header("host", "127.0.0.1:7780")
        .header("authorization", "Bearer ")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tools_report_needs_login_then_recover() {
    let r = rig();
    let res = r.server.call_pending().await;
    assert_eq!(res.is_error, Some(true));
    assert!(text_of(&res).contains("insta login"), "{}", text_of(&res));

    log_in(&r.secrets).await;
    r.api.data.lock().unwrap().fail_with = Some(IgError::NeedsLogin);
    assert!(text_of(&r.server.call_pending().await).contains("insta login"));

    r.api.data.lock().unwrap().fail_with = None;
    let res = r.server.call_pending().await;
    assert_ne!(res.is_error, Some(true), "{}", text_of(&res));

    // Dies mid-use, then recovers without a restart.
    r.api.data.lock().unwrap().fail_with = Some(IgError::NeedsLogin);
    assert!(text_of(&r.server.call_pending().await).contains("insta login"));
    r.api.data.lock().unwrap().fail_with = None;
    assert_ne!(r.server.call_pending().await.is_error, Some(true));
}

#[tokio::test]
async fn challenge_stops_all_instagram_calls_until_login_clears_it() {
    let r = rig();
    log_in(&r.secrets).await;
    assert_ne!(r.server.call_pending().await.is_error, Some(true));
    r.api.data.lock().unwrap().fail_once.push_back(("inbox".into(), IgError::Challenge));
    assert!(text_of(&r.server.call_pending().await).contains("security check"));
    assert!(r.store.challenge_at().unwrap().is_some());
    let calls = r.api.calls().len();
    assert!(text_of(&r.server.call_pending().await).contains("security check"));
    assert_eq!(r.api.calls().len(), calls, "no Instagram call while challenged");
    r.store.clear_challenge().unwrap();
    assert_ne!(r.server.call_pending().await.is_error, Some(true));
}

#[tokio::test]
async fn send_message_returns_a_job_without_sending() {
    let r = rig();
    log_in(&r.secrets).await;
    let res = r.server.call_send(SendArgs { chat: "ana".into(), text: "fake".into(), seen_until: None }).await;
    let t = text_of(&res);
    assert!(t.contains("job_id") && t.contains("eta"), "{t}");
    assert_eq!(r.api.count("send_text"), 0);
}

#[test]
fn instructions_append_style_and_reread_per_call() {
    assert_eq!(build_instructions(None), BASE_INSTRUCTIONS);
    let s = build_instructions(Some("Short. No em dashes."));
    assert!(s.starts_with(BASE_INSTRUCTIONS) && s.ends_with("## Writing style\n\nShort. No em dashes."));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("style.md");
    assert_eq!(current_instructions(Some(&p)), BASE_INSTRUCTIONS);
    std::fs::write(&p, "be brief\n").unwrap();
    assert!(current_instructions(Some(&p)).ends_with("be brief"));
    std::fs::write(&p, "be briefer").unwrap();
    assert!(current_instructions(Some(&p)).ends_with("be briefer"), "no restart needed");
    assert_eq!(current_instructions(Some(dir.path())), BASE_INSTRUCTIONS, "unreadable style never blocks");
}

#[test]
fn blank_style_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("style.md");
    assert_eq!(read_style_guide(&p).unwrap(), None);
    std::fs::write(&p, "  \n").unwrap();
    assert_eq!(read_style_guide(&p).unwrap(), None);
}

#[test]
fn server_info_carries_instructions() {
    let r = rig();
    assert_eq!(r.server.get_info().instructions.as_deref(), Some(BASE_INSTRUCTIONS));
}

#[test]
fn parse_time_accepts_rfc3339_only() {
    assert_eq!(parse_time("2026-11-08T09:00:00-03:00").unwrap(), 1_794_139_200);
    assert!(parse_time("tomorrow").is_err());
}

#[test]
fn tools_list_has_all_17_tools() {
    let mut names: Vec<String> =
        InstaServer::tool_router().list_all().into_iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "clear_label",
            "get_my_messages",
            "get_pending",
            "get_voice",
            "list_chats",
            "list_stale_profiles",
            "mark_read",
            "read_history",
            "save_chat_notes",
            "save_profile",
            "send_message",
            "send_status",
            "set_category",
            "set_label",
            "set_watch_level",
            "status",
            "transcribe_voice",
        ]
    );
}
