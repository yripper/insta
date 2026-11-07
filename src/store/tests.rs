use super::*;

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;

fn cached(id: &str, user: &str, at: i64) -> CachedThread {
    CachedThread {
        thread_id: id.into(),
        peer_id: format!("P{id}"),
        username: user.into(),
        full_name: format!("{user} name"),
        fetched_at: at,
    }
}

#[test]
fn migrations_are_idempotent_and_db_is_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.db");
    Store::open(&path).unwrap().set_watch("TH001", WatchLevel::Muted).unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.thread_state("TH001").unwrap().watch, Some(WatchLevel::Muted));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn thread_state_defaults_and_keeps_watch_and_category_apart() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.thread_state("TH001").unwrap(), ThreadState { watch: None, category: "general".into() });
    s.set_category("TH001", "friends").unwrap();
    s.set_watch("TH001", WatchLevel::Normal).unwrap();
    assert_eq!(
        s.thread_state("TH001").unwrap(),
        ThreadState { watch: Some(WatchLevel::Normal), category: "friends".into() }
    );
    s.set_watch("TH002", WatchLevel::Muted).unwrap();
    assert_eq!(s.thread_state("TH002").unwrap().category, "general");
}

#[test]
fn labels_roundtrip_and_clear() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.label("TH001").unwrap(), None);
    let l = Label { label: LabelKind::Snoozed, snooze_until: Some(NOW), note: Some("later".into()), set_at_ts: 42 };
    s.set_label("TH001", &l).unwrap();
    assert_eq!(s.label("TH001").unwrap(), Some(l));
    let done = Label { label: LabelKind::Done, snooze_until: None, note: None, set_at_ts: 50 };
    s.set_label("TH001", &done).unwrap();
    assert_eq!(s.label("TH001").unwrap(), Some(done));
    assert!(s.clear_label("TH001").unwrap());
    assert!(!s.clear_label("TH001").unwrap());
    assert_eq!(s.label("TH001").unwrap(), None);
}

#[test]
fn thread_cache_upserts_and_lists_newest_first() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.threads_fetched_at().unwrap(), None);
    s.put_threads(&[cached("TH001", "ana", 10), cached("TH002", "bo", 20)]).unwrap();
    s.put_threads(&[cached("TH001", "ana_new", 30)]).unwrap();
    let all = s.cached_threads().unwrap();
    assert_eq!(all.iter().map(|t| t.thread_id.as_str()).collect::<Vec<_>>(), ["TH001", "TH002"]);
    assert_eq!(all[0].username, "ana_new");
    assert_eq!(s.threads_fetched_at().unwrap(), Some(30));
}

#[test]
fn threads_in_category_excludes_self_and_counts_default_general() {
    let s = Store::open_in_memory().unwrap();
    s.put_threads(&[cached("TH001", "a", 10), cached("TH002", "b", 20), cached("TH003", "c", 30)]).unwrap();
    s.set_category("TH003", "work").unwrap();
    assert_eq!(s.threads_in_category("general", "TH001", 3).unwrap(), ["TH002"]);
    assert_eq!(s.threads_in_category("work", "TH001", 3).unwrap(), ["TH003"]);
    assert!(s.threads_in_category("work", "TH003", 3).unwrap().is_empty());
}

#[test]
fn challenge_flag_roundtrips() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.challenge_at().unwrap(), None);
    s.set_challenge(NOW).unwrap();
    assert_eq!(s.challenge_at().unwrap(), Some(NOW));
    s.clear_challenge().unwrap();
    assert_eq!(s.challenge_at().unwrap(), None);
}

#[test]
fn profiles_and_notes_upsert() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.profile("general").unwrap(), None);
    s.save_profile("general", "casual", 1).unwrap();
    s.save_profile("general", "very casual", 2).unwrap();
    assert_eq!(s.profile("general").unwrap(), Some(VoiceEntry { body: "very casual".into(), updated_at: 2 }));
    assert_eq!(s.profile_dates().unwrap(), [("general".to_string(), 2)]);
    assert_eq!(s.notes("TH001").unwrap(), None);
    s.save_notes("TH001", "uses jaja", 3).unwrap();
    assert_eq!(s.notes("TH001").unwrap().unwrap().body, "uses jaja");
}

#[test]
fn own_counts_dedupe_exclude_sent_and_are_strictly_after() {
    let s = Store::open_in_memory().unwrap();
    let since = NOW - DAY;
    let at = |secs: i64| secs * 1_000_000;
    let items = vec![
        ("I001".to_string(), at(since)), // exactly at updated_at: not after
        ("I002".to_string(), at(since + 1)),
        ("I003".to_string(), at(since + 2)),
    ];
    s.record_own_activity("TH001", &items, NOW).unwrap();
    s.record_own_activity("TH001", &items, NOW).unwrap();
    assert_eq!(s.own_count_in_thread("TH001", since).unwrap(), 2);
    s.record_sent("I003", "TH001", at(since + 2)).unwrap();
    assert_eq!(s.own_count_in_thread("TH001", since).unwrap(), 1);
    assert_eq!(s.sent_ids("TH001").unwrap().into_iter().collect::<Vec<_>>(), ["I003"]);

    s.record_own_activity("TH002", &[("I010".into(), at(since + 5))], NOW).unwrap();
    s.set_category("TH002", "work").unwrap();
    assert_eq!(s.own_count_in_category("general", since).unwrap(), 1);
    assert_eq!(s.own_count_in_category("work", since).unwrap(), 1);
}

#[test]
fn old_own_activity_is_pruned() {
    let s = Store::open_in_memory().unwrap();
    let old = (NOW - OWN_ACTIVITY_KEEP_SECS - 1) * 1_000_000;
    s.record_own_activity("TH001", &[("I001".into(), old)], NOW).unwrap();
    assert_eq!(s.own_count_in_thread("TH001", 0).unwrap(), 0);
}
