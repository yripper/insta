use super::*;
use crate::domain::clock::fake::{FakeTimer, SeqGaps};
use crate::domain::send_queue::Step;
use crate::ig::fake::{FakeData, FakeInstagram, ME_ID, dm, group, me, text, user, voice};
use crate::ig::types::IgError;
use crate::whisper::WhisperError;
use crate::whisper::fake::FakeWhisper;

const NOW: i64 = 1_800_000_000;

fn us(secs: i64) -> i64 {
    secs * 1_000_000
}

struct Rig {
    api: Arc<FakeInstagram>,
    store: Arc<Store>,
    queue: Arc<SendQueue>,
    whisper: Arc<FakeWhisper>,
    a: Actions,
}

fn data() -> FakeData {
    let mut d = FakeData {
        threads: vec![
            dm("TH001", user("U001", "ana", "Ana Example"), us(NOW - 10), false),
            group("TH900", us(NOW - 20)),
            dm("TH002", user("U002", "bo", "Bo Example"), us(NOW - 30), true),
            dm("TH003", user("U003", "cy", ""), us(NOW - 40), false),
        ],
        ..Default::default()
    };
    d.messages.insert(
        "TH001".into(),
        vec![
            text("I012", us(NOW - 10), "U001", "fake question?"),
            text("I011", us(NOW - 11), "U001", "fake hello"),
            text("I010", us(NOW - 12), ME_ID, "fake mine"),
            voice("I009", us(NOW - 13), "U001", "https://cdn.example.com/v1", 12_000),
        ],
    );
    d.messages.insert("TH002".into(), vec![text("I020", us(NOW - 30), ME_ID, "fake reply")]);
    d.messages.insert("TH003".into(), vec![text("I030", us(NOW - 40), "U003", "fake ping")]);
    d.media.insert("https://cdn.example.com/v1".into(), b"\0\0\0\x18ftypM4A ".to_vec());
    d
}

fn rig_with(d: FakeData, whisper: FakeWhisper) -> Rig {
    let api = Arc::new(FakeInstagram::new(d));
    let store = Arc::new(Store::open_in_memory().unwrap());
    let timer = Arc::new(FakeTimer::at(NOW * 1000));
    let queue = Arc::new(SendQueue::new(store.clone(), timer.clone(), Arc::new(SeqGaps::new(&[20_000]))));
    queue.attach(api.clone());
    let whisper = Arc::new(whisper);
    let a = Actions::new(api.clone(), store.clone(), me(), queue.clone(), whisper.clone(), timer);
    Rig { api, store, queue, whisper, a }
}

fn rig() -> Rig {
    rig_with(data(), FakeWhisper::new(Ok("fake transcript".into())))
}

#[tokio::test]
async fn pending_lists_one_to_one_chats_where_they_wrote_last() {
    let r = rig();
    let p = r.a.pending().await.unwrap();
    let ids: Vec<&str> = p.chats.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["TH001", "TH003"]);
    let first = &p.chats[0];
    assert_eq!(first.name, "@ana (Ana Example)");
    let texts: Vec<&str> = first.messages.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["fake hello", "fake question?"], "their messages since mine, oldest first");
    assert!(!first.more_before);
    assert_eq!(r.api.count("mark_seen"), 0, "pending never marks seen");
}

#[tokio::test]
async fn pending_respects_labels_and_clears_ended_ones() {
    let r = rig();
    r.a.set_label("ana", LabelKind::Done, None, None).await.unwrap();
    let ids = |p: PendingReport| p.chats.into_iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(ids(r.a.pending().await.unwrap()), ["TH003"]);
    // A new incoming message clears done.
    r.api.data.lock().unwrap().threads[0].last_activity_ts = us(NOW - 5);
    assert_eq!(ids(r.a.pending().await.unwrap()), ["TH001", "TH003"]);
    assert_eq!(r.store.label("TH001").unwrap(), None, "ended label deleted");
}

#[tokio::test]
async fn pending_survives_a_broken_chat_but_not_a_dead_session() {
    let r = rig();
    r.api.data.lock().unwrap().fail_once.push_back(("thread TH001".into(), IgError::Http("boom".into())));
    let p = r.a.pending().await.unwrap();
    assert!(p.chats[0].error.is_some());
    assert_eq!(p.chats[1].messages.len(), 1);
    r.api.data.lock().unwrap().fail_once.push_back(("thread TH001".into(), IgError::NeedsLogin));
    assert!(r.a.pending().await.unwrap_err().is_fatal());
}

