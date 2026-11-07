//! In-memory `InstagramApi` for tests. Records every call so tests can assert
//! what was (and wasn't) sent to Instagram. Fake ids and text only.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;

use super::api::{IgResult, InstagramApi};
use super::types::{IgError, InboxPage, Me, Message, MessageKind, SentMessage, Thread, ThreadPage, User};

pub const ME_ID: &str = "U000";

#[derive(Default)]
pub struct FakeData {
    /// Newest first, like Instagram.
    pub threads: Vec<Thread>,
    /// Per thread, newest first.
    pub messages: HashMap<String, Vec<Message>>,
    pub media: HashMap<String, Vec<u8>>,
    /// When set, every call fails with this error.
    pub fail_with: Option<IgError>,
    /// Each entry fails the next call whose recorded string starts with the prefix, once.
    pub fail_once: VecDeque<(String, IgError)>,
    pub next_item: u64,
}

pub struct FakeInstagram {
    pub data: Mutex<FakeData>,
    calls: Mutex<Vec<String>>,
}

pub fn me() -> Me {
    Me { user_id: ME_ID.into(), username: "me".into() }
}

pub fn user(id: &str, username: &str, full_name: &str) -> User {
    User { id: id.into(), username: username.into(), full_name: full_name.into() }
}

pub fn dm(id: &str, peer: User, last_ts: i64, last_from_me: bool) -> Thread {
    Thread {
        id: id.into(),
        is_group: false,
        peer: Some(peer),
        last_activity_ts: last_ts,
        last_item_from_me: last_from_me,
        unread: !last_from_me,
        muted_in_app: false,
    }
}

pub fn group(id: &str, last_ts: i64) -> Thread {
    Thread {
        id: id.into(),
        is_group: true,
        peer: None,
        last_activity_ts: last_ts,
        last_item_from_me: false,
        unread: true,
        muted_in_app: false,
    }
}

pub fn text(id: &str, ts: i64, from: &str, body: &str) -> Message {
    Message {
        id: id.into(),
        ts,
        from_me: from == ME_ID,
        sender_id: from.into(),
        kind: MessageKind::Text,
        text: Some(body.into()),
        link: None,
        media_url: None,
        duration_ms: None,
    }
}

pub fn voice(id: &str, ts: i64, from: &str, url: &str, duration_ms: u64) -> Message {
    Message {
        kind: MessageKind::Voice,
        text: None,
        media_url: Some(url.into()),
        duration_ms: Some(duration_ms),
        ..text(id, ts, from, "")
    }
}

impl FakeInstagram {
    pub fn new(data: FakeData) -> FakeInstagram {
        FakeInstagram { data: Mutex::new(FakeData { next_item: 900, ..data }), calls: Mutex::new(vec![]) }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn count(&self, prefix: &str) -> usize {
        self.calls().iter().filter(|c| c.starts_with(prefix)).count()
    }

    fn record(&self, call: String) -> IgResult<()> {
        self.calls.lock().unwrap().push(call.clone());
        let mut d = self.data.lock().unwrap();
        if let Some(pos) = d.fail_once.iter().position(|(p, _)| call.starts_with(p.as_str())) {
            let (_, e) = d.fail_once.remove(pos).expect("position is in range");
            return Err(e);
        }
        match &d.fail_with {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
}

/// Cursor = index of the first item of the page.
fn page<T: Clone>(all: &[T], cursor: Option<&str>, limit: usize) -> (Vec<T>, Option<String>) {
    let start: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
    let items: Vec<T> = all.iter().skip(start).take(limit).cloned().collect();
    let next = (start + limit < all.len()).then(|| (start + limit).to_string());
    (items, next)
}

#[async_trait]
impl InstagramApi for FakeInstagram {
    async fn whoami(&self) -> IgResult<Me> {
        self.record("whoami".into())?;
        Ok(me())
    }

    async fn inbox(&self, cursor: Option<&str>, limit: usize) -> IgResult<InboxPage> {
        self.record(format!("inbox {}", cursor.unwrap_or("-")))?;
        let d = self.data.lock().unwrap();
        let (threads, next_cursor) = page(&d.threads, cursor, limit);
        Ok(InboxPage { threads, next_cursor })
    }

    async fn thread(&self, thread_id: &str, cursor: Option<&str>, limit: usize) -> IgResult<ThreadPage> {
        self.record(format!("thread {thread_id} {}", cursor.unwrap_or("-")))?;
        let d = self.data.lock().unwrap();
        let Some(all) = d.messages.get(thread_id) else {
            return Err(IgError::NotFound);
        };
        let (messages, older_cursor) = page(all, cursor, limit);
        Ok(ThreadPage { messages, older_cursor })
    }

    async fn send_text(&self, thread_id: &str, body: &str) -> IgResult<SentMessage> {
        self.record(format!("send_text {thread_id}"))?;
        let mut d = self.data.lock().unwrap();
        d.next_item += 1;
        let id = format!("I{}", d.next_item);
        let ts = d.messages.get(thread_id).and_then(|m| m.first()).map_or(1_000, |m| m.ts + 1);
        d.messages.entry(thread_id.to_string()).or_default().insert(0, text(&id, ts, ME_ID, body));
        Ok(SentMessage { item_id: id, ts })
    }

    async fn mark_seen(&self, thread_id: &str, item_id: &str) -> IgResult<()> {
        self.record(format!("mark_seen {thread_id} {item_id}"))
    }

    async fn fetch_media(&self, url: &str, max_bytes: usize) -> IgResult<Vec<u8>> {
        self.record(format!("fetch_media {url}"))?;
        let d = self.data.lock().unwrap();
        let bytes = d.media.get(url).cloned().ok_or(IgError::NotFound)?;
        if bytes.len() > max_bytes {
            return Err(IgError::Http(format!("media is larger than {max_bytes} bytes")));
        }
        Ok(bytes)
    }
}
