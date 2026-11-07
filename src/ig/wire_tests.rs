//! Fixtures mirror the captured shapes (docs/endpoints.md) with fake ids and text.

use serde_json::json;

use super::*;

const ME: &str = "F000";
const PEER: &str = "F001";

fn msg(id: &str, sender: &str, ts_ms: i64, content_type: &str, content: Value, text: &str) -> Value {
    json!({"node": {"id": id, "message_id": id, "sender_fbid": sender, "timestamp_ms": ts_ms.to_string(),
        "content_type": content_type, "text_body": text, "content": content}, "cursor": "c"})
}

fn text(id: &str, sender: &str, ts_ms: i64, body: &str) -> Value {
    msg(id, sender, ts_ms, "TEXT", json!({"__typename": "SlideMessageText", "text_body": body}), body)
}

fn thread(id: &str, group: bool, previews: Vec<Value>, my_watermark_ms: i64) -> Value {
    json!({"node": {"as_ig_direct_thread": {
        "id": id, "thread_key": format!("K{id}"), "is_group": group, "is_muted": false, "marked_as_unread": false,
        "last_activity_timestamp_ms": "2000",
        "users": [{"id": "U001", "username": "peer", "full_name": "Peer One"}],
        "viewer": {"id": "U000", "interop_messaging_user_fbid": ME},
        "slide_read_receipts": [{"participant_fbid": ME, "watermark_timestamp_ms": my_watermark_ms.to_string()},
                                 {"participant_fbid": PEER, "watermark_timestamp_ms": "0"}],
        "slide_messages": {"edges": previews}
    }}})
}

fn inbox(threads: Vec<Value>, next: bool) -> Value {
    json!({"data": {"get_slide_mailbox_for_iris_subscription": {"id": "MB1",
        "threads_by_folder": {"edges": threads, "page_info": {"end_cursor": "CUR1", "has_next_page": next}}}}})
}

