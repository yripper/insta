//! Which 1:1 chat the user means ("TH001", "@ana", "ana"), backed by the
//! thread cache, which is refreshed from the inbox.

use std::sync::Arc;

use super::errors::{Error, Result};
use crate::ig::InstagramApi;
use crate::ig::types::Thread;
use crate::store::{CachedThread, Store};

/// Inbox reads are paced and Instagram is touchy: re-list at most every 10 min.
const CACHE_TTL: i64 = 600;
/// A name that isn't cached may be a brand-new chat: one forced refresh, but
/// not more than once a minute.
const MISS_REFRESH_AFTER: i64 = 60;
pub const INBOX_PAGE: usize = 20;
pub const INBOX_PAGES: usize = 3;

#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    One(CachedThread),
    Many(Vec<CachedThread>),
    None,
}

/// Pure resolution over the cache. Never guesses: several matches is `Many`.
pub fn resolve(query: &str, threads: &[CachedThread]) -> Resolved {
    let q = query.trim();
    if q.is_empty() {
        return Resolved::None;
    }
    if let Some(t) = threads.iter().find(|t| t.thread_id == q) {
        return Resolved::One(t.clone());
    }
    let by_username = q.starts_with('@');
    let q = q.trim_start_matches('@').to_lowercase();
    if q.is_empty() {
        return Resolved::None;
    }
    let names = |t: &CachedThread| -> Vec<String> {
        let mut n = vec![t.username.to_lowercase()];
        if !by_username && !t.full_name.is_empty() {
            n.push(t.full_name.to_lowercase());
        }
        n
    };
    let exact: Vec<CachedThread> = threads.iter().filter(|t| names(t).contains(&q)).cloned().collect();
    let pick = if exact.is_empty() {
        threads.iter().filter(|t| names(t).iter().any(|n| n.contains(&q))).cloned().collect()
    } else {
        exact
    };
    match pick.len() {
        0 => Resolved::None,
        1 => Resolved::One(pick.into_iter().next().expect("len 1")),
        _ => Resolved::Many(pick),
    }
}

/// Cache row for a 1:1 thread; groups and peerless threads have none.
pub fn to_cached(t: &Thread, now: i64) -> Option<CachedThread> {
    let peer = t.peer.as_ref().filter(|_| !t.is_group)?;
    Some(CachedThread {
        thread_id: t.id.clone(),
        peer_id: peer.id.clone(),
        username: peer.username.clone(),
        full_name: peer.full_name.clone(),
        fetched_at: now,
    })
}

/// `@username (Full Name)`, how a chat is shown to the user.
pub fn display_name(c: &CachedThread) -> String {
    if c.full_name.is_empty() { format!("@{}", c.username) } else { format!("@{} ({})", c.username, c.full_name) }
}

pub struct Directory {
    api: Arc<dyn InstagramApi>,
    store: Arc<Store>,
}

impl Directory {
    pub fn new(api: Arc<dyn InstagramApi>, store: Arc<Store>) -> Directory {
        Directory { api, store }
    }

    /// Reads the newest inbox pages and caches their 1:1 threads. Returns the
    /// 1:1 threads, newest first.
    pub async fn refresh(&self, now: i64) -> Result<Vec<Thread>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..INBOX_PAGES {
            let page = self.api.inbox(cursor.as_deref(), INBOX_PAGE).await?;
            out.extend(page.threads.into_iter().filter(|t| !t.is_group && t.peer.is_some()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let cached: Vec<CachedThread> = out.iter().filter_map(|t| to_cached(t, now)).collect();
        self.store.put_threads(&cached)?;
        Ok(out)
    }

    async fn refresh_if_older(&self, now: i64, max_age: i64) -> Result<bool> {
        let fresh = self.store.threads_fetched_at()?.is_some_and(|t| now - t < max_age);
        if !fresh {
            self.refresh(now).await?;
        }
        Ok(!fresh)
    }

    pub async fn resolve(&self, query: &str, now: i64) -> Result<Resolved> {
        let refreshed = self.refresh_if_older(now, CACHE_TTL).await?;
        let r = resolve(query, &self.store.cached_threads()?);
        if r != Resolved::None || refreshed || !self.refresh_if_older(now, MISS_REFRESH_AFTER).await? {
            return Ok(r);
        }
        Ok(resolve(query, &self.store.cached_threads()?))
    }

    pub async fn resolve_one(&self, query: &str, now: i64) -> Result<CachedThread> {
        match self.resolve(query, now).await? {
            Resolved::One(c) => Ok(c),
            Resolved::Many(cs) => Err(Error::Ambiguous {
                query: query.to_string(),
                candidates: cs.iter().map(|c| format!("{} ({})", display_name(c), c.thread_id)).collect(),
            }),
            Resolved::None => Err(Error::NotFound(query.to_string())),
        }
    }
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
