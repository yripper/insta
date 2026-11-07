use super::*;
use crate::domain::clock::fake::{FakeTimer, SeqGaps};
use crate::ig::fake::{FakeData, FakeInstagram, ME_ID, text};

const T0: i64 = 1_800_000_000_000;

struct Rig {
    api: Arc<FakeInstagram>,
    store: Arc<Store>,
    timer: Arc<FakeTimer>,
    q: SendQueue,
}

fn rig(gaps: &[u64]) -> Rig {
    let mut d = FakeData::default();
    d.messages.insert("TH001".into(), vec![text("I010", 500, "U001", "fake question"), text("I009", 400, ME_ID, "x")]);
    let api = Arc::new(FakeInstagram::new(d));
    let store = Arc::new(Store::open_in_memory().unwrap());
    let timer = Arc::new(FakeTimer::at(T0));
    let q = SendQueue::new(store.clone(), timer.clone(), Arc::new(SeqGaps::new(gaps)));
    q.attach(api.clone());
    Rig { api, store, timer, q }
}

async fn drain(q: &SendQueue) {
    for _ in 0..1_000 {
        if q.step().await == Step::Idle {
            return;
        }
    }
    panic!("queue never went idle");
}

fn sent_times(q: &SendQueue) -> Vec<i64> {
    q.lock().jobs.iter().filter_map(|j| j.sent_at_ms).map(|t| t - T0).collect()
}

fn job(q: &SendQueue, id: u64) -> JobView {
    q.status(Some(id)).unwrap().jobs.remove(0)
}

#[tokio::test]
async fn first_send_is_immediate_then_random_gaps_and_etas_match() {
    let r = rig(&[20_000, 30_000, 45_000]);
    let a = r.q.enqueue("TH001", "@ana", "one", None).unwrap();
    let b = r.q.enqueue("TH001", "@ana", "two", None).unwrap();
    let c = r.q.enqueue("TH001", "@ana", "three", None).unwrap();
    assert_eq!((a.position, a.eta_in_secs), (1, 0));
    assert_eq!((b.position, b.eta_in_secs), (2, 30));
    assert_eq!((c.position, c.eta_in_secs), (3, 75));
    assert_eq!(r.api.count("send_text"), 0, "enqueue returns before sending");
    drain(&r.q).await;
    assert_eq!(sent_times(&r.q), [0, 30_000, 75_000]);
    assert_eq!(r.api.count("send_text"), 3);
}

#[tokio::test]
async fn gap_counts_from_the_previous_send_not_from_enqueue() {
    let r = rig(&[20_000, 40_000]);
    r.q.enqueue("TH001", "@ana", "one", None).unwrap();
    drain(&r.q).await;
    r.timer.advance(100_000);
    let e = r.q.enqueue("TH001", "@ana", "two", None).unwrap();
    assert_eq!(e.eta_in_secs, 0, "gap already elapsed");
    drain(&r.q).await;
    assert_eq!(sent_times(&r.q), [0, 100_000]);
}

#[tokio::test]
async fn hourly_cap_holds_the_31st_send_for_an_hour() {
    let r = rig(&[20_000]);
    let mut last = None;
    for i in 0..32 {
        last = Some(r.q.enqueue("TH001", "@ana", &format!("m{i}"), None).unwrap());
    }
    assert_eq!(last.unwrap().eta_in_secs, 3_600 + 20, "eta reflects the cap");
    drain(&r.q).await;
    let times = sent_times(&r.q);
    assert_eq!(times[29], 29 * 20_000);
    assert_eq!(times[30], 3_600_000, "31st waits until the 1st is an hour old");
    assert_eq!(times[31], 3_620_000);
    for w in times.windows(2) {
        assert!(w[1] - w[0] >= MIN_GAP_MS as i64);
    }
    assert!(r.q.status(None).unwrap().sends_last_hour <= HOURLY_CAP);
}

