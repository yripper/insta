//! Outgoing messages go through an in-memory queue, so insta never sends in
//! bursts: the first job goes at once, each later one waits a random 20–60 s
//! after the previous send, and at most 30 go out per rolling hour. Text lives
//! only in memory and is dropped as soon as the job finishes; a restart drops
//! unsent jobs. There is no automatic retry except one after a rate limit,
//! because a retried send that had actually gone through would duplicate it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tokio::sync::Notify;

use super::clock::{GapRng, Timer};
use super::errors::{Error, Result};
use super::voice::observe_own;
use crate::ig::InstagramApi;
use crate::ig::types::{IgError, Micros, SentMessage};
use crate::store::Store;

pub const MIN_GAP_MS: u64 = 20_000;
pub const MAX_GAP_MS: u64 = 60_000;
pub const HOURLY_CAP: usize = 30;
const HOUR_MS: i64 = 3_600_000;
/// Used when Instagram rate-limits without saying for how long.
pub const RATE_LIMIT_DEFAULT_SECS: u64 = 60;
/// Never pause longer than this for one rate limit.
pub const RATE_LIMIT_MAX_SECS: u64 = 600;
/// Finished jobs kept for send_status.
const KEEP_FINISHED: usize = 50;
/// Messages read after a send to look for ones the user hasn't seen.
const CHECK_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Sending,
    Sent,
    Failed,
}

