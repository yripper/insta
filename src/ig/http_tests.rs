//! `HttpInstagram` against a wiremock server, with pacing off: a paused tokio
//! clock would auto-advance into reqwest's timeout while waiting on sockets.

use serde_json::json;
use wiremock::matchers::{body_string_contains, header, header_exists, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;

const HTML_OK: &str = r#"<html><script>["DTSGInitialData",[],{"token":"DT1"}]</script></html>"#;

fn creds() -> Credentials {
    Credentials { sessionid: "S1".into(), csrftoken: "C1".into(), ds_user_id: "U000".into() }
}

fn api(server: &MockServer) -> HttpInstagram {
    HttpInstagram::with_base(creds(), &server.uri(), vec!["127.0.0.1".into()]).unwrap().without_pacing()
}

fn inbox_body() -> Value {
    json!({"data": {"get_slide_mailbox_for_iris_subscription": {"id": "MB1",
    "threads_by_folder": {"page_info": {"end_cursor": "CUR1", "has_next_page": true}, "edges": [
        {"node": {"as_ig_direct_thread": {"id": "TH001", "thread_key": "K001", "is_group": false,
            "last_activity_timestamp_ms": "2000", "users": [{"id": "U001", "username": "peer", "full_name": "Peer"}],
            "viewer": {"id": "U000", "interop_messaging_user_fbid": "F000"},
            "slide_read_receipts": [], "slide_messages": {"edges": []}}}}
    ]}}}})
}

async fn mount_html(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/direct/inbox/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HTML_OK))
        .mount(server)
        .await;
}

async fn mount_op(server: &MockServer, op: Op, body: Value) {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains(format!("doc_id={}", op.doc_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn form(req: &Request) -> std::collections::HashMap<String, String> {
    url::form_urlencoded::parse(&req.body).into_owned().collect()
}

#[tokio::test]
async fn graphql_sends_the_captured_minimal_request() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(header("Sec-Fetch-Site", "same-origin"))
        .and(header("X-IG-App-ID", IG_APP_ID))
        .and(header_exists("Accept-Language"))
        .and(header("Cookie", "sessionid=S1; csrftoken=C1; ds_user_id=U000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(inbox_body()))
        .mount(&server)
        .await;
    let page = api(&server).inbox(None, 20).await.unwrap();
    assert_eq!(page.threads.len(), 1);
    assert_eq!(page.next_cursor.as_deref(), Some("MB1|CUR1"));
    let reqs = server.received_requests().await.unwrap();
    let f = form(reqs.iter().find(|r| r.method.as_str() == "POST").unwrap());
    assert_eq!(f["fb_dtsg"], "DT1");
    assert_eq!(f["doc_id"], INBOX.doc_id);
    assert_eq!(f["fb_api_req_friendly_name"], INBOX.name);
    let vars: Value = serde_json::from_str(&f["variables"]).unwrap();
    assert_eq!(vars["device_id_for_iris_subscription"].as_str().unwrap().len(), 36);
}

#[tokio::test]
async fn dtsg_is_fetched_once_and_reused() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    mount_op(&server, INBOX, inbox_body()).await;
    let a = api(&server);
    a.inbox(None, 20).await.unwrap();
    a.inbox(None, 20).await.unwrap();
    let gets = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "GET").count();
    assert_eq!(gets, 1);
}

#[tokio::test]
async fn html_answer_refetches_dtsg_once_then_gives_up() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
        .mount(&server)
        .await;
    let err = api(&server).inbox(None, 20).await.unwrap_err();
    assert!(matches!(err, IgError::Decode(_)), "{err:?}");
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.iter().filter(|r| r.method.as_str() == "GET").count(), 2);
    assert_eq!(reqs.iter().filter(|r| r.method.as_str() == "POST").count(), 2);
}

#[tokio::test]
async fn send_never_retries() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
        .mount(&server)
        .await;
    assert!(api(&server).send_text("TH001", "hola").await.is_err());
    let posts = server.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "POST").count();
    assert_eq!(posts, 1);
}

