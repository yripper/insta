//! Per-thread triage state: watch level, category and the label.

use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::Store;
use crate::ig::types::Micros;

pub const DEFAULT_CATEGORY: &str = "general";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WatchLevel {
    /// Shown in pending when the other person spoke last (even if muted in the app).
    Normal,
    /// Never pending, hidden from list_chats.
    Muted,
}

impl WatchLevel {
    fn as_str(self) -> &'static str {
        match self {
            WatchLevel::Normal => "normal",
            WatchLevel::Muted => "muted",
        }
    }

    fn parse(s: &str) -> Result<WatchLevel> {
        Ok(match s {
            "normal" => WatchLevel::Normal,
            "muted" => WatchLevel::Muted,
            other => bail!("unknown watch level in db: {other}"),
        })
    }
}

/// `watch: None` means the default: muted only when muted in the Instagram app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadState {
    pub watch: Option<WatchLevel>,
    pub category: String,
}

impl Default for ThreadState {
    fn default() -> Self {
        ThreadState { watch: None, category: DEFAULT_CATEGORY.to_string() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// Nothing owed. A new incoming message clears it.
    Done,
    /// The ball is in the other person's court. A new incoming message clears it.
    Waiting,
    /// Hidden until `snooze_until`, whatever arrives meanwhile.
    Snoozed,
}

impl LabelKind {
    fn as_str(self) -> &'static str {
        match self {
            LabelKind::Done => "done",
            LabelKind::Waiting => "waiting",
            LabelKind::Snoozed => "snoozed",
        }
    }

    fn parse(s: &str) -> Result<LabelKind> {
        Ok(match s {
            "done" => LabelKind::Done,
            "waiting" => LabelKind::Waiting,
            "snoozed" => LabelKind::Snoozed,
            other => bail!("unknown label in db: {other}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Label {
    pub label: LabelKind,
    /// Unix seconds; set for `snoozed` only.
    pub snooze_until: Option<i64>,
    pub note: Option<String>,
    /// Newest message ts in the thread when the label was set.
    pub set_at_ts: Micros,
}

type LabelRow = (String, Option<i64>, Option<String>, i64);

fn to_label((label, snooze_until, note, set_at_ts): LabelRow) -> Result<Label> {
    Ok(Label { label: LabelKind::parse(&label)?, snooze_until, note, set_at_ts })
}

impl Store {
    pub fn thread_state(&self, thread_id: &str) -> Result<ThreadState> {
        let row: Option<(Option<String>, String)> = self
            .conn()
            .query_row("SELECT watch, category FROM thread_state WHERE thread_id = ?1", [thread_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        match row {
            Some((w, category)) => {
                Ok(ThreadState { watch: w.as_deref().map(WatchLevel::parse).transpose()?, category })
            }
            None => Ok(ThreadState::default()),
        }
    }

    pub fn set_watch(&self, thread_id: &str, level: WatchLevel) -> Result<()> {
        self.conn().execute(
            "INSERT INTO thread_state (thread_id, watch) VALUES (?1, ?2)
             ON CONFLICT(thread_id) DO UPDATE SET watch = excluded.watch",
            params![thread_id, level.as_str()],
        )?;
        Ok(())
    }

    pub fn set_category(&self, thread_id: &str, category: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO thread_state (thread_id, category) VALUES (?1, ?2)
             ON CONFLICT(thread_id) DO UPDATE SET category = excluded.category",
            params![thread_id, category],
        )?;
        Ok(())
    }

    pub fn label(&self, thread_id: &str) -> Result<Option<Label>> {
        let row: Option<LabelRow> = self
            .conn()
            .query_row(
                "SELECT label, snooze_until, note, set_at_ts FROM labels WHERE thread_id = ?1",
                [thread_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(to_label).transpose()
    }

    pub fn set_label(&self, thread_id: &str, label: &Label) -> Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO labels (thread_id, label, snooze_until, note, set_at_ts)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![thread_id, label.label.as_str(), label.snooze_until, label.note, label.set_at_ts],
        )?;
        Ok(())
    }

    pub fn clear_label(&self, thread_id: &str) -> Result<bool> {
        Ok(self.conn().execute("DELETE FROM labels WHERE thread_id = ?1", [thread_id])? > 0)
    }
}
