//! What each MCP tool does, independent of MCP. Every method that takes a
//! chat accepts an id, @username or part of a name and refuses to guess
//! between several.

use std::sync::Arc;

use serde::Serialize;

use super::clock::Timer;
use super::directory::{Directory, display_name, to_cached};
use super::errors::{Error, Result};
use super::pending::{Candidate, is_muted, label_active, select_pending};
use super::render::{RenderedMessage, duration, render_message, rfc3339};
use super::send_queue::{Enqueued, QueueStatus, SendQueue};
use super::voice::{
    CATEGORY_SAMPLE, CHAT_SAMPLE, OwnMessage, VoiceContext, observe_own, own_messages, own_messages_in_category,
    stale_profiles, valid_category, voice_context,
};
use crate::ig::InstagramApi;
use crate::ig::types::{Me, Message, MessageKind, Micros, Thread, User};
use crate::store::{CachedThread, Label, LabelKind, Store, WatchLevel};
use crate::whisper::{MAX_AUDIO_BYTES, Transcriber, audio_file_name};

/// Pending chats whose messages get fetched; each is a paced Instagram call.
const PENDING_DETAIL: usize = 5;
const PENDING_MESSAGES: usize = 10;
const NOTE_MAX: usize = 500;
const PROFILE_MAX: usize = 4_000;
const CHAT_NOTES_MAX: usize = 2_000;
/// How far back transcribe_voice looks for the item.
const VOICE_SEARCH_PAGES: usize = 3;
const VOICE_SEARCH_PAGE: usize = 50;

#[derive(Debug, Clone, Serialize)]
pub struct ChatSummary {
    pub id: String,
    pub name: String,
    pub username: String,
    pub last_activity: String,
    pub last_from_me: bool,
    pub unread: bool,
    pub muted: bool,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

#[derive(Debug, Serialize)]
pub struct PendingEntry {
    pub id: String,
    pub name: String,
    pub last_activity: String,
    /// Instagram shows the newest incoming message as not seen by the user.
    pub unread: bool,
    /// Their messages since the user's last one, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<RenderedMessage>,
    /// Older messages of theirs exist: read_history before replying.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub more_before: bool,
    /// Messages not fetched (beyond the first few chats): read_history to see them.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub not_fetched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PendingReport {
    pub chats: Vec<PendingEntry>,
}

#[derive(Debug, Serialize)]
pub struct HistoryReport {
    pub chat: String,
    pub chat_id: String,
    /// Oldest first.
    pub messages: Vec<RenderedMessage>,
    /// Pass this as `seen_until` when replying.
    pub newest_ts: Option<Micros>,
    /// Pass as `cursor` for older messages.
    pub older_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MarkReport {
    pub chat: String,
    pub marked_seen_until: String,
}

#[derive(Debug, Serialize)]
pub struct LabelReport {
    pub chat: String,
    pub label: Option<Label>,
}

#[derive(Debug, Serialize)]
pub struct VoiceReport {
    #[serde(flatten)]
    pub context: VoiceContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_messages_in_chat: Option<Vec<OwnMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_messages_in_category: Option<Vec<OwnMessage>>,
}

#[derive(Debug, Serialize)]
pub struct Transcript {
    pub chat: String,
    pub item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Saved {
    pub ok: bool,
}

pub struct Actions {
    api: Arc<dyn InstagramApi>,
    store: Arc<Store>,
    me: Me,
    dir: Directory,
    queue: Arc<SendQueue>,
    whisper: Arc<dyn Transcriber>,
    timer: Arc<dyn Timer>,
}

fn thread_name(t: &Thread) -> String {
    to_cached(t, 0).map_or_else(|| t.id.clone(), |c| display_name(&c))
}

fn peer_of(c: &CachedThread) -> User {
    User { id: c.peer_id.clone(), username: c.username.clone(), full_name: c.full_name.clone() }
}

fn check_len(what: &str, body: &str, max: usize) -> Result<()> {
    let n = body.chars().count();
    if n == 0 || n > max {
        return Err(Error::Invalid(format!("{what} must be 1 to {max} characters (got {n})")));
    }
    Ok(())
}

/// Their messages since the user's last one, oldest first, from a newest-first page.
fn since_my_last(page: &[Message]) -> (Vec<Message>, bool) {
    let theirs: Vec<Message> = page.iter().take_while(|m| !m.from_me).cloned().collect();
    let more = theirs.len() == page.len() && !page.is_empty();
    (theirs.into_iter().rev().collect(), more)
}

impl Actions {
    pub fn new(
        api: Arc<dyn InstagramApi>,
        store: Arc<Store>,
        me: Me,
        queue: Arc<SendQueue>,
        whisper: Arc<dyn Transcriber>,
        timer: Arc<dyn Timer>,
    ) -> Actions {
        let dir = Directory::new(api.clone(), store.clone());
        Actions { api, store, me, dir, queue, whisper, timer }
    }