#[tokio::test]
async fn logged_out_inbox_page_means_needs_login() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html class=\"not-logged-in\"></html>"))
        .mount(&server)
        .await;
    assert_eq!(api(&server).inbox(None, 20).await.unwrap_err(), IgError::NeedsLogin);
}

#[tokio::test]
async fn challenge_redirect_and_rate_limit_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/direct/inbox/"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/challenge/xyz/"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/challenge/xyz/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    assert_eq!(api(&server).inbox(None, 20).await.unwrap_err(), IgError::Challenge);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "30"))
        .mount(&server)
        .await;
    assert_eq!(api(&server).inbox(None, 20).await.unwrap_err(), IgError::RateLimited { retry_after_secs: Some(30) });
}

#[tokio::test]
async fn checkpoint_json_is_a_challenge() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    mount_op(&server, INBOX, json!({"message": "checkpoint_required", "status": "fail"})).await;
    assert_eq!(api(&server).inbox(None, 20).await.unwrap_err(), IgError::Challenge);
}

#[tokio::test]
async fn thread_learns_viewer_then_marks_my_messages() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    mount_op(&server, INBOX, inbox_body()).await;
    mount_op(
        &server,
        MESSAGES,
        json!({"data": {"fetch__SlideThread": {"as_ig_direct_thread": {"id": "TH001",
        "slide_messages": {"page_info": {"end_cursor": "OLD", "has_next_page": true}, "edges": [
            {"node": {"id": "I2", "sender_fbid": "F000", "timestamp_ms": "3", "content_type": "TEXT", "text_body": "x",
                "content": {"__typename": "SlideMessageText"}}},
            {"node": {"id": "I1", "sender_fbid": "F001", "timestamp_ms": "2", "content_type": "TEXT", "text_body": "y",
                "content": {"__typename": "SlideMessageText"}}}]}}}}}),
    )
    .await;
    let p = api(&server).thread("TH001", Some("OLDER"), 20).await.unwrap();
    assert!(p.messages[0].from_me && !p.messages[1].from_me);
    assert_eq!(p.older_cursor.as_deref(), Some("OLD"));
    let reqs = server.received_requests().await.unwrap();
    let msg_req = reqs.iter().map(form).find(|f| f.get("doc_id").map(String::as_str) == Some(MESSAGES.doc_id)).unwrap();
    let vars: Value = serde_json::from_str(&msg_req["variables"]).unwrap();
    assert_eq!(vars["id"], "TH001");
    assert_eq!(vars["after"], "OLDER");
    assert_eq!(vars["first"], 20);
}

#[tokio::test]
async fn inbox_next_page_uses_mailbox_from_cursor() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    let mut b = inbox_body();
    let mb = b["data"]["get_slide_mailbox_for_iris_subscription"].take();
    b["data"] = json!({"fetch__SlideMailbox": mb});
    mount_op(&server, INBOX_PAGE, b).await;
    api(&server).inbox(Some("MB9|CURX"), 10).await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    let f = reqs.iter().map(form).find(|f| f.contains_key("variables")).unwrap();
    let vars: Value = serde_json::from_str(&f["variables"]).unwrap();
    assert_eq!(
        (vars["id"].as_str(), vars["cursor"].as_str(), vars["count"].as_u64()),
        (Some("MB9"), Some("CURX"), Some(10))
    );
    assert!(api(&server).inbox(Some("garbage"), 10).await.is_err());
}

