//! Writing in the user's voice: one tone profile per category and notes per
//! chat, both written by Claude. insta never stores message text: the user's
//! own messages are fetched live from Instagram when Claude needs to learn,
//! and staleness is counted from message ids/times only (`own_activity`).

use std::collections::HashSet;

use serde::Serialize;

use super::errors::Result;
use super::render::rfc3339;
use crate::ig::InstagramApi;
use crate::ig::types::{Message, MessageKind};
use crate::store::Store;

/// A profile or notes entry is stale after this many own messages or this long.
pub const STALE_OWN_MESSAGES: u32 = 30;
pub const STALE_AGE_SECS: i64 = 21 * 86_400;
pub const CHAT_SAMPLE: usize = 60;
pub const CATEGORY_SAMPLE: usize = 40;
pub const CATEGORY_THREADS: usize = 3;
const OWN_PAGE: usize = 50;
/// Each page is a paced Instagram call: bound how far back a sample digs.
const OWN_MAX_PAGES: usize = 3;

pub fn is_stale(updated_at: i64, own_since: u32, now: i64) -> bool {
    own_since >= STALE_OWN_MESSAGES || now - updated_at >= STALE_AGE_SECS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceContext {
    pub category: String,
    pub profile: Option<String>,
    pub notes: Option<String>,
    pub profile_stale: bool,
    /// Also true when the chat has no notes yet: they need writing.
    pub notes_stale: bool,
    /// The category profile is missing or stale.
    pub needs_learning: bool,
}

pub fn profile_stale(store: &Store, category: &str, updated_at: i64, now: i64) -> Result<bool> {
    Ok(is_stale(updated_at, store.own_count_in_category(category, updated_at)?, now))
}

pub fn voice_context(store: &Store, thread_id: &str, now: i64) -> Result<VoiceContext> {
    let category = store.thread_state(thread_id)?.category;
    let profile = store.profile(&category)?;
    let notes = store.notes(thread_id)?;
    let profile_stale = match &profile {
        Some(p) => profile_stale(store, &category, p.updated_at, now)?,
        None => false,
    };
    let notes_stale = match &notes {
        Some(n) => is_stale(n.updated_at, store.own_count_in_thread(thread_id, n.updated_at)?, now),
        None => true,
    };
    Ok(VoiceContext {
        needs_learning: profile.is_none() || profile_stale,
        category,
        profile: profile.map(|p| p.body),
        notes: notes.map(|n| n.body),
        profile_stale,
        notes_stale,
    })
}

/// Categories whose saved profile is due for a refresh.
pub fn stale_profiles(store: &Store, now: i64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (category, updated_at) in store.profile_dates()? {
        if profile_stale(store, &category, updated_at, now)? {
            out.push(category);
        }
    }
    Ok(out)
}

/// `^[a-z0-9_-]{1,32}$`
pub fn valid_category(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnMessage {
    pub chat_id: String,
    pub id: String,
    pub at: String,
    pub text: String,
}

/// Remembers which own messages exist (ids and times, no text) so staleness
/// can be counted later. Called on every thread page insta reads.
pub fn observe_own(store: &Store, thread_id: &str, msgs: &[Message], now: i64) -> Result<()> {
    let own: Vec<(String, i64)> = msgs.iter().filter(|m| m.from_me).map(|m| (m.id.clone(), m.ts)).collect();
    if !own.is_empty() {
        store.record_own_activity(thread_id, &own, now)?;
    }
    Ok(())
}

/// The user's own typed text messages, excluding what insta sent for them.
pub fn own_texts<'a>(msgs: &'a [Message], sent: &HashSet<String>) -> Vec<&'a Message> {
    msgs.iter()
        .filter(|m| m.from_me && !sent.contains(&m.id))
        .filter(|m| matches!(m.kind, MessageKind::Text | MessageKind::Link))
        .filter(|m| m.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        .collect()
}

/// Up to `limit` own messages from one thread, newest first, fetched live.
pub async fn own_messages(
    api: &dyn InstagramApi,
    store: &Store,
    thread_id: &str,
    limit: usize,
    now: i64,
) -> Result<Vec<OwnMessage>> {
    let sent = store.sent_ids(thread_id)?;
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..OWN_MAX_PAGES {
        let page = api.thread(thread_id, cursor.as_deref(), OWN_PAGE).await?;
        observe_own(store, thread_id, &page.messages, now)?;
        for m in own_texts(&page.messages, &sent) {
            out.push(OwnMessage {
                chat_id: thread_id.to_string(),
                id: m.id.clone(),
                at: rfc3339(m.ts),
                text: m.text.clone().unwrap_or_default(),
            });
        }
        cursor = page.older_cursor;
        if out.len() >= limit || cursor.is_none() {
            break;
        }
    }
    out.truncate(limit);
    Ok(out)
}

/// Up to `limit` own messages from at most `CATEGORY_THREADS` other threads of the category.
pub async fn own_messages_in_category(
    api: &dyn InstagramApi,
    store: &Store,
    category: &str,
    except: &str,
    limit: usize,
    now: i64,
) -> Result<Vec<OwnMessage>> {
    let mut out = Vec::new();
    for thread_id in store.threads_in_category(category, except, CATEGORY_THREADS)? {
        if out.len() >= limit {
            break;
        }
        out.extend(own_messages(api, store, &thread_id, limit - out.len(), now).await?);
    }
    Ok(out)
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;