#[tokio::test]
async fn challenge_halts_fails_the_rest_and_flags_the_store() {
    let r = rig(&[20_000]);
    r.api.data.lock().unwrap().fail_once.push_back(("send_text".into(), IgError::Challenge));
    for t in ["one", "two", "three"] {
        r.q.enqueue("TH001", "@ana", t, None).unwrap();
    }
    drain(&r.q).await;
    assert_eq!(r.api.count("send_text"), 1, "nothing else is tried");
    let st = r.q.status(None).unwrap();
    assert!(st.jobs.iter().all(|j| j.status == JobStatus::Failed));
    assert!(st.halted.as_deref().unwrap().contains("insta login"));
    assert!(r.store.challenge_at().unwrap().is_some());
    assert!(r.q.enqueue("TH001", "@ana", "four", None).is_err(), "no new jobs while halted");
    assert!(r.q.lock().jobs.iter().all(|j| j.text.is_none()), "text dropped");

    r.q.attach(r.api.clone());
    assert!(r.q.enqueue("TH001", "@ana", "five", None).is_ok(), "new session resumes");
}

#[tokio::test]
async fn needs_login_halts_without_challenge_flag() {
    let r = rig(&[20_000]);
    r.q.enqueue("TH001", "@ana", "one", None).unwrap();
    drain(&r.q).await;
    r.api.data.lock().unwrap().fail_once.push_back(("send_text".into(), IgError::NeedsLogin));
    r.q.enqueue("TH001", "@ana", "two", None).unwrap();
    r.q.enqueue("TH001", "@ana", "three", None).unwrap();
    drain(&r.q).await;
    assert_eq!(job(&r.q, 1).status, JobStatus::Sent);
    assert_eq!(job(&r.q, 3).status, JobStatus::Failed);
    assert!(r.q.status(None).unwrap().halted.is_some());
    assert_eq!(r.store.challenge_at().unwrap(), None);
}

#[tokio::test]
async fn rate_limit_pauses_then_retries_once() {
    let r = rig(&[20_000]);
    r.api
        .data
        .lock()
        .unwrap()
        .fail_once
        .push_back(("send_text".into(), IgError::RateLimited { retry_after_secs: Some(120) }));
    r.q.enqueue("TH001", "@ana", "one", None).unwrap();
    drain(&r.q).await;
    assert_eq!(job(&r.q, 1).status, JobStatus::Sent);
    assert_eq!(sent_times(&r.q), [120_000]);
    assert_eq!(r.api.count("send_text"), 2);
}

#[tokio::test]
async fn second_rate_limit_fails_the_job_and_the_queue_goes_on() {
    let r = rig(&[20_000]);
    {
        let mut d = r.api.data.lock().unwrap();
        d.fail_once.push_back(("send_text".into(), IgError::RateLimited { retry_after_secs: None }));
        d.fail_once.push_back(("send_text".into(), IgError::RateLimited { retry_after_secs: Some(99_999) }));
    }
    r.q.enqueue("TH001", "@ana", "one", None).unwrap();
    r.q.enqueue("TH001", "@ana", "two", None).unwrap();
    drain(&r.q).await;
    assert_eq!(job(&r.q, 1).status, JobStatus::Failed);
    assert_eq!(job(&r.q, 2).status, JobStatus::Sent);
    // 60 s default pause, then a pause capped at 600 s.
    assert_eq!(sent_times(&r.q), [(RATE_LIMIT_DEFAULT_SECS + RATE_LIMIT_MAX_SECS) as i64 * 1000]);
}

#[tokio::test]
async fn other_errors_fail_only_that_job() {
    let r = rig(&[20_000]);
    r.api.data.lock().unwrap().fail_once.push_back(("send_text".into(), IgError::Http("boom".into())));
    r.q.enqueue("TH001", "@ana", "one", None).unwrap();
    r.q.enqueue("TH001", "@ana", "two", None).unwrap();
    drain(&r.q).await;
    assert_eq!(job(&r.q, 1).error.as_deref(), Some("HTTP error talking to Instagram: boom"));
    assert_eq!(job(&r.q, 2).status, JobStatus::Sent);
    assert_eq!(r.api.count("send_text"), 2, "no retry");
}

#[tokio::test]
async fn marks_seen_at_the_sent_item_when_nothing_new_arrived() {
    let r = rig(&[20_000]);
    r.q.enqueue("TH001", "@ana", "reply", Some(500)).unwrap();
    drain(&r.q).await;
    let j = job(&r.q, 1);
    let item = j.sent_item_id.clone().unwrap();
    assert!(j.marked_seen);
    assert_eq!(j.unseen, Some(0));
    assert_eq!(r.api.count(&format!("mark_seen TH001 {item}")), 1);
    assert!(r.store.sent_ids("TH001").unwrap().contains(&item), "sent id recorded");
}