#[derive(Debug, Clone)]
struct Job {
    id: u64,
    thread_id: String,
    chat: String,
    /// `None` once the job is finished: text never outlives its send.
    text: Option<String>,
    seen_until: Option<Micros>,
    /// Wait after the previous send, drawn at enqueue time so the ETA is exact.
    gap_ms: u64,
    status: JobStatus,
    sent_item_id: Option<String>,
    sent_at_ms: Option<i64>,
    error: Option<String>,
    mark_error: Option<String>,
    unseen: Option<usize>,
    marked_seen: bool,
    rate_limit_retried: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobView {
    pub job_id: u64,
    pub chat: String,
    pub chat_id: String,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_in_secs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The message WAS sent; only marking it seen failed. Never resend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_error: Option<String>,
    /// Incoming messages newer than seen_until: not marked seen. Show them to the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unseen: Option<usize>,
    pub marked_seen: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Enqueued {
    pub job_id: u64,
    /// 1 = next to go.
    pub position: usize,
    pub eta: String,
    pub eta_in_secs: i64,
    pub note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueStatus {
    pub jobs: Vec<JobView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halted: Option<String>,
    pub sends_last_hour: usize,
    pub hourly_cap: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do (empty, halted or no session).
    Idle,
    /// Slept until the next job's slot.
    Waited,
    /// One send attempt finished.
    Processed,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    next_id: u64,
    last_send_ms: Option<i64>,
    /// Times of recent sends, at most `HOURLY_CAP` of them.
    window: VecDeque<i64>,
    paused_until_ms: Option<i64>,
    halted: Option<String>,
    api: Option<Arc<dyn InstagramApi>>,
}

/// When a job with `gap_ms` may start.
fn earliest(now: i64, last_send: Option<i64>, gap_ms: u64, paused_until: Option<i64>, window: &VecDeque<i64>) -> i64 {
    let mut t = now;
    if let Some(last) = last_send {
        t = t.max(last + gap_ms as i64);
    }
    if let Some(p) = paused_until {
        t = t.max(p);
    }
    if window.len() >= HOURLY_CAP {
        t = t.max(window[window.len() - HOURLY_CAP] + HOUR_MS);
    }
    t
}

fn push_window(window: &mut VecDeque<i64>, at: i64) {
    window.push_back(at);
    while window.len() > HOURLY_CAP {
        window.pop_front();
    }
}

/// Start time of every queued job, simulating the queue in order.
fn etas(s: &State, now: i64) -> HashMap<u64, i64> {
    let mut last = s.last_send_ms;
    let mut window = s.window.clone();
    // A send in flight lands about now.
    if s.jobs.iter().any(|j| j.status == JobStatus::Sending) {
        last = Some(now);
        push_window(&mut window, now);
    }
    let mut out = HashMap::new();
    for j in s.jobs.iter().filter(|j| j.status == JobStatus::Queued) {
        let at = earliest(now, last, j.gap_ms, s.paused_until_ms, &window);
        out.insert(j.id, at);
        last = Some(at);
        push_window(&mut window, at);
    }
    out
}

fn rfc3339_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| ms.to_string())
}

fn view(j: &Job, eta: Option<i64>, now: i64) -> JobView {
    JobView {
        job_id: j.id,
        chat: j.chat.clone(),
        chat_id: j.thread_id.clone(),
        status: j.status,
        eta: eta.map(rfc3339_ms),
        eta_in_secs: eta.map(|e| (e - now).max(0).div_euclid(1000)),
        sent_item_id: j.sent_item_id.clone(),
        sent_at: j.sent_at_ms.map(rfc3339_ms),
        error: j.error.clone(),
        mark_error: j.mark_error.clone(),
        unseen: j.unseen,
        marked_seen: j.marked_seen,
    }
}

fn fail(j: &mut Job, reason: String) {
    j.status = JobStatus::Failed;
    j.error = Some(reason);
    j.text = None;
}

fn trim_finished(jobs: &mut VecDeque<Job>) {
    let finished = |j: &Job| matches!(j.status, JobStatus::Sent | JobStatus::Failed);
    let mut extra = jobs.iter().filter(|j| finished(j)).count().saturating_sub(KEEP_FINISHED);
    jobs.retain(|j| {
        if extra > 0 && finished(j) {
            extra -= 1;
            return false;
        }
        true
    });
}

struct MarkOutcome {
    marked: bool,
    unseen: Option<usize>,
}

pub struct SendQueue {
    state: Mutex<State>,
    notify: Notify,
    store: Arc<Store>,
    timer: Arc<dyn Timer>,
    rng: Arc<dyn GapRng>,
}

impl SendQueue {
    pub fn new(store: Arc<Store>, timer: Arc<dyn Timer>, rng: Arc<dyn GapRng>) -> SendQueue {
        SendQueue {
            state: Mutex::new(State { next_id: 1, ..State::default() }),
            notify: Notify::new(),
            store,
            timer,
            rng,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A (new) working session: sending may resume.
    pub fn attach(&self, api: Arc<dyn InstagramApi>) {
        let mut s = self.lock();
        s.api = Some(api);
        s.halted = None;
        drop(s);
        self.notify.notify_one();
    }

    pub fn enqueue(&self, thread_id: &str, chat: &str, text: &str, seen_until: Option<Micros>) -> Result<Enqueued> {
        if text.trim().is_empty() {
            return Err(Error::Invalid("text is empty".into()));
        }
        let now = self.timer.now_ms();
        let mut s = self.lock();
        if let Some(reason) = &s.halted {
            return Err(Error::Invalid(format!("sending is stopped: {reason}")));
        }
        let id = s.next_id;
        s.next_id += 1;
        s.jobs.push_back(Job {
            id,
            thread_id: thread_id.to_string(),
            chat: chat.to_string(),
            text: Some(text.to_string()),
            seen_until,
            gap_ms: self.rng.gap_ms(MIN_GAP_MS, MAX_GAP_MS),
            status: JobStatus::Queued,
            sent_item_id: None,
            sent_at_ms: None,
            error: None,
            mark_error: None,
            unseen: None,
            marked_seen: false,
            rate_limit_retried: false,
        });
        let eta = etas(&s, now).get(&id).copied().unwrap_or(now);
        let position = s.jobs.iter().filter(|j| j.status == JobStatus::Queued).count();
        drop(s);
        self.notify.notify_one();
        Ok(Enqueued {
            job_id: id,
            position,
            eta: rfc3339_ms(eta),
            eta_in_secs: (eta - now).max(0).div_euclid(1000),
            note: "Queued, not sent yet. Messages go out 20-60 s apart; check send_status.",
        })
    }

    /// One job (`Some`) or all recent jobs, oldest first.
    pub fn status(&self, job_id: Option<u64>) -> Result<QueueStatus> {
        let now = self.timer.now_ms();
        let s = self.lock();
        let etas = etas(&s, now);
        let jobs: Vec<JobView> = s
            .jobs
            .iter()
            .filter(|j| job_id.is_none_or(|id| j.id == id))
            .map(|j| view(j, etas.get(&j.id).copied(), now))
            .collect();
        if let Some(id) = job_id
            && jobs.is_empty()
        {
            return Err(Error::Invalid(format!("no job {id}: unsent jobs are dropped when insta restarts")));
        }
        Ok(QueueStatus {
            jobs,
            halted: s.halted.clone(),
            sends_last_hour: s.window.iter().filter(|t| now - **t < HOUR_MS).count(),
            hourly_cap: HOURLY_CAP,
        })
    }

    pub fn queued_count(&self) -> usize {
        self.lock().jobs.iter().filter(|j| j.status == JobStatus::Queued).count()
    }

    /// The worker: runs forever, one step at a time.
    pub async fn run(self: Arc<Self>) {
        loop {
            let notified = self.notify.notified();
            if self.step().await == Step::Idle {
                notified.await;
            }
        }
    }

    /// Waits for or sends the next job.
    pub async fn step(&self) -> Step {
        let now = self.timer.now_ms();
        let next = {
            let s = self.lock();
            let job = s.jobs.iter().find(|j| j.status == JobStatus::Queued);
            match (&s.halted, &s.api, job) {
                (None, Some(api), Some(j)) => {
                    Some((j.id, earliest(now, s.last_send_ms, j.gap_ms, s.paused_until_ms, &s.window), api.clone()))
                }
                _ => None,
            }
        };
        let Some((id, start, api)) = next else {
            return Step::Idle;
        };
        if start > now {
            self.timer.sleep_until(start).await;
            return Step::Waited;
        }
        let Some((thread_id, text, seen_until)) = self.begin(id) else {
            return Step::Idle;
        };
        match api.send_text(&thread_id, &text).await {
            Ok(sent) => self.on_sent(id, api.as_ref(), &thread_id, seen_until, sent).await,
            Err(e) => self.on_send_error(id, e),
        }
        Step::Processed
    }

    fn begin(&self, id: u64) -> Option<(String, String, Option<Micros>)> {
        let mut s = self.lock();
        let j = s.jobs.iter_mut().find(|j| j.id == id)?;
        j.status = JobStatus::Sending;
        Some((j.thread_id.clone(), j.text.clone().unwrap_or_default(), j.seen_until))
    }

    async fn on_sent(
        &self,
        id: u64,
        api: &dyn InstagramApi,
        thread_id: &str,
        seen_until: Option<Micros>,
        sent: SentMessage,
    ) {
        let now = self.timer.now_ms();
        if let Err(e) = self.store.record_sent(&sent.item_id, thread_id, sent.ts) {
            // Only voice learning depends on this; the message itself is out.
            tracing::warn!("recording a sent item id failed: {e}");
        }
        self.update(id, |s, j| {
            s.last_send_ms = Some(now);
            push_window(&mut s.window, now);
            j.status = JobStatus::Sent;
            j.sent_item_id = Some(sent.item_id.clone());
            j.sent_at_ms = Some(now);
            j.text = None;
        });
        // From here on the message is out: a failure must not look like a
        // failed send, or it gets approved and sent again.
        match self.mark_after_send(api, thread_id, seen_until, &sent).await {
            Ok(o) => self.update(id, |_, j| (j.marked_seen, j.unseen) = (o.marked, o.unseen)),
            Err(e) => {
                self.update(id, |_, j| j.mark_error = Some(e.to_string()));
                if e.is_fatal() {
                    self.halt(&e);
                }
            }
        }
        tracing::info!(job = id, "message sent");
    }

    /// Marks the thread seen up to the sent message, unless something arrived
    /// after `seen_until` that the user hasn't seen. Without `seen_until`
    /// nothing is marked: insta can't tell what the user saw.
    async fn mark_after_send(
        &self,
        api: &dyn InstagramApi,
        thread_id: &str,
        seen_until: Option<Micros>,
        sent: &SentMessage,
    ) -> std::result::Result<MarkOutcome, IgError> {
        let Some(seen) = seen_until else {
            return Ok(MarkOutcome { marked: false, unseen: None });
        };
        let page = api.thread(thread_id, None, CHECK_LIMIT).await?;
        if let Err(e) = observe_own(&self.store, thread_id, &page.messages, self.timer.now_secs()) {
            tracing::warn!(kind = e.kind(), "recording own activity failed");
        }
        let unseen = page.messages.iter().filter(|m| !m.from_me && m.ts > seen && m.id != sent.item_id).count();
        if unseen > 0 {
            return Ok(MarkOutcome { marked: false, unseen: Some(unseen) });
        }
        api.mark_seen(thread_id, &sent.item_id).await?;
        Ok(MarkOutcome { marked: true, unseen: Some(0) })
    }

    fn on_send_error(&self, id: u64, e: IgError) {
        tracing::warn!(job = id, error = %e, "send failed");
        if e.is_fatal() {
            self.update(id, |_, j| fail(j, e.to_string()));
            self.halt(&e);
            return;
        }
        let now = self.timer.now_ms();
        self.update(id, |s, j| match &e {
            IgError::RateLimited { retry_after_secs } => {
                let pause = retry_after_secs.unwrap_or(RATE_LIMIT_DEFAULT_SECS).min(RATE_LIMIT_MAX_SECS);
                s.paused_until_ms = Some(now + pause as i64 * 1000);
                if j.rate_limit_retried {
                    fail(j, format!("{e} (twice; not retried again)"));
                } else {
                    j.rate_limit_retried = true;
                    j.status = JobStatus::Queued;
                }
            }
            other => fail(j, other.to_string()),
        });
    }

    /// The session is dead or challenged: fail everything still queued and
    /// refuse new jobs until a new session attaches.
    fn halt(&self, e: &IgError) {
        let reason = e.to_string();
        let mut s = self.lock();
        let mut dropped = 0;
        for j in s.jobs.iter_mut().filter(|j| j.status == JobStatus::Queued) {
            fail(j, reason.clone());
            dropped += 1;
        }
        s.halted = Some(reason);
        drop(s);
        if *e == IgError::Challenge
            && let Err(err) = self.store.set_challenge(self.timer.now_secs())
        {
            tracing::warn!("saving the challenge flag failed: {err}");
        }
        tracing::warn!(dropped, "send queue halted");
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut State, &mut Job)) {
        let mut s = self.lock();
        let Some(pos) = s.jobs.iter().position(|j| j.id == id) else { return };
        let mut job = s.jobs[pos].clone();
        f(&mut s, &mut job);
        s.jobs[pos] = job;
        trim_finished(&mut s.jobs);
    }
}

#[cfg(test)]
#[path = "send_queue_tests.rs"]
mod tests;
