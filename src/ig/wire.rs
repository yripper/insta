//! Instagram web GraphQL JSON → domain types. Pure functions over
//! `serde_json::Value` so a renamed or missing field degrades one message
//! instead of failing a whole page. Shapes are documented in
//! `docs/endpoints.md` (captured from instagram.com/direct, 2026-11-07).

use serde_json::Value;

use super::types::{IgError, InboxPage, Message, MessageKind, Micros, SentMessage, Thread, ThreadPage, User};

const WEB: &str = "https://www.instagram.com";

/// Ids that identify the viewer in Instagram's responses. Message senders are
/// keyed by the *interop fbid*, not the Instagram user id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub interop_fbid: String,
    pub mailbox_id: String,
}

/// A parsed inbox page plus what it taught us about the viewer.
pub struct ParsedInbox {
    pub page: InboxPage,
    pub viewer: Option<Viewer>,
    /// `thread_key` of the newest thread: the only id `IGDThreadDetailQuery` takes.
    pub first_thread_key: Option<String>,
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn non_empty(v: Option<&str>) -> Option<String> {
    v.filter(|t| !t.is_empty()).map(str::to_owned)
}

/// Instagram sends millisecond timestamps as strings.
fn ms_to_micros(v: Option<&str>) -> Micros {
    v.and_then(|t| t.parse::<i64>().ok()).unwrap_or(0).saturating_mul(1_000)
}

/// Turns an error-shaped GraphQL body into the matching `IgError`.
/// Returns `None` when the body has data.
pub fn graphql_error(body: &Value) -> Option<IgError> {
    // REST-style failures ({"message": "checkpoint_required", "status": "fail"}).
    if let Some(msg) = s(body, "message") {
        return Some(classify_message(msg));
    }
    let errors = body.get("errors").and_then(Value::as_array)?;
    if body.get("data").is_some_and(|d| !d.is_null()) {
        // Partial errors next to data: the data is still usable.
        return None;
    }
    let msg = errors.iter().filter_map(|e| s(e, "message")).collect::<Vec<_>>().join("; ");
    Some(classify_message(&msg))
}

fn classify_message(msg: &str) -> IgError {
    let m = msg.to_lowercase();
    if m.contains("checkpoint") || m.contains("challenge") || m.contains("feedback_required") {
        IgError::Challenge
    } else if m.contains("login_required") || m.contains("not logged in") {
        IgError::NeedsLogin
    } else if m.contains("please wait") || m.contains("rate limit") || m.contains("too many") {
        IgError::RateLimited { retry_after_secs: None }
    } else {
        IgError::Http(format!("Instagram GraphQL error: {msg}"))
    }
}

/// `"DTSGInitialData",[],{"token":"…"}` in the inbox HTML: the per-session
/// token every GraphQL POST needs as `fb_dtsg`.
pub fn extract_dtsg(html: &str) -> Option<String> {
    for marker in ["\"DTSGInitialData\",[],{\"token\":\"", "\"dtsg\":{\"token\":\""] {
        if let Some(start) = html.find(marker).map(|i| i + marker.len())
            && let Some(len) = html[start..].find('"')
            && len > 0
        {
            return Some(html[start..start + len].to_owned());
        }
    }
    None
}

/// `PolarisDirectInboxQuery` (first page) and
/// `IGDThreadListOffMsysPaginationQuery` (later pages) share the
/// `threads_by_folder` shape under different roots.
pub fn parse_inbox(body: &Value) -> Result<ParsedInbox, IgError> {
    parse_inbox_inner(body, None)
}

/// Like `parse_inbox`, for a page whose threads don't carry the viewer: "from
/// me" and "unread" are judged against the viewer we already know.
pub fn parse_inbox_with(body: &Value, known: &Viewer) -> Result<ParsedInbox, IgError> {
    parse_inbox_inner(body, Some(known))
}

fn parse_inbox_inner(body: &Value, known: Option<&Viewer>) -> Result<ParsedInbox, IgError> {
    let data = body.get("data").ok_or_else(|| IgError::Decode("inbox: no data".into()))?;
    let mailbox = data
        .get("get_slide_mailbox_for_iris_subscription")
        .or_else(|| data.get("fetch__SlideMailbox"))
        .ok_or_else(|| IgError::Decode("inbox: no mailbox".into()))?;
    let folder = mailbox.get("threads_by_folder").ok_or_else(|| IgError::Decode("inbox: no threads".into()))?;
    let nodes: Vec<&Value> = folder
        .get("edges")
        .and_then(Value::as_array)
        .map(|e| e.iter().filter_map(|x| x.pointer("/node/as_ig_direct_thread")).collect())
        .unwrap_or_default();

    let viewer = nodes.first().and_then(|t| {
        Some(Viewer {
            interop_fbid: s(t.get("viewer")?, "interop_messaging_user_fbid")?.to_owned(),
            mailbox_id: s(mailbox, "id")?.to_owned(),
        })
    });
    let my_fbid = viewer.as_ref().or(known).map(|v| v.interop_fbid.as_str()).unwrap_or("");
    let threads = nodes.iter().filter_map(|t| parse_thread(t, my_fbid)).collect();
    let page_info = folder.get("page_info");
    let has_next = page_info.and_then(|p| p.get("has_next_page")).and_then(Value::as_bool).unwrap_or(false);
    let next_cursor = if has_next { page_info.and_then(|p| non_empty(s(p, "end_cursor"))) } else { None };
    Ok(ParsedInbox {
        page: InboxPage { threads, next_cursor },
        viewer,
        first_thread_key: nodes.first().and_then(|t| non_empty(s(t, "thread_key"))),
    })
}

fn parse_thread(t: &Value, my_fbid: &str) -> Option<Thread> {
    let id = s(t, "id")?.to_owned();
    let is_group = t.get("is_group").and_then(Value::as_bool).unwrap_or(false);
    let peer =
        if is_group { None } else { t.get("users").and_then(Value::as_array).and_then(|u| u.first()).map(parse_user) };
    let previews: Vec<&Value> = t
        .pointer("/slide_messages/edges")
        .and_then(Value::as_array)
        .map(|e| e.iter().filter_map(|x| x.get("node")).collect())
        .unwrap_or_default();
    // Previews come newest first.
    let newest = previews.first();
    let last_item_from_me = newest.is_some_and(|n| s(n, "sender_fbid") == Some(my_fbid));
    let newest_incoming_ms = previews
        .iter()
        .find(|n| s(n, "sender_fbid") != Some(my_fbid))
        .map(|n| ms_to_micros(s(n, "timestamp_ms")) / 1_000);
    let my_watermark_ms = t
        .get("slide_read_receipts")
        .and_then(Value::as_array)
        .and_then(|r| r.iter().find(|x| s(x, "participant_fbid") == Some(my_fbid)))
        .map(|x| ms_to_micros(s(x, "watermark_timestamp_ms")) / 1_000)
        .unwrap_or(0);
    let marked_unread = t.get("marked_as_unread").and_then(Value::as_bool).unwrap_or(false);
    let unread = marked_unread || newest_incoming_ms.is_some_and(|ms| ms > my_watermark_ms);
    Some(Thread {
        id,
        is_group,
        peer,
        last_activity_ts: ms_to_micros(s(t, "last_activity_timestamp_ms")),
        last_item_from_me,
        unread,
        muted_in_app: t.get("is_muted").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn parse_user(u: &Value) -> User {
    User {
        id: s(u, "id").unwrap_or_default().to_owned(),
        username: s(u, "username").unwrap_or_default().to_owned(),
        full_name: s(u, "full_name").unwrap_or_default().to_owned(),
    }
}

/// `IGDMessageListOffMsysQuery`: newest first, `end_cursor` pages older.
pub fn parse_messages(body: &Value, my_fbid: &str) -> Result<ThreadPage, IgError> {
    let msgs = body.pointer("/data/fetch__SlideThread/as_ig_direct_thread/slide_messages").ok_or(IgError::NotFound)?;
    let messages = msgs
        .get("edges")
        .and_then(Value::as_array)
        .map(|e| e.iter().filter_map(|x| x.get("node")).filter_map(|n| parse_message(n, my_fbid)).collect())
        .unwrap_or_default();
    let has_next = msgs.pointer("/page_info/has_next_page").and_then(Value::as_bool).unwrap_or(false);
    let older_cursor =
        if has_next { non_empty(msgs.pointer("/page_info/end_cursor").and_then(Value::as_str)) } else { None };
    Ok(ThreadPage { messages, older_cursor })
}

fn parse_message(n: &Value, my_fbid: &str) -> Option<Message> {
    let sender_fbid = s(n, "sender_fbid").unwrap_or_default();
    // `sender.igid` is the Instagram user id (what `Thread.peer.id` holds);
    // `sender_fbid` is a separate interop id, only good for telling "me".
    let sender_id = n.pointer("/sender/igid").and_then(Value::as_str).unwrap_or(sender_fbid).to_owned();
    let mut m = Message {
        id: s(n, "id").or_else(|| s(n, "message_id"))?.to_owned(),
        ts: ms_to_micros(s(n, "timestamp_ms")),
        from_me: !my_fbid.is_empty() && sender_fbid == my_fbid,
        sender_id,
        kind: MessageKind::Text,
        text: non_empty(s(n, "text_body")),
        link: None,
        media_url: None,
        duration_ms: None,
    };
    let content = n.get("content").cloned().unwrap_or(Value::Null);
    let content_type = s(n, "content_type").unwrap_or("UNKNOWN");
    match s(&content, "__typename").unwrap_or("") {
        "SlideMessageText" => {}
        "SlideMessageAdminText" => {
            let line: Vec<&str> = content
                .get("text_fragments")
                .and_then(Value::as_array)
                .map(|f| f.iter().filter_map(|x| s(x, "plaintext")).collect())
                .unwrap_or_default();
            m.text = non_empty(Some(&line.join("")));
            let reaction = content.get("is_reaction_action_log").and_then(Value::as_bool).unwrap_or(false);
            m.kind = if reaction { MessageKind::Like } else { MessageKind::Other("system".into()) };
        }
        "SlideMessageXMAContent" => apply_xma(&mut m, &content, content_type),
        "SlideMessageAudiosContent" => {
            let a = content.pointer("/audio_attachments/0");
            m.kind = MessageKind::Voice;
            m.media_url = a.and_then(|a| non_empty(s(a, "attachment_cdn_url")));
            m.duration_ms = a.and_then(|a| a.get("playable_duration_ms")).and_then(Value::as_u64);
        }
        "SlideMessageImageContent" | "SlideMessageRavenImageContent" => m.kind = MessageKind::Image,
        "SlideMessageAnimatedMediaContent" => m.kind = MessageKind::Gif,
        t if t.contains("Video") => m.kind = MessageKind::Video,
        _ if m.text.is_some() && content_type == "TEXT" => {}
        _ => m.kind = MessageKind::Other(content_type.to_lowercase()),
    }
    Some(m)
}

/// Shared reels, posts and stories arrive as "XMA" cards.
fn apply_xma(m: &mut Message, content: &Value, content_type: &str) {
    let xma = content.get("xma").cloned().unwrap_or(Value::Null);
    let target = s(&xma, "target_url").filter(|u| !u.is_empty());
    let path = target.map(|u| u.trim_start_matches(WEB)).unwrap_or("");
    // Reel and post links carry tracking params; the path alone opens them.
    let clean = if path.starts_with("/reel/") || path.starts_with("/p/") { path.split('?').next() } else { None };
    m.link = target.map(|u| match clean {
        Some(p) => format!("{WEB}{p}"),
        None if u.starts_with('/') => format!("{WEB}{u}"),
        None => u.to_owned(),
    });
    m.kind = if content_type.starts_with("MONTAGE") || path.starts_with("/stories/") {
        MessageKind::Story
    } else if path.starts_with("/reel/") {
        MessageKind::Reel
    } else if path.starts_with("/p/") {
        MessageKind::Post
    } else if target.is_some() {
        MessageKind::Link
    } else {
        MessageKind::Other("unavailable_share".into())
    };
    if m.text.is_none() {
        m.text = non_empty(s(content, "xma_text_body"))
            .or_else(|| non_empty(s(&xma, "caption_body_text")))
            .or_else(|| non_empty(s(&xma, "title_text")));
    }
}

/// `IGDirectTextSendMutation` response.
pub fn parse_sent(body: &Value) -> Result<SentMessage, IgError> {
    let r = body
        .pointer("/data/xig_direct_text_send_with_slide_messaging_response")
        .ok_or_else(|| IgError::Decode("send: no response object".into()))?;
    let item_id =
        s(r, "message_id").or_else(|| s(r, "id")).ok_or_else(|| IgError::Decode("send: no message id".into()))?;
    Ok(SentMessage { item_id: item_id.to_owned(), ts: ms_to_micros(s(r, "timestamp_ms")) })
}

/// `IGDThreadDetailQuery` is the only response with the viewer's username.
pub fn parse_viewer_username(body: &Value) -> Option<String> {
    non_empty(
        body.pointer("/data/get_slide_thread_nullable/as_ig_direct_thread/viewer/username").and_then(Value::as_str),
    )
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
