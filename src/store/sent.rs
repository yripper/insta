//! Ids of the messages insta sent (metadata only), so voice learning studies
//! what the user typed themselves, not drafts Claude wrote.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::params;

use super::Store;
use crate::ig::types::Micros;

impl Store {
    pub fn record_sent(&self, item_id: &str, thread_id: &str, ts: Micros) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO sent_items (item_id, thread_id, ts) VALUES (?1, ?2, ?3)",
            params![item_id, thread_id, ts],
        )?;
        Ok(())
    }

    pub fn sent_ids(&self, thread_id: &str) -> Result<HashSet<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT item_id FROM sent_items WHERE thread_id = ?1")?;
        let rows = stmt.query_map([thread_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
