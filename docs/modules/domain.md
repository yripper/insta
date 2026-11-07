# domain

Last verified: 2026-11-07

## Purpose
What each MCP tool does, independent of MCP: resolving chats, the pending rule, rendering, labels, and the glue to the send queue, voice and whisper. Voice and the send queue have their own docs ([voice.md](voice.md), [send_queue.md](send_queue.md)).

## Structure
- `src/domain/actions.rs`: `Actions`, one method per tool, plus the report structs.
- `src/domain/directory.rs`: pure `resolve`, `to_cached`, `display_name`, and `Directory` (inbox refresh + cached lookups).
- `src/domain/pending.rs`: the pure pending rule.
- `src/domain/render.rs`: `RenderedMessage`, `Sender`, markers.
- `src/domain/clock.rs`: `Timer` / `GapRng` traits, `SystemTimer`, `ThreadRng`, and test fakes `FakeTimer` (virtual clock; sleeping moves time) and `SeqGaps`.
- `src/domain/errors.rs`: `Error` (Ig, Whisper, Ambiguous, NotFound, Invalid, Store), `kind()` (log-safe, `src/domain/errors.rs:24`), `is_fatal()` (`:41`).

## Public surface (`Actions`)
`list_chats` (`src/domain/actions.rs:198`), `pending` (`:217`), `read_history` (`:267`), `mark_read` (`:288`), `send` (`:298`), `send_status` (`:303`), `set_label` (`:307`), `clear_label` (`:334`), `set_watch` (`:340`), `set_category` (`:346`), `get_voice` (`:355`), `get_my_messages` (`:375`), `save_profile` (`:395`), `save_chat_notes` (`:404`), `list_stale_profiles` (`:411`), `transcribe_voice` (`:417`).

## Key methods
- `resolve` (`src/domain/directory.rs:27`): exact thread id → One; then exact username (or full name, unless the query starts with `@`) → else substring. Several matches is `Many` (the error lists `@user (Name) (id)`): never a guess. Only 1:1 threads are cached (`to_cached`, `:61`), so groups can't be resolved.
- `Directory::refresh` (`src/domain/directory.rs:89`): up to `INBOX_PAGES = 3` pages of `INBOX_PAGE = 20` (`:16`), keeps 1:1 threads, upserts the cache. `resolve` (`:113`) refreshes when the cache is older than 10 min, and once more on a miss if older than 60 s.
- Pending rule (`src/domain/pending.rs`): `is_pending` (`:32`) = 1:1 with a peer, newest item not from me, not muted, no active label. `is_muted` (`:16`): a local watch level wins; otherwise muted in the Instagram app counts as muted. `label_active` (`:25`): done/waiting stay while `last_activity_ts <= set_at_ts` (or I wrote last); snoozed stays while `snooze_until > now`. `select_pending` (`:42`) sorts newest first.
- `Actions::pending` (`src/domain/actions.rs:217`): one inbox refresh, deletes labels that ended, and fetches messages (10, their run since my last message) only for the first `PENDING_DETAIL = 5` chats (`:25`): each is a paced call. The rest come back with `not_fetched`. Per-chat failures are reported in `error`; a fatal error aborts (`soft`, `:450`).
- `set_label`: `set_at_ts` = the newest message's ts (one `thread(limit 1)` call). Validates note ≤ 500 chars, snoozed needs a future `snooze_until`, other labels refuse one.
- Render (`src/domain/render.rs`): `from` is `"me"` or `{id, username, name}` (`Sender`, `:12`). `marker` (`:64`): `[reel] <link>`, `[post]`, `[story]`, `[gif]`, `[voice m:ss] — call transcribe_voice`, `[image]`, `[video]`, `[like]`, `[<other>]`; `[link] url` only when the text lacks the url. Each message carries `ts` (µs) and `at` (RFC 3339 UTC).
- `transcribe_voice` (`src/domain/actions.rs:417`): `check_ready` first (no download without a key), finds the item in up to 3 pages of 50, requires `Voice` + `media_url`, `fetch_media` capped at 25 MiB, then Whisper.

## Dependencies
ig (trait only), store, whisper (trait only), chrono, serde.

## Gotchas & invariants
- Nothing here marks seen except `mark_read` (and the send queue).
- Every thread page read goes through `observe_own` so voice staleness counts stay current.
- `Actions` uses the injected `Timer` for "now", so tests are deterministic.

## Testing
`cargo test domain`: resolve rules (id, @, exact vs partial, Many, None), refresh paging and cache TTLs, pending rule incl. label clearing and snooze, render markers, the action flows against `FakeInstagram` + `FakeWhisper`.
