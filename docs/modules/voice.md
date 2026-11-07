# voice

Last verified: 2026-11-07

## Purpose
Writing in the user's voice: a tone profile per chat category and notes per chat, both written by Claude, plus the staleness logic that says when to refresh them. Port of wasap's design (`src/domain/voice.ts`, `src/mcp/tools/voice.ts`) with one big change: **own messages are fetched live from Instagram and never stored**; staleness is counted from message ids and times only.

## Structure
- `src/domain/voice.rs`: constants, `is_stale`, `VoiceContext`, `voice_context`, `stale_profiles`, `valid_category`, `observe_own`, `own_texts`, `own_messages`, `own_messages_in_category`.
- `src/store/voice.rs`: profiles, notes, `own_activity` and its counts.
- `src/store/sent.rs`: `sent_items` (ids of messages insta sent).

## Public surface
- `STALE_OWN_MESSAGES = 30`, `STALE_AGE_SECS = 21 days` (`src/domain/voice.rs:17`). `is_stale` (`:26`): `own_since >= 30 || now - updated_at >= 21d`.
- `voice_context` (`src/domain/voice.rs:47`) → `{category, profile, notes, profileStale, notesStale, needsLearning}` (camelCase, `:32`).
  - `notesStale` is **true when the chat has no notes** (wasap returned false there, so new chats never got notes).
  - `profileStale` is false with no profile; `needsLearning` = no profile or stale profile.
- Tool behavior (`Actions::get_voice`, `src/domain/actions.rs:355`): with `needsLearning || notesStale` it adds `my_messages_in_chat` (up to `CHAT_SAMPLE = 60`); with `needsLearning` also `my_messages_in_category` (up to `CATEGORY_SAMPLE = 40` from at most `CATEGORY_THREADS = 3` other cached threads of the category, `:19`-`21`).
- `stale_profiles` (`src/domain/voice.rs:70`), `valid_category` (`:81`, `^[a-z0-9_-]{1,32}$`).

## Key methods
- `observe_own` (`src/domain/voice.rs:96`): records `(item_id, ts)` of own messages on every thread page insta reads (read_history, pending, voice samples, the post-send check). Insert-or-ignore, so re-reading counts once.
- `own_count_in_thread` / `own_count_in_category` (`src/store/voice.rs:91`, `:96`): own messages strictly after `updated_at`, **excluding `sent_items`** (what insta sent is Claude's writing, not the user's). Category membership is the thread's *current* category (default `general`).
- `own_texts` (`src/domain/voice.rs:105`): own Text/Link messages with non-blank text, excluding `sent_items`.
- `own_messages` (`src/domain/voice.rs:114`): pages the live thread (50 per page, at most 3 pages) until it has `limit`.
- `own_messages_in_category` (`src/domain/voice.rs:145`): `Store::threads_in_category` (most recently seen first), stops once it has `limit`. Each page is a paced Instagram call.

## Dependencies
store, ig (trait), render (`rfc3339`).

## Gotchas & invariants
- Profiles and notes are personal data: only in `~/.insta/insta.db`, never logged, committed or in fixtures. `profile_dates` returns no bodies.
- Staleness only sees own messages insta has *read*. Messages the user typed in threads insta hasn't opened since don't count until they are read; age (21 days) is the backstop.
- `own_activity` rows older than 22 days are pruned (`OWN_ACTIVITY_KEEP_SECS`, `src/store/voice.rs:14`): anything saved before the 21-day window is stale by age anyway.
- Differs from wasap: wasap counts messages it sent toward staleness; insta doesn't (spec: "own non-insta messages").

## Testing
`cargo test voice`: staleness thresholds, the no-notes case, sent-item exclusion in counts and samples, live paging caps, the 3-thread category limit.