#[tokio::test]
async fn send_and_mark_seen_use_captured_variables() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    mount_op(&server, SEND_TEXT, json!({"data": {"xig_direct_text_send_with_slide_messaging_response": {"message_id": "mid.$N", "id": "mid.$N", "timestamp_ms": "9"}}})).await;
    mount_op(&server, MARK_SEEN, json!({"data": {"xig_direct_item_seen_mutation_with_slide_messaging_response": {"watermark_timestamp_ms": "9"}}})).await;
    let a = api(&server);
    assert_eq!(a.send_text("TH001", "hola po").await.unwrap(), SentMessage { item_id: "mid.$N".into(), ts: 9_000 });
    a.mark_seen("TH001", "mid.$M").await.unwrap();
    let reqs = server.received_requests().await.unwrap();
    let vars: Vec<Value> =
        reqs.iter().map(form).filter_map(|f| f.get("variables").map(|v| serde_json::from_str(v).unwrap())).collect();
    assert_eq!(vars[0]["ig_thread_igid"], "TH001");
    assert_eq!(vars[0]["text"]["sensitive_string_value"], "hola po");
    assert_eq!(vars[0]["send_attribution"], "igd_web_chat_tab:in_thread");
    let otid: u64 = vars[0]["offline_threading_id"].as_str().unwrap().parse().unwrap();
    assert!(otid >> 22 > 1_700_000_000_000, "otid carries the send time in ms");
    assert_eq!(
        vars[1],
        json!({"metadata": {"ig_thread_igid": "TH001"}, "data": {"item_id": "", "message_id": "mid.$M"}})
    );
}

#[tokio::test]
async fn whoami_reads_username_from_thread_detail() {
    let server = MockServer::start().await;
    mount_html(&server).await;
    mount_op(&server, INBOX, inbox_body()).await;
    mount_op(
        &server,
        THREAD_DETAIL,
        json!({"data": {"get_slide_thread_nullable": {"as_ig_direct_thread": {"viewer": {"username": "me"}}}}}),
    )
    .await;
    assert_eq!(api(&server).whoami().await.unwrap(), Me { user_id: "U000".into(), username: "me".into() });
    let reqs = server.received_requests().await.unwrap();
    let f = reqs.iter().map(form).find(|f| f.get("doc_id").map(String::as_str) == Some(THREAD_DETAIL.doc_id)).unwrap();
    let vars: Value = serde_json::from_str(&f["variables"]).unwrap();
    assert_eq!(vars["thread_fbid"], "K001", "thread detail takes thread_key, not the thread id");
}

#[tokio::test]
async fn fetch_media_caps_size_sends_no_cookies_and_checks_host() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v.mp4"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 100]))
        .mount(&server)
        .await;
    let a = api(&server);
    let url = format!("{}/v.mp4", server.uri());
    assert_eq!(a.fetch_media(&url, 100).await.unwrap().len(), 100);
    assert!(a.fetch_media(&url, 99).await.is_err());
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs.iter().all(|r| !r.headers.contains_key("cookie")));
    assert!(a.fetch_media("https://evil.example.com/x.mp4", 100).await.is_err());
    assert!(a.fetch_media("not a url", 100).await.is_err());

    let real = HttpInstagram::new(creds()).unwrap();
    assert!(real.media_host_allowed(&reqwest::Url::parse("https://scontent.xx.fbsbx.com/v.mp4?oe=1").unwrap()));
    assert!(!real.media_host_allowed(&reqwest::Url::parse("http://scontent.xx.fbsbx.com/v.mp4").unwrap()));
    assert!(!real.media_host_allowed(&reqwest::Url::parse("https://fbsbx.com.evil.io/v.mp4").unwrap()));
}

#[test]
fn ids_and_cursors() {
    assert_eq!(offline_threading_id(1, 0), (1u64 << 22).to_string());
    assert_eq!(offline_threading_id(1, u64::MAX), ((1u64 << 22) | 0x3F_FFFF).to_string());
    assert_eq!(decode_inbox_cursor(&encode_inbox_cursor("MB1", "a|b")).unwrap(), ("MB1", "a|b"));
    let u = random_uuid();
    assert_eq!(u.len(), 36);
    assert_eq!(&u[14..15], "4");
}

#[tokio::test(start_paused = true)]
async fn pace_spaces_calls_1_5_to_3_seconds_apart() {
    let a = HttpInstagram::new(creds()).unwrap();
    let t0 = Instant::now();
    a.pace().await;
    assert_eq!(Instant::now(), t0, "first call goes immediately");
    a.pace().await;
    let waited = Instant::now() - t0;
    assert!(waited >= Duration::from_millis(MIN_GAP_MS) && waited <= Duration::from_millis(MAX_GAP_MS));
}