    pub fn me(&self) -> &Me {
        &self.me
    }

    fn now(&self) -> i64 {
        self.timer.now_secs()
    }

    async fn chat(&self, query: &str) -> Result<CachedThread> {
        self.dir.resolve_one(query, self.now()).await
    }

    fn candidate(&self, t: Thread) -> Result<Candidate> {
        Ok(Candidate { state: self.store.thread_state(&t.id)?, label: self.store.label(&t.id)?, thread: t })
    }

    fn summary(&self, c: Candidate) -> ChatSummary {
        ChatSummary {
            id: c.thread.id.clone(),
            name: thread_name(&c.thread),
            username: c.thread.peer.as_ref().map(|p| p.username.clone()).unwrap_or_default(),
            last_activity: rfc3339(c.thread.last_activity_ts),
            last_from_me: c.thread.last_item_from_me,
            unread: c.thread.unread,
            muted: is_muted(&c),
            category: c.state.category,
            label: c.label,
        }
    }

    /// Recent 1:1 chats, newest first. Muted ones only show when searched for.
    pub async fn list_chats(&self, query: Option<&str>, limit: usize) -> Result<Vec<ChatSummary>> {
        let threads = self.dir.refresh(self.now()).await?;
        let q = query.map(|q| q.trim().trim_start_matches('@').to_lowercase()).filter(|q| !q.is_empty());
        let mut out = Vec::new();
        for t in threads {
            let s = self.summary(self.candidate(t)?);
            let hit = match &q {
                Some(q) => s.name.to_lowercase().contains(q) || s.id == *q,
                None => !s.muted,
            };
            if hit {
                out.push(s);
            }
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Chats waiting on the user, newest first, with their latest messages.
    pub async fn pending(&self) -> Result<PendingReport> {
        let now = self.now();
        let mut cands = Vec::new();
        for t in self.dir.refresh(now).await? {
            let c = self.candidate(t)?;
            // Labels that a newer message (or time) ended are cleared for good.
            if let Some(l) = &c.label
                && !label_active(l, &c.thread, now)
            {
                self.store.clear_label(&c.thread.id)?;
            }
            cands.push(c);
        }
        let mut chats = Vec::new();
        for (i, c) in select_pending(cands, now).into_iter().enumerate() {
            let mut entry = self.pending_entry(&c.thread);
            if i < PENDING_DETAIL {
                match soft(self.recent_from_them(&c.thread).await)? {
                    Ok((msgs, more)) => (entry.messages, entry.more_before) = (msgs, more),
                    Err(e) => entry.error = Some(e),
                }
            } else {
                entry.not_fetched = true;
            }
            chats.push(entry);
        }
        Ok(PendingReport { chats })
    }

    fn pending_entry(&self, t: &Thread) -> PendingEntry {
        let name = thread_name(t);
        PendingEntry {
            id: t.id.clone(),
            name,
            last_activity: rfc3339(t.last_activity_ts),
            unread: t.unread,
            messages: vec![],
            more_before: false,
            not_fetched: false,
            error: None,
        }
    }

    async fn recent_from_them(&self, t: &Thread) -> Result<(Vec<RenderedMessage>, bool)> {
        let page = self.api.thread(&t.id, None, PENDING_MESSAGES).await?;
        observe_own(&self.store, &t.id, &page.messages, self.now())?;
        let (msgs, more) = since_my_last(&page.messages);
        Ok((msgs.iter().map(|m| render_message(m, t.peer.as_ref())).collect(), more))
    }

    pub async fn read_history(&self, chat: &str, limit: usize, cursor: Option<&str>) -> Result<HistoryReport> {
        let c = self.chat(chat).await?;
        let page = self.api.thread(&c.thread_id, cursor, limit).await?;
        observe_own(&self.store, &c.thread_id, &page.messages, self.now())?;
        let peer = peer_of(&c);
        let newest_ts = page.messages.iter().map(|m| m.ts).max();
        let messages = page.messages.iter().rev().map(|m| render_message(m, Some(&peer))).collect();
        Ok(HistoryReport {
            chat: display_name(&c),
            chat_id: c.thread_id,
            messages,
            newest_ts,
            older_cursor: page.older_cursor,
        })
    }

    async fn newest(&self, thread_id: &str) -> Result<Option<Message>> {
        Ok(self.api.thread(thread_id, None, 1).await?.messages.into_iter().next())
    }

    /// Marks the chat seen (the other person sees "Seen") up to `item_id` or the newest message.
    pub async fn mark_read(&self, chat: &str, item_id: Option<&str>) -> Result<MarkReport> {
        let c = self.chat(chat).await?;
        let item = match item_id {
            Some(i) => i.to_string(),
            None => self.newest(&c.thread_id).await?.ok_or_else(|| Error::Invalid("chat has no messages".into()))?.id,
        };
        self.api.mark_seen(&c.thread_id, &item).await?;
        Ok(MarkReport { chat: display_name(&c), marked_seen_until: item })
    }

    pub async fn send(&self, chat: &str, text: &str, seen_until: Option<Micros>) -> Result<Enqueued> {
        let c = self.chat(chat).await?;
        self.queue.enqueue(&c.thread_id, &display_name(&c), text, seen_until)
    }

    pub fn send_status(&self, job_id: Option<u64>) -> Result<QueueStatus> {
        self.queue.status(job_id)
    }

    pub async fn set_label(
        &self,
        chat: &str,
        label: LabelKind,
        note: Option<String>,
        snooze_until: Option<i64>,
    ) -> Result<LabelReport> {
        if let Some(n) = &note {
            check_len("note", n, NOTE_MAX)?;
        }
        match (label, snooze_until) {
            (LabelKind::Snoozed, None) => return Err(Error::Invalid("snoozed needs snooze_until".into())),
            (LabelKind::Snoozed, Some(t)) if t <= self.now() => {
                return Err(Error::Invalid("snooze_until must be in the future".into()));
            }
            (LabelKind::Done | LabelKind::Waiting, Some(_)) => {
                return Err(Error::Invalid("snooze_until only goes with label snoozed".into()));
            }
            _ => {}
        }
        let c = self.chat(chat).await?;
        let set_at_ts = self.newest(&c.thread_id).await?.map_or(0, |m| m.ts);
        let l = Label { label, snooze_until, note, set_at_ts };
        self.store.set_label(&c.thread_id, &l)?;
        Ok(LabelReport { chat: display_name(&c), label: Some(l) })
    }

    pub async fn clear_label(&self, chat: &str) -> Result<LabelReport> {
        let c = self.chat(chat).await?;
        self.store.clear_label(&c.thread_id)?;
        Ok(LabelReport { chat: display_name(&c), label: None })
    }

    pub async fn set_watch(&self, chat: &str, level: WatchLevel) -> Result<Saved> {
        let c = self.chat(chat).await?;
        self.store.set_watch(&c.thread_id, level)?;
        Ok(Saved { ok: true })
    }

    pub async fn set_category(&self, chat: &str, category: &str) -> Result<Saved> {
        if !valid_category(category) {
            return Err(Error::Invalid("category must match ^[a-z0-9_-]{1,32}$".into()));
        }
        let c = self.chat(chat).await?;
        self.store.set_category(&c.thread_id, category)?;
        Ok(Saved { ok: true })
    }

    pub async fn get_voice(&self, chat: &str) -> Result<VoiceReport> {
        let c = self.chat(chat).await?;
        let now = self.now();
        let context = voice_context(&self.store, &c.thread_id, now)?;
        let mut report = VoiceReport { my_messages_in_chat: None, my_messages_in_category: None, context };
        if report.context.needs_learning || report.context.notes_stale {
            report.my_messages_in_chat =
                Some(own_messages(self.api.as_ref(), &self.store, &c.thread_id, CHAT_SAMPLE, now).await?);
        }
        if report.context.needs_learning {
            let cat = report.context.category.clone();
            report.my_messages_in_category = Some(
                own_messages_in_category(self.api.as_ref(), &self.store, &cat, &c.thread_id, CATEGORY_SAMPLE, now)
                    .await?,
            );
        }
        Ok(report)
    }

    /// Exactly one of `chat` or `category`.
    pub async fn get_my_messages(
        &self,
        chat: Option<&str>,
        category: Option<&str>,
        limit: usize,
    ) -> Result<Vec<OwnMessage>> {
        let now = self.now();
        match (chat, category) {
            (Some(chat), None) => {
                let c = self.chat(chat).await?;
                own_messages(self.api.as_ref(), &self.store, &c.thread_id, limit, now).await
            }
            (None, Some(cat)) if valid_category(cat) => {
                own_messages_in_category(self.api.as_ref(), &self.store, cat, "", limit, now).await
            }
            (None, Some(_)) => Err(Error::Invalid("category must match ^[a-z0-9_-]{1,32}$".into())),
            _ => Err(Error::Invalid("give exactly one of chat or category".into())),
        }
    }

    pub fn save_profile(&self, category: &str, body: &str) -> Result<Saved> {
        if !valid_category(category) {
            return Err(Error::Invalid("category must match ^[a-z0-9_-]{1,32}$".into()));
        }
        check_len("profile", body, PROFILE_MAX)?;
        self.store.save_profile(category, body, self.now())?;
        Ok(Saved { ok: true })
    }

    pub async fn save_chat_notes(&self, chat: &str, body: &str) -> Result<Saved> {
        check_len("notes", body, CHAT_NOTES_MAX)?;
        let c = self.chat(chat).await?;
        self.store.save_notes(&c.thread_id, body, self.now())?;
        Ok(Saved { ok: true })
    }

    pub fn list_stale_profiles(&self) -> Result<Vec<String>> {
        stale_profiles(&self.store, self.now())
    }

    /// Fetches a voice note's audio and transcribes it. Costs OpenAI money;
    /// nothing is written to disk.
    pub async fn transcribe_voice(&self, chat: &str, item_id: &str) -> Result<Transcript> {
        self.whisper.check_ready().await?;
        let c = self.chat(chat).await?;
        let m = self.find_item(&c.thread_id, item_id).await?;
        let url = match (&m.kind, &m.media_url) {
            (MessageKind::Voice, Some(u)) => u.clone(),
            (MessageKind::Voice, None) => return Err(Error::Invalid("that voice note has no audio url".into())),
            _ => return Err(Error::Invalid(format!("item {item_id} is not a voice note"))),
        };
        let audio = self.api.fetch_media(&url, MAX_AUDIO_BYTES).await?;
        let name = audio_file_name(&audio);
        let text = self.whisper.transcribe(audio, name).await?;
        Ok(Transcript { chat: display_name(&c), item_id: m.id, duration: m.duration_ms.map(duration), text })
    }

    async fn find_item(&self, thread_id: &str, item_id: &str) -> Result<Message> {
        let mut cursor: Option<String> = None;
        for _ in 0..VOICE_SEARCH_PAGES {
            let page = self.api.thread(thread_id, cursor.as_deref(), VOICE_SEARCH_PAGE).await?;
            if let Some(m) = page.messages.into_iter().find(|m| m.id == item_id) {
                return Ok(m);
            }
            cursor = page.older_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Err(Error::Invalid(format!("item {item_id} not found in the recent messages of this chat")))
    }
}

/// Splits per-chat failures (reported, the rest goes on) from a dead session
/// (aborts: nothing else will work either).
fn soft<T>(r: Result<T>) -> Result<std::result::Result<T, String>> {
    match r {
        Err(e) if e.is_fatal() => Err(e),
        Err(e) => Ok(Err(e.to_string())),
        Ok(v) => Ok(Ok(v)),
    }
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
