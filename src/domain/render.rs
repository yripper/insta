//! How messages look to Claude. Non-text items become short markers
//! (`[reel] <link>`, `[voice 0:12] …`), so Claude knows what is there without
//! insta fetching any media.

use serde::Serialize;

use crate::ig::types::{Message, MessageKind, Micros, User};

/// `"me"`, or who the other person is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Sender {
    Me(&'static str),
    Peer { id: String, username: Option<String>, name: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderedMessage {
    pub id: String,
    /// Microseconds; pass the newest one as `seen_until`.
    pub ts: Micros,
    /// The same time, readable (UTC).
    pub at: String,
    pub from: Sender,
    pub kind: MessageKind,
    pub text: String,
}

pub fn render_message(m: &Message, peer: Option<&User>) -> RenderedMessage {
    RenderedMessage {
        id: m.id.clone(),
        ts: m.ts,
        at: rfc3339(m.ts),
        from: sender(m, peer),
        kind: m.kind.clone(),
        text: body(m),
    }
}

fn sender(m: &Message, peer: Option<&User>) -> Sender {
    if m.from_me {
        return Sender::Me("me");
    }
    match peer.filter(|p| p.id == m.sender_id) {
        Some(p) => Sender::Peer {
            id: p.id.clone(),
            username: Some(p.username.clone()),
            name: (!p.full_name.is_empty()).then(|| p.full_name.clone()),
        },
        None => Sender::Peer { id: m.sender_id.clone(), username: None, name: None },
    }
}

/// The message text, then the marker for whatever else it carries.
pub fn body(m: &Message) -> String {
    let text = m.text.as_deref().unwrap_or("").trim();
    match marker(m) {
        Some(mk) if text.is_empty() => mk,
        Some(mk) => format!("{text}\n{mk}"),
        None => text.to_string(),
    }
}

pub fn marker(m: &Message) -> Option<String> {
    let with_link = |tag: &str| match &m.link {
        Some(l) => format!("[{tag}] {l}"),
        None => format!("[{tag}]"),
    };
    Some(match &m.kind {
        MessageKind::Text => return None,
        // The link is usually inside the text already; don't repeat it.
        MessageKind::Link => match &m.link {
            Some(l) if !m.text.as_deref().unwrap_or("").contains(l.as_str()) => format!("[link] {l}"),
            _ => return None,
        },
        MessageKind::Reel => with_link("reel"),
        MessageKind::Post => with_link("post"),
        MessageKind::Story => with_link("story"),
        MessageKind::Gif => "[gif]".to_string(),
        MessageKind::Voice => match m.duration_ms {
            Some(ms) => format!("[voice {}] — call transcribe_voice", duration(ms)),
            None => "[voice] — call transcribe_voice".to_string(),
        },
        MessageKind::Image => "[image]".to_string(),
        MessageKind::Video => "[video]".to_string(),
        MessageKind::Like => "[like]".to_string(),
        MessageKind::Other(t) => format!("[{t}]"),
    })
}

/// `m:ss`, rounding to the nearest second.
pub fn duration(ms: u64) -> String {
    let secs = (ms + 500) / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn rfc3339(micros: Micros) -> String {
    chrono::DateTime::from_timestamp_micros(micros)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| micros.to_string())
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