#[test]
fn dtsg_is_extracted_from_both_markers() {
    assert_eq!(extract_dtsg(r#"x"DTSGInitialData",[],{"token":"AbC:1"}y"#).as_deref(), Some("AbC:1"));
    assert_eq!(extract_dtsg(r#"{"dtsg":{"token":"Zz"}}"#).as_deref(), Some("Zz"));
    assert_eq!(extract_dtsg("<html>login</html>"), None);
    assert_eq!(extract_dtsg(r#""DTSGInitialData",[],{"token":""}"#), None);
}

#[test]
fn inbox_parses_threads_viewer_and_cursor() {
    let body = inbox(
        vec![
            thread("TH001", false, vec![text("I2", PEER, 1500, "hi"), text("I1", ME, 1000, "yo")], 1200),
            thread("TH002", false, vec![text("I4", ME, 1900, "ok"), text("I3", PEER, 1800, "q")], 1800),
            thread("TH003", true, vec![], 0),
        ],
        true,
    );
    let p = parse_inbox(&body).unwrap();
    assert_eq!(p.viewer, Some(Viewer { interop_fbid: ME.into(), mailbox_id: "MB1".into() }));
    assert_eq!(p.first_thread_key.as_deref(), Some("KTH001"));
    assert_eq!(p.page.next_cursor.as_deref(), Some("CUR1"));
    let [a, b, g] = &p.page.threads[..] else { panic!("3 threads") };
    assert!(a.unread && !a.last_item_from_me, "incoming 1500 > my watermark 1200");
    assert_eq!(a.peer.as_ref().unwrap().username, "peer");
    assert_eq!(a.last_activity_ts, 2_000_000);
    assert!(!b.unread && b.last_item_from_me, "I replied last and saw their message");
    assert!(g.is_group && g.peer.is_none());
}

#[test]
fn inbox_without_next_page_has_no_cursor_and_pagination_root_works() {
    let mut body = inbox(vec![], false);
    let mb = body["data"]["get_slide_mailbox_for_iris_subscription"].take();
    body["data"] = json!({"fetch__SlideMailbox": mb});
    let p = parse_inbox(&body).unwrap();
    assert!(p.page.threads.is_empty() && p.page.next_cursor.is_none() && p.viewer.is_none());
}

#[test]
fn marked_as_unread_counts_as_unread() {
    let mut t = thread("TH001", false, vec![text("I1", ME, 1000, "x")], 1000);
    t["node"]["as_ig_direct_thread"]["marked_as_unread"] = json!(true);
    assert!(parse_inbox(&inbox(vec![t], false)).unwrap().page.threads[0].unread);
}

fn page(edges: Vec<Value>, next: bool) -> Value {
    json!({"data": {"fetch__SlideThread": {"as_ig_direct_thread": {"id": "TH001",
        "slide_messages": {"edges": edges, "page_info": {"end_cursor": "OLD1", "has_next_page": next}}}}}})
}

#[test]
fn messages_map_kinds_links_and_voice() {
    let edges = vec![
        text("I1", ME, 5000, "hola"),
        msg(
            "I2",
            PEER,
            4000,
            "AUDIOS",
            json!({"__typename": "SlideMessageAudiosContent",
            "audio_attachments": [{"attachment_cdn_url": "https://cdn.fbsbx.com/v.mp4?oe=1", "playable_duration_ms": 12000}]}),
            "",
        ),
        msg(
            "I3",
            PEER,
            3000,
            "MESSAGE_INLINE_SHARE",
            json!({"__typename": "SlideMessageXMAContent", "xma_text_body": "",
            "xma": {"__typename": "SlideMessagePortraitXMA", "target_url": "/reel/ABC/?id=1_2&is_sponsored=false"}}),
            "",
        ),
        msg(
            "I4",
            PEER,
            2900,
            "MESSAGE_INLINE_SHARE",
            json!({"__typename": "SlideMessageXMAContent",
            "xma": {"__typename": "SlideMessageStandardXMA", "target_url": "https://www.instagram.com/p/XYZ/", "caption_body_text": "cap"}}),
            "",
        ),
        msg(
            "I5",
            PEER,
            2800,
            "MONTAGE_SHARE_XMA",
            json!({"__typename": "SlideMessageXMAContent",
            "xma": {"__typename": "SlideMessageExpiredPlaceholderXMA"}}),
            "jaja buena",
        ),
        msg(
            "I6",
            PEER,
            2700,
            "INSTAGRAM_MESSAGING_ANIMATED_IMAGE",
            json!({"__typename": "SlideMessageAnimatedMediaContent"}),
            "",
        ),
        msg(
            "I7",
            PEER,
            2600,
            "REACTION_LOG_XMAT",
            json!({"__typename": "SlideMessageAdminText", "is_reaction_action_log": true,
            "text_fragments": [{"plaintext": "Peer reacted"}]}),
            "",
        ),
        msg("I8", PEER, 2500, "IMAGES", json!({"__typename": "SlideMessageImageContent"}), ""),
        msg("I9", PEER, 2400, "SOMETHING_NEW", json!({"__typename": "SlideMessageFutureThing"}), ""),
        msg(
            "I10",
            PEER,
            2300,
            "MESSAGE_INLINE_SHARE",
            json!({"__typename": "SlideMessageXMAContent",
            "xma": {"__typename": "SlideMessagePlaceholderXMA", "title_text": "Unavailable"}}),
            "",
        ),
    ];
    let p = parse_messages(&page(edges, true), ME).unwrap();
    assert_eq!(p.older_cursor.as_deref(), Some("OLD1"));
    let k: Vec<&MessageKind> = p.messages.iter().map(|m| &m.kind).collect();
    assert_eq!(
        k,
        vec![
            &MessageKind::Text,
            &MessageKind::Voice,
            &MessageKind::Reel,
            &MessageKind::Post,
            &MessageKind::Story,
            &MessageKind::Gif,
            &MessageKind::Like,
            &MessageKind::Image,
            &MessageKind::Other("something_new".into()),
            &MessageKind::Other("unavailable_share".into()),
        ]
    );
    let m = &p.messages;
    assert!(m[0].from_me && !m[1].from_me);
    assert_eq!(m[1].sender_id, PEER, "no sender object: falls back to the fbid");
    assert_eq!(m[0].ts, 5_000_000);
    assert_eq!(m[1].media_url.as_deref(), Some("https://cdn.fbsbx.com/v.mp4?oe=1"));
    assert_eq!(m[1].duration_ms, Some(12_000));
    assert_eq!(m[2].link.as_deref(), Some("https://www.instagram.com/reel/ABC/"));
    assert_eq!(m[3].text.as_deref(), Some("cap"));
    assert_eq!(m[4].text.as_deref(), Some("jaja buena"), "story reply keeps the reply text");
    assert_eq!(m[6].text.as_deref(), Some("Peer reacted"));
}

#[test]
fn unknown_viewer_never_marks_from_me() {
    let p = parse_messages(&page(vec![text("I1", "", 1, "x")], false), "").unwrap();
    assert!(!p.messages[0].from_me);
    assert!(p.older_cursor.is_none());
}

#[test]
fn missing_thread_is_not_found() {
    assert_eq!(parse_messages(&json!({"data": {"fetch__SlideThread": null}}), ME).unwrap_err(), IgError::NotFound);
}

#[test]
fn sent_parses_message_id_and_ts() {
    let body = json!({"data": {"xig_direct_text_send_with_slide_messaging_response": {"message_id": "mid.$X", "id": "mid.$X", "timestamp_ms": "7000"}}});
    assert_eq!(parse_sent(&body).unwrap(), SentMessage { item_id: "mid.$X".into(), ts: 7_000_000 });
    assert!(matches!(parse_sent(&json!({"data": {}})), Err(IgError::Decode(_))));
}

#[test]
fn graphql_errors_are_classified() {
    assert_eq!(graphql_error(&json!({"message": "checkpoint_required", "status": "fail"})), Some(IgError::Challenge));
    assert_eq!(graphql_error(&json!({"message": "challenge_required"})), Some(IgError::Challenge));
    assert_eq!(graphql_error(&json!({"message": "feedback_required"})), Some(IgError::Challenge));
    assert_eq!(graphql_error(&json!({"message": "login_required"})), Some(IgError::NeedsLogin));
    assert_eq!(
        graphql_error(&json!({"errors": [{"message": "Please wait a few minutes before you try again."}]})),
        Some(IgError::RateLimited { retry_after_secs: None })
    );
    assert!(matches!(
        graphql_error(&json!({"errors": [{"message": "missing_required_variable_value"}], "data": null})),
        Some(IgError::Http(_))
    ));
    assert_eq!(graphql_error(&json!({"errors": [{"message": "partial"}], "data": {"x": 1}})), None);
    assert_eq!(graphql_error(&json!({"data": {"x": 1}})), None);
}

#[test]
fn viewer_username_from_thread_detail() {
    let body = json!({"data": {"get_slide_thread_nullable": {"as_ig_direct_thread": {"viewer": {"username": "me"}}}}});
    assert_eq!(parse_viewer_username(&body).as_deref(), Some("me"));
    assert_eq!(parse_viewer_username(&json!({"data": {}})), None);
}

#[test]
fn sender_id_prefers_the_instagram_id() {
    let mut e = text("I1", PEER, 1, "x");
    e["node"]["sender"] = json!({"igid": "U001", "id": "F001"});
    let p = parse_messages(&page(vec![e], false), ME).unwrap();
    assert_eq!(p.messages[0].sender_id, "U001");
    assert!(!p.messages[0].from_me);
}
