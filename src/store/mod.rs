//! SQLite store: triage state (watch level, category, labels), a metadata
//! cache (thread peers), voice profiles and notes (Claude-written style
//! descriptions), and message *ids* insta sent or saw. No message text, ever.

mod cache;
mod schema;
mod sent;
mod state;
mod voice;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::Result;
use rusqlite::Connection;

pub use cache::CachedThread;
pub use state::{DEFAULT_CATEGORY, Label, LabelKind, ThreadState, WatchLevel};
pub use voice::{OWN_ACTIVITY_KEEP_SECS, VoiceEntry};

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)?;
        // Holds names, triage state and voice profiles: owner-only, like the directory.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        schema::migrate(&conn)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent, so
        // keep serving instead of poisoning every later call.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests;