#[tokio::test]
async fn unseen_incoming_message_blocks_mark_seen() {
    let r = rig(&[20_000]);
    r.q.enqueue("TH001", "@ana", "reply", Some(400)).unwrap();
    drain(&r.q).await;
    let j = job(&r.q, 1);
    assert_eq!(j.status, JobStatus::Sent);
    assert!(!j.marked_seen);
    assert_eq!(j.unseen, Some(1));
    assert_eq!(r.api.count("mark_seen"), 0);
}

#[tokio::test]
async fn without_seen_until_nothing_is_marked_or_read() {
    let r = rig(&[20_000]);
    r.q.enqueue("TH001", "@ana", "reply", None).unwrap();
    drain(&r.q).await;
    assert_eq!(r.api.count("mark_seen"), 0);
    assert_eq!(r.api.count("thread"), 0);
    assert_eq!(job(&r.q, 1).unseen, None);
}

#[tokio::test]
async fn mark_error_does_not_fail_a_sent_message() {
    let r = rig(&[20_000]);
    r.api.data.lock().unwrap().fail_once.push_back(("mark_seen".into(), IgError::Http("flaky".into())));
    r.q.enqueue("TH001", "@ana", "reply", Some(500)).unwrap();
    drain(&r.q).await;
    let j = job(&r.q, 1);
    assert_eq!(j.status, JobStatus::Sent);
    assert!(j.sent_item_id.is_some());
    assert!(j.mark_error.as_deref().unwrap().contains("flaky"));
    assert!(r.q.status(None).unwrap().halted.is_none());
}

#[tokio::test]
async fn fatal_mark_error_keeps_the_send_but_halts_the_rest() {
    let r = rig(&[20_000]);
    r.api.data.lock().unwrap().fail_once.push_back(("thread".into(), IgError::NeedsLogin));
    r.q.enqueue("TH001", "@ana", "one", Some(500)).unwrap();
    r.q.enqueue("TH001", "@ana", "two", None).unwrap();
    drain(&r.q).await;
    assert_eq!(job(&r.q, 1).status, JobStatus::Sent);
    assert_eq!(job(&r.q, 2).status, JobStatus::Failed);
    assert_eq!(r.api.count("send_text"), 1);
}

#[tokio::test]
async fn idle_without_session_and_rejects_empty_text() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let q = SendQueue::new(store, Arc::new(FakeTimer::at(T0)), Arc::new(SeqGaps::new(&[20_000])));
    assert!(matches!(q.enqueue("TH001", "@ana", "  ", None), Err(Error::Invalid(_))));
    q.enqueue("TH001", "@ana", "hi", None).unwrap();
    assert_eq!(q.step().await, Step::Idle, "no session attached yet");
    assert_eq!(q.queued_count(), 1);
    assert!(q.status(Some(99)).is_err());
}

#[tokio::test]
async fn finished_history_is_bounded_and_text_is_dropped() {
    let r = rig(&[20_000]);
    for i in 0..(KEEP_FINISHED + 5) {
        r.q.enqueue("TH001", "@ana", &format!("m{i}"), None).unwrap();
        drain(&r.q).await;
    }
    let s = r.q.lock();
    assert_eq!(s.jobs.len(), KEEP_FINISHED);
    assert!(s.jobs.iter().all(|j| j.text.is_none()));
}

#[test]
fn earliest_combines_gap_pause_and_cap() {
    let w: VecDeque<i64> = (0..30).map(|i| i * 1_000).collect();
    assert_eq!(earliest(100, None, 20_000, None, &VecDeque::new()), 100);
    assert_eq!(earliest(100, Some(50), 20_000, None, &VecDeque::new()), 20_050);
    assert_eq!(earliest(100, None, 20_000, Some(9_000), &VecDeque::new()), 9_000);
    assert_eq!(earliest(100, Some(29_000), 20_000, None, &w), HOUR_MS);
}
