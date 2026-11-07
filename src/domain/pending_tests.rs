use super::*;
use crate::ig::fake::{dm, group, user};

const NOW: i64 = 1_800_000_000;

fn cand(id: &str, ts: i64, from_me: bool) -> Candidate {
    Candidate { thread: dm(id, user("U001", "ana", ""), ts, from_me), state: ThreadState::default(), label: None }
}

fn label(kind: LabelKind, set_at_ts: i64, snooze_until: Option<i64>) -> Option<Label> {
    Some(Label { label: kind, snooze_until, note: None, set_at_ts })
}

fn ids(v: Vec<Candidate>) -> Vec<String> {
    v.into_iter().map(|c| c.thread.id).collect()
}

#[test]
fn pending_when_other_person_spoke_last_newest_first() {
    let got = select_pending(vec![cand("TH001", 10, false), cand("TH002", 30, false), cand("TH003", 20, true)], NOW);
    assert_eq!(ids(got), ["TH002", "TH001"]);
}

#[test]
fn groups_and_peerless_threads_are_never_pending() {
    let g = Candidate { thread: group("TH009", 50), state: ThreadState::default(), label: None };
    let mut peerless = cand("TH008", 40, false);
    peerless.thread.peer = None;
    assert!(select_pending(vec![g, peerless], NOW).is_empty());
}

#[test]
fn muted_locally_or_in_app_unless_overridden() {
    let mut local = cand("TH001", 10, false);
    local.state.watch = Some(WatchLevel::Muted);
    let mut in_app = cand("TH002", 10, false);
    in_app.thread.muted_in_app = true;
    let mut override_normal = cand("TH003", 10, false);
    override_normal.thread.muted_in_app = true;
    override_normal.state.watch = Some(WatchLevel::Normal);
    assert_eq!(ids(select_pending(vec![local, in_app, override_normal], NOW)), ["TH003"]);
}

#[test]
fn done_and_waiting_hide_until_a_newer_incoming_message() {
    for kind in [LabelKind::Done, LabelKind::Waiting] {
        let mut same = cand("TH001", 10, false);
        same.label = label(kind, 10, None);
        assert!(!is_pending(&same, NOW), "{kind:?} hides the message it was set on");
        let mut newer = cand("TH001", 11, false);
        newer.label = label(kind, 10, None);
        assert!(is_pending(&newer, NOW), "{kind:?} cleared by a newer incoming message");
        assert!(!label_active(newer.label.as_ref().unwrap(), &newer.thread, NOW));
    }
}

#[test]
fn done_label_stays_active_when_i_wrote_last() {
    let t = dm("TH001", user("U001", "ana", ""), 99, true);
    assert!(label_active(&label(LabelKind::Done, 10, None).unwrap(), &t, NOW));
}

#[test]
fn snooze_survives_new_messages_until_its_time() {
    let mut c = cand("TH001", 999, false);
    c.label = label(LabelKind::Snoozed, 10, Some(NOW + 60));
    assert!(!is_pending(&c, NOW), "new message doesn't wake a snooze");
    assert!(is_pending(&c, NOW + 60), "snooze ends at snooze_until");
    let mut no_time = cand("TH001", 999, false);
    no_time.label = label(LabelKind::Snoozed, 10, None);
    assert!(is_pending(&no_time, NOW));
}
