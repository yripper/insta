//! Metadata cache of 1:1 threads (who the peer is), and the challenge flag.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::Store;

const CHALLENGE_KEY: &str = "challenge_at";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedThread {
    pub thread_id: String,
    pub peer_id: String,
    pub username: String,
    pub full_name: String,
    pub fetched_at: i64,
}

impl Store {
    /// Upserts: insta reads only the newest inbox pages, so threads not seen
    /// this time are kept, not dropped.
    pub fn put_threads(&self, threads: &[CachedThread]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for t in threads {
            tx.execute(
                "INSERT OR REPLACE INTO thread_cache (thread_id, peer_id, username, full_name, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![t.thread_id, t.peer_id, t.username, t.full_name, t.fetched_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Most recently seen first.
    pub fn cached_threads(&self) -> Result<Vec<CachedThread>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT thread_id, peer_id, username, full_name, fetched_at FROM thread_cache
             ORDER BY fetched_at DESC, thread_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CachedThread {
                thread_id: r.get(0)?,
                peer_id: r.get(1)?,
                username: r.get(2)?,
                full_name: r.get(3)?,
                fetched_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn threads_fetched_at(&self) -> Result<Option<i64>> {
        Ok(self.conn().query_row("SELECT MAX(fetched_at) FROM thread_cache", [], |r| r.get(0))?)
    }

    /// Up to `limit` cached threads in `category` other than `except`, most recently seen first.
    pub fn threads_in_category(&self, category: &str, except: &str, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.thread_id FROM thread_cache c LEFT JOIN thread_state s ON s.thread_id = c.thread_id
             WHERE COALESCE(s.category, 'general') = ?1 AND c.thread_id != ?2
             ORDER BY c.fetched_at DESC, c.thread_id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![category, except, limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Persisted so a daemon restart doesn't hit Instagram again after a challenge.
    pub fn set_challenge(&self, at: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
            params![CHALLENGE_KEY, at.to_string()],
        )?;
        Ok(())
    }

    pub fn challenge_at(&self) -> Result<Option<i64>> {
        let raw: Option<String> =
            self.conn().query_row("SELECT value FROM kv WHERE key = ?1", [CHALLENGE_KEY], |r| r.get(0)).optional()?;
        Ok(raw.and_then(|v| v.parse().ok()))
    }

    pub fn clear_challenge(&self) -> Result<()> {
        self.conn().execute("DELETE FROM kv WHERE key = ?1", [CHALLENGE_KEY])?;
        Ok(())
    }
}
