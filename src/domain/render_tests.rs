use super::*;
use crate::ig::fake::{ME_ID, text, user, voice};

fn with_kind(kind: MessageKind, body: Option<&str>, link: Option<&str>) -> Message {
    Message {
        kind,
        text: body.map(str::to_string),
        link: link.map(str::to_string),
        ..text("I001", 1_700_000_000_000_000, "U001", "")
    }
}

#[test]
fn me_renders_as_me_and_peer_with_username_and_name() {
    let peer = user("U001", "ana", "Ana Example");
    let mine = render_message(&text("I001", 1, ME_ID, "hola"), Some(&peer));
    assert_eq!(serde_json::to_value(&mine.from).unwrap(), serde_json::json!("me"));
    let theirs = render_message(&text("I002", 2, "U001", "hey"), Some(&peer));
    assert_eq!(
        serde_json::to_value(&theirs.from).unwrap(),
        serde_json::json!({"id": "U001", "username": "ana", "name": "Ana Example"})
    );
    let stranger = render_message(&text("I003", 3, "U009", "?"), Some(&peer));
    assert_eq!(
        serde_json::to_value(&stranger.from).unwrap(),
        serde_json::json!({"id": "U009", "username": null, "name": null})
    );
}

#[test]
fn markers_for_every_non_text_kind() {
    let cases = [
        (with_kind(MessageKind::Reel, None, Some("https://example.com/r/1")), "[reel] https://example.com/r/1"),
        (with_kind(MessageKind::Post, None, None), "[post]"),
        (
            with_kind(MessageKind::Story, Some("lol"), Some("https://example.com/s/1")),
            "lol\n[story] https://example.com/s/1",
        ),
        (with_kind(MessageKind::Gif, None, None), "[gif]"),
        (with_kind(MessageKind::Image, None, None), "[image]"),
        (with_kind(MessageKind::Video, None, None), "[video]"),
        (with_kind(MessageKind::Like, None, None), "[like]"),
        (with_kind(MessageKind::Other("poll".into()), None, None), "[poll]"),
        (with_kind(MessageKind::Text, Some("  plain  "), None), "plain"),
    ];
    for (m, want) in cases {
        assert_eq!(body(&m), want);
    }
}

#[test]
fn voice_marker_shows_duration_and_points_to_transcribe() {
    let v = voice("I001", 1, "U001", "https://cdn.example.com/a.mp4", 12_400);
    assert_eq!(body(&v), "[voice 0:12] — call transcribe_voice");
    let long = Message { duration_ms: Some(75_000), ..v.clone() };
    assert_eq!(body(&long), "[voice 1:15] — call transcribe_voice");
    let unknown = Message { duration_ms: None, ..v };
    assert_eq!(body(&unknown), "[voice] — call transcribe_voice");
}

#[test]
fn link_marker_only_when_text_lacks_the_url() {
    let inside = with_kind(MessageKind::Link, Some("see https://example.com"), Some("https://example.com"));
    assert_eq!(body(&inside), "see https://example.com");
    let bare = with_kind(MessageKind::Link, None, Some("https://example.com"));
    assert_eq!(body(&bare), "[link] https://example.com");
}

#[test]
fn duration_and_time_formatting() {
    assert_eq!(duration(0), "0:00");
    assert_eq!(duration(59_600), "1:00");
    assert_eq!(rfc3339(1_700_000_000_000_000), "2023-11-14T22:13:20Z");
    assert_eq!(rfc3339(i64::MAX), i64::MAX.to_string());
}
