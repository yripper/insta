use anyhow::Result;
use rusqlite::Connection;

/// Each entry upgrades `user_version` N to N+1. Append only; never edit a shipped one.
/// Times: `*_ts` columns are Instagram item times in microseconds; `fetched_at`,
/// `updated_at` and `snooze_until` are unix seconds.
const MIGRATIONS: &[&str] = &["CREATE TABLE thread_state (
        thread_id TEXT PRIMARY KEY,
        watch TEXT,
        category TEXT NOT NULL DEFAULT 'general'
    );
    CREATE TABLE labels (
        thread_id TEXT PRIMARY KEY,
        label TEXT NOT NULL,
        snooze_until INTEGER,
        note TEXT,
        set_at_ts INTEGER NOT NULL
    );
    CREATE TABLE thread_cache (
        thread_id TEXT PRIMARY KEY,
        peer_id TEXT NOT NULL,
        username TEXT NOT NULL,
        full_name TEXT NOT NULL,
        fetched_at INTEGER NOT NULL
    );
    CREATE TABLE tone_profiles (
        category TEXT PRIMARY KEY,
        body TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE chat_notes (
        thread_id TEXT PRIMARY KEY,
        body TEXT NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE TABLE own_activity (
        item_id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL,
        ts INTEGER NOT NULL
    );
    CREATE INDEX own_activity_thread_ts ON own_activity (thread_id, ts);
    CREATE TABLE sent_items (
        item_id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL,
        ts INTEGER NOT NULL
    );
    CREATE TABLE kv (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );"];

pub fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {}; COMMIT;", i + 1))?;
    }
    Ok(())
}
