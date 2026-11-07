//! The pending rule: a 1:1 chat is waiting on the user when the other person
//! spoke last and the user hasn't hidden it (muted, snoozed, or labeled
//! done/waiting since that message). Pure: no I/O.

use crate::ig::types::Thread;
use crate::store::{Label, LabelKind, ThreadState, WatchLevel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub thread: Thread,
    pub state: ThreadState,
    pub label: Option<Label>,
}

/// A local watch level wins; with none, an in-app mute counts as muted.
pub fn is_muted(c: &Candidate) -> bool {
    match c.state.watch {
        Some(w) => w == WatchLevel::Muted,
        None => c.thread.muted_in_app,
    }
}

/// Whether a label still applies. done/waiting end when a message from the
/// other person arrives after the label was set; snoozed ends only at its time.
pub fn label_active(label: &Label, thread: &Thread, now: i64) -> bool {
    match label.label {
        LabelKind::Snoozed => label.snooze_until.is_some_and(|until| until > now),
        LabelKind::Done | LabelKind::Waiting => thread.last_item_from_me || thread.last_activity_ts <= label.set_at_ts,
    }
}

pub fn is_pending(c: &Candidate, now: i64) -> bool {
    let t = &c.thread;
    !t.is_group
        && t.peer.is_some()
        && !t.last_item_from_me
        && !is_muted(c)
        && !c.label.as_ref().is_some_and(|l| label_active(l, t, now))
}

/// Pending chats, newest activity first.
pub fn select_pending(cands: Vec<Candidate>, now: i64) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = cands.into_iter().filter(|c| is_pending(c, now)).collect();
    out.sort_by(|a, b| b.thread.last_activity_ts.cmp(&a.thread.last_activity_ts));
    out
}

#[cfg(test)]
#[path = "pending_tests.rs"]
mod tests;