#[tokio::test]
async fn read_history_is_oldest_first_with_newest_ts_and_records_own_activity() {
    let r = rig();
    let h = r.a.read_history("@ana", 30, None).await.unwrap();
    assert_eq!(h.chat_id, "TH001");
    assert_eq!(h.messages.first().unwrap().id, "I009");
    assert_eq!(h.newest_ts, Some(us(NOW - 10)));
    assert_eq!(h.messages[0].text, "[voice 0:12] — call transcribe_voice");
    assert_eq!(r.store.own_count_in_thread("TH001", NOW - 100).unwrap(), 1);
    assert_eq!(r.api.count("mark_seen"), 0);
}

#[tokio::test]
async fn groups_cannot_be_resolved() {
    let r = rig();
    assert!(matches!(r.a.read_history("TH900", 10, None).await, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn mark_read_marks_newest_or_given_item() {
    let r = rig();
    assert_eq!(r.a.mark_read("ana", None).await.unwrap().marked_seen_until, "I012");
    r.a.mark_read("ana", Some("I011")).await.unwrap();
    assert_eq!(r.api.calls().iter().filter(|c| c.starts_with("mark_seen")).count(), 2);
    assert_eq!(r.api.count("mark_seen TH001 I011"), 1);
}

#[tokio::test]
async fn send_enqueues_and_the_worker_sends() {
    let r = rig();
    let e = r.a.send("ana", "fake answer", Some(us(NOW - 10))).await.unwrap();
    assert_eq!(e.position, 1);
    assert_eq!(r.api.count("send_text"), 0);
    while r.queue.step().await != Step::Idle {}
    let st = r.a.send_status(Some(e.job_id)).unwrap();
    assert!(st.jobs[0].marked_seen);
    assert_eq!(r.api.count("send_text TH001"), 1);
}

#[tokio::test]
async fn send_to_ambiguous_name_is_refused() {
    let mut d = data();
    d.threads.push(dm("TH004", user("U004", "anabel", ""), us(NOW - 50), false));
    let r = rig_with(d, FakeWhisper::new(Ok(String::new())));
    assert!(matches!(r.a.send("an", "x", None).await, Err(Error::Ambiguous { .. })));
    assert_eq!(r.queue.queued_count(), 0);
}

#[tokio::test]
async fn set_label_validates_inputs() {
    let r = rig();
    assert!(r.a.set_label("ana", LabelKind::Snoozed, None, None).await.is_err());
    assert!(r.a.set_label("ana", LabelKind::Snoozed, None, Some(NOW - 1)).await.is_err());
    assert!(r.a.set_label("ana", LabelKind::Done, None, Some(NOW + 60)).await.is_err());
    assert!(r.a.set_label("ana", LabelKind::Waiting, Some("x".repeat(501)), None).await.is_err());
    let ok = r.a.set_label("ana", LabelKind::Snoozed, Some("after lunch".into()), Some(NOW + 60)).await.unwrap();
    assert_eq!(ok.label.unwrap().set_at_ts, us(NOW - 10), "set at the newest message");
    r.a.clear_label("ana").await.unwrap();
    assert_eq!(r.store.label("TH001").unwrap(), None);
}

#[tokio::test]
async fn watch_and_category_and_listing() {
    let r = rig();
    r.a.set_watch("cy", WatchLevel::Muted).await.unwrap();
    let names: Vec<String> = r.a.list_chats(None, 30).await.unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(names, ["TH001", "TH002"], "groups and muted hidden");
    let found = r.a.list_chats(Some("@cy"), 30).await.unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].muted);
    assert!(r.a.set_category("ana", "Friends").await.is_err());
    r.a.set_category("ana", "friends").await.unwrap();
    assert_eq!(r.store.thread_state("TH001").unwrap().category, "friends");
    assert_eq!(r.a.list_chats(None, 1).await.unwrap().len(), 1);
}

#[tokio::test]
async fn get_voice_returns_samples_when_learning_is_needed() {
    let r = rig();
    let v = r.a.get_voice("ana").await.unwrap();
    assert!(v.context.needs_learning && v.context.notes_stale);
    let chat: Vec<&str> = v.my_messages_in_chat.as_ref().unwrap().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(chat, ["I010"]);
    let cat: Vec<&str> = v.my_messages_in_category.as_ref().unwrap().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(cat, ["I020"], "other general chats, live");
    let json = serde_json::to_value(&v).unwrap();
    assert!(json.get("needsLearning").is_some() && json.get("my_messages_in_chat").is_some());
}

#[tokio::test]
async fn get_voice_fetches_nothing_when_profile_and_notes_are_fresh() {
    let r = rig();
    r.a.save_profile("general", "casual").unwrap();
    r.a.save_chat_notes("ana", "jaja").await.unwrap();
    let before = r.api.count("thread");
    let v = r.a.get_voice("ana").await.unwrap();
    assert!(!v.context.needs_learning && !v.context.notes_stale);
    assert!(v.my_messages_in_chat.is_none() && v.my_messages_in_category.is_none());
    assert_eq!(r.api.count("thread"), before);
}

#[tokio::test]
async fn get_voice_with_profile_but_no_notes_samples_chat_only() {
    let r = rig();
    r.a.save_profile("general", "casual").unwrap();
    let v = r.a.get_voice("ana").await.unwrap();
    assert!(!v.context.needs_learning && v.context.notes_stale);
    assert!(v.my_messages_in_chat.is_some() && v.my_messages_in_category.is_none());
}

#[tokio::test]
async fn voice_samples_exclude_messages_insta_sent() {
    let r = rig();
    r.store.record_sent("I010", "TH001", us(NOW - 12)).unwrap();
    assert!(r.a.get_my_messages(Some("ana"), None, 60).await.unwrap().is_empty());
}

#[tokio::test]
async fn get_my_messages_needs_exactly_one_source_and_save_validates() {
    let r = rig();
    assert!(r.a.get_my_messages(None, None, 10).await.is_err());
    assert!(r.a.get_my_messages(Some("ana"), Some("general"), 10).await.is_err());
    assert!(r.a.get_my_messages(None, Some("Bad Slug"), 10).await.is_err());
    r.a.read_history("ana", 5, None).await.unwrap(); // fills the cache
    assert_eq!(r.a.get_my_messages(None, Some("general"), 10).await.unwrap().len(), 2);
    assert!(r.a.save_profile("general", "").is_err());
    assert!(r.a.save_profile("general", &"x".repeat(4_001)).is_err());
    assert!(r.a.save_chat_notes("ana", &"x".repeat(2_001)).await.is_err());
    r.a.save_profile("general", "x").unwrap();
    assert!(r.a.list_stale_profiles().unwrap().is_empty());
}

#[tokio::test]
async fn transcribe_voice_fetches_media_and_returns_text() {
    let r = rig();
    let t = r.a.transcribe_voice("ana", "I009").await.unwrap();
    assert_eq!(t.text, "fake transcript");
    assert_eq!(t.duration.as_deref(), Some("0:12"));
    assert_eq!(r.whisper.calls.lock().unwrap()[0].1, "voice.m4a");
    assert_eq!(r.api.count("fetch_media"), 1);
}

#[tokio::test]
async fn transcribe_voice_refuses_non_voice_and_missing_key() {
    let r = rig();
    assert!(matches!(r.a.transcribe_voice("ana", "I012").await, Err(Error::Invalid(_))));
    assert!(matches!(r.a.transcribe_voice("ana", "I999").await, Err(Error::Invalid(_))));
    let nokey = rig_with(data(), FakeWhisper::new(Err(WhisperError::MissingKey)));
    let e = nokey.a.transcribe_voice("ana", "I009").await.unwrap_err();
    assert!(e.to_string().contains("insta set-openai-key"));
    assert_eq!(nokey.api.count("fetch_media"), 0, "no download without a key");
}

#[test]
fn since_my_last_takes_their_trailing_run() {
    let page = vec![text("I3", 3, "U001", "c"), text("I2", 2, "U001", "b"), text("I1", 1, ME_ID, "a")];
    let (msgs, more) = since_my_last(&page);
    assert_eq!(msgs.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["I2", "I3"]);
    assert!(!more);
    let (all, more) = since_my_last(&page[..2]);
    assert_eq!(all.len(), 2);
    assert!(more);
    assert_eq!(since_my_last(&[]), (vec![], false));
}
