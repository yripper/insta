//! Voice: tone profiles per category, notes per chat (both Claude-written
//! style descriptions), and `own_activity`, the ids and times of the user's
//! own messages insta has seen, used only to count how much they've written
//! since a profile was saved. Personal data: never log bodies.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use super::Store;
use crate::ig::types::Micros;

/// Older own-activity rows can't affect staleness (anything saved before the
/// 21-day window is stale by age anyway), so they are pruned.
pub const OWN_ACTIVITY_KEEP_SECS: i64 = 22 * 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceEntry {
    pub body: String,
    /// Unix seconds.
    pub updated_at: i64,
}

const COUNT_IN_THREAD: &str = "SELECT COUNT(*) FROM own_activity o
     WHERE o.thread_id = ?1 AND o.ts > ?2
     AND NOT EXISTS (SELECT 1 FROM sent_items x WHERE x.item_id = o.item_id)";

/// Counts by the thread's *current* category, so recategorizing a chat moves
/// its activity with it.
const COUNT_IN_CATEGORY: &str = "SELECT COUNT(*) FROM own_activity o
     LEFT JOIN thread_state s ON s.thread_id = o.thread_id
     WHERE COALESCE(s.category, 'general') = ?1 AND o.ts > ?2
     AND NOT EXISTS (SELECT 1 FROM sent_items x WHERE x.item_id = o.item_id)";

impl Store {
    pub fn profile(&self, category: &str) -> Result<Option<VoiceEntry>> {
        self.voice_entry("SELECT body, updated_at FROM tone_profiles WHERE category = ?1", category)
    }

    pub fn save_profile(&self, category: &str, body: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO tone_profiles (category, body, updated_at) VALUES (?1, ?2, ?3)",
            params![category, body, now],
        )?;
        Ok(())
    }

    /// (category, updated_at) only, so listing never exposes profile text.
    pub fn profile_dates(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT category, updated_at FROM tone_profiles ORDER BY category")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn notes(&self, thread_id: &str) -> Result<Option<VoiceEntry>> {
        self.voice_entry("SELECT body, updated_at FROM chat_notes WHERE thread_id = ?1", thread_id)
    }

    pub fn save_notes(&self, thread_id: &str, body: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO chat_notes (thread_id, body, updated_at) VALUES (?1, ?2, ?3)",
            params![thread_id, body, now],
        )?;
        Ok(())
    }

    fn voice_entry(&self, sql: &str, key: &str) -> Result<Option<VoiceEntry>> {
        Ok(self
            .conn()
            .query_row(sql, [key], |r| Ok(VoiceEntry { body: r.get(0)?, updated_at: r.get(1)? }))
            .optional()?)
    }

    /// Records own messages seen in a thread (ids and times only). Seeing the
    /// same message twice counts once.
    pub fn record_own_activity(&self, thread_id: &str, items: &[(String, Micros)], now: i64) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (id, ts) in items {
            tx.execute(
                "INSERT OR IGNORE INTO own_activity (item_id, thread_id, ts) VALUES (?1, ?2, ?3)",
                params![id, thread_id, ts],
            )?;
        }
        tx.execute("DELETE FROM own_activity WHERE ts < ?1", [(now - OWN_ACTIVITY_KEEP_SECS) * 1_000_000])?;
        tx.commit()?;
        Ok(())
    }

    /// Own messages (not sent by insta) in a thread strictly after `since` (unix seconds).
    pub fn own_count_in_thread(&self, thread_id: &str, since: i64) -> Result<u32> {
        Ok(self.conn().query_row(COUNT_IN_THREAD, params![thread_id, since * 1_000_000], |r| r.get(0))?)
    }

    /// Own messages (not sent by insta) across a category strictly after `since` (unix seconds).
    pub fn own_count_in_category(&self, category: &str, since: i64) -> Result<u32> {
        Ok(self.conn().query_row(COUNT_IN_CATEGORY, params![category, since * 1_000_000], |r| r.get(0))?)
    }
}
