# send_queue

Last verified: 2026-11-07

## Purpose
Every outgoing message goes through this in-memory queue, so insta never bursts: the first job goes at once, each later one waits a uniformly random 20–60 s after the previous send, and at most 30 go out per rolling hour. Text lives only in memory and is dropped when the job finishes.

## Structure
- `src/domain/send_queue.rs`: `SendQueue`, `JobStatus`, `JobView`, `Enqueued`, `QueueStatus`, `Step`, and the pure helpers `earliest`, `etas`.
- Owned by `AppState` (survives re-logins; `attach` swaps the client). The worker (`run`) is spawned by `insta start`.

## Public surface
- Constants (`src/domain/send_queue.rs:21`-`32`): `MIN_GAP_MS = 20_000`, `MAX_GAP_MS = 60_000`, `HOURLY_CAP = 30`, `RATE_LIMIT_DEFAULT_SECS = 60`, `RATE_LIMIT_MAX_SECS = 600`, `KEEP_FINISHED = 50`, `CHECK_LIMIT = 20`.
- `new(store, timer, rng)` (`:225`), `attach(api)` (`:240`, clears a halt), `enqueue` (`:248`) → `{job_id, position, eta, eta_in_secs, note}` immediately, `status(job_id?)` (`:289`), `queued_count` (`:312`), `run` (`:317`), `step` (`:327`).

## Key methods
- Scheduling: each job's gap is drawn **at enqueue time**, so the ETA is exact. `earliest` (`:130`) = max(now, last_send + gap, rate-limit pause, the 30th-most-recent send + 1 h). `etas` (`:152`) simulates the queue in order (a send in flight counts as "now").
- `step` (`:327`): sleeps until the first queued job's slot (returns `Waited`, then re-plans), else sends it.
- `on_sent` (`:363`): records the item id in `sent_items`, marks the job sent and drops its text. Then `mark_after_send` (`:401`): with `seen_until`, reads the newest 20 messages; if no incoming message is newer than `seen_until`, `mark_seen` at the sent item, else reports `unseen` (count) and marks nothing. Without `seen_until` nothing is read or marked. Any failure here only sets `mark_error`: **a job with `sent_item_id` is sent**. A fatal mark error still halts the rest.
- `on_send_error` (`:423`): NeedsLogin/Challenge → `halt` (`:448`): fail this and every queued job with the reason, refuse new jobs until `attach`; Challenge also sets the persisted challenge flag. RateLimited → pause `retry_after` (default 60 s, cap 600 s) and retry that job once; a second rate limit fails it and the queue continues after the pause. Anything else fails only that job, no retry.

## Dependencies
clock (`Timer`, `GapRng`), store, ig (trait), voice (`observe_own`), tokio `Notify`.

## Gotchas & invariants
- No automatic retry except the single post-rate-limit one: a timed-out send may have gone out, and a retry would duplicate it.
- Restart drops unsent jobs; `insta start` logs only their count.
- `etas` assumes no future rate limits; an ETA can slip after one.
- Clock and RNG are injected, so tests run on a virtual clock (`FakeTimer`) with fixed gaps (`SeqGaps`).

## Testing
`cargo test send_queue`: first send immediate, exact gaps and ETAs, gap counted from the previous send, hourly cap (31st at +1 h), Challenge / NeedsLogin halts, rate-limit retry once and cap, other errors fail one job, mark_seen vs unseen, no seen_until, mark_error keeps a send, fatal mark error halts, history bound and text dropped.
