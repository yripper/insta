# store

Last verified: 2026-11-07

## Purpose
SQLite: triage state, a metadata cache of 1:1 threads, voice profiles/notes, and message **ids** (own activity, sent items). No message text, ever.

## Structure
- `src/store/mod.rs`: `Store` (`open` 0600 + WAL at `src/store/mod.rs:26`, `open_in_memory`, a mutex-guarded connection that ignores poisoning).
- `src/store/schema.rs`: append-only `MIGRATIONS` keyed by `PRAGMA user_version` (`src/store/schema.rs:7`, `migrate` at `:52`).
- `src/store/state.rs`: `WatchLevel` (normal | muted), `ThreadState` (watch + category, default `general`), `LabelKind` (done | waiting | snoozed), `Label`.
- `src/store/cache.rs`: `CachedThread`, thread cache, the challenge flag in `kv`.
- `src/store/voice.rs`, `src/store/sent.rs`: see [voice.md](voice.md).

## Tables (migration 1)
`thread_state(thread_id PK, watch, category DEFAULT 'general')`, `labels(thread_id PK, label, snooze_until, note, set_at_ts)`, `thread_cache(thread_id PK, peer_id, username, full_name, fetched_at)`, `tone_profiles(category PK, body, updated_at)`, `chat_notes(thread_id PK, body, updated_at)`, `own_activity(item_id PK, thread_id, ts)`, `sent_items(item_id PK, thread_id, ts)`, `kv(key PK, value)`.

Units: `*_ts` / `ts` are Instagram item times in microseconds; `fetched_at`, `updated_at`, `snooze_until` are unix seconds.

## Key methods
- `thread_state` / `set_watch` / `set_category` (`src/store/state.rs:98`, `:113`, `:122`): upserts that don't clobber the other column.
- `label` / `set_label` / `clear_label` (`src/store/state.rs:131`, `:143`, `:152`).
- `put_threads` (`src/store/cache.rs:22`): upsert, never drops threads (only the newest inbox pages are read). `cached_threads` (`:37`) newest first. `threads_in_category` (`:60`): other cached threads whose current category matches, defaulting to `general`.
- `set_challenge` / `challenge_at` / `clear_challenge` (`src/store/cache.rs:72`, `:80`, `:86`): persisted so a restart doesn't hit Instagram again after a challenge.

## Dependencies
rusqlite (bundled), anyhow, serde, schemars.

## Gotchas & invariants
- Spec deviation: `own_activity` is keyed by `item_id` (one row per own message seen, no text) instead of `(thread_id PK, last_own_ts_seen)` plus a counter. Per-item rows dedupe repeated reads and let one table answer both "since the notes were saved" and "since the profile was saved" counts. Rows older than 22 days are pruned on write.
- Never edit a shipped migration; append a new one.

## Testing
`cargo test store`: migrations idempotent + 0600, state/label roundtrips, cache upsert, category queries, challenge flag, voice counts.
