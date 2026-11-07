# ig

Last verified: 2026-11-07

## Purpose
The only code that talks HTTP to Instagram (and its CDN). Everything else uses the `InstagramApi` trait. Requests mirror what instagram.com/direct sends; the full captured reference (operations, variables, response shapes, how each was verified) is [`docs/endpoints.md`](../endpoints.md). Update that file first when Instagram changes something, then the code.

## Structure
- `src/ig/api.rs`: `InstagramApi` (`src/ig/api.rs:10`) and `IgResult`.
- `src/ig/types.rs`: domain types, `Credentials` (redacting `Debug`), `IgError`.
- `src/ig/http.rs`: `HttpInstagram`: session token, pacing, GraphQL POSTs, CDN download.
- `src/ig/wire.rs`: pure JSON → domain mapping (`serde_json::Value` navigation, so one odd message degrades instead of failing the page). Tests in `src/ig/wire_tests.rs`.
- `src/ig/fake.rs` (`#[cfg(test)]`): `FakeInstagram` with canned data and call recording.

## Public surface
```rust
async fn whoami(&self) -> IgResult<Me>;
async fn inbox(&self, cursor: Option<&str>, limit: usize) -> IgResult<InboxPage>;          // newest first, includes groups
async fn thread(&self, thread_id: &str, cursor: Option<&str>, limit: usize) -> IgResult<ThreadPage>; // newest first + older_cursor
async fn send_text(&self, thread_id: &str, text: &str) -> IgResult<SentMessage>;
async fn mark_seen(&self, thread_id: &str, item_id: &str) -> IgResult<()>;
async fn fetch_media(&self, url: &str, max_bytes: usize) -> IgResult<Vec<u8>>;
```
`HttpInstagram::raw(what)` (`src/ig/http.rs:121`) returns the raw JSON behind `inbox` / `thread` / `whoami` for `insta probe`.

## Key methods
- GraphQL operations (name + `doc_id`) are constants at `src/ig/http.rs:40-45`.
- `dtsg` (`src/ig/http.rs:139`): GETs `/direct/inbox/` once per process and extracts `fb_dtsg` (`wire::extract_dtsg`, `src/ig/wire.rs:72`). No token in the page → `NeedsLogin`.
- `graphql` (`src/ig/http.rs:166`): POST `/api/graphql` with `fb_dtsg`, `doc_id`, `variables`, `fb_api_req_friendly_name` and the headers that are required: `X-IG-App-ID`, `Sec-Fetch-Site: same-origin` (without it Instagram answers HTML). An HTML answer to a read drops the cached token and retries once; sends never retry.
- `check_landing` (`src/ig/http.rs:258`): final URL `/challenge*` or `/accounts/suspended` → `Challenge`, `/accounts/login` → `NeedsLogin`, 429 → `RateLimited` (with `Retry-After`), 401/403 → `NeedsLogin`.
- `wire::graphql_error` (`src/ig/wire.rs:43`): `checkpoint`/`challenge`/`feedback_required` → `Challenge`, `login_required` → `NeedsLogin`, "please wait"/rate limit → `RateLimited`, other `errors[]` without data → `Http`.
- `viewer` (`src/ig/http.rs:239`): learns the viewer's interop fbid and the mailbox id from the first inbox page and caches them. Message senders are keyed by interop fbid, so `thread` needs it to tell "from me".
- `inbox` (`src/ig/http.rs:345`): first page via `PolarisDirectInboxQuery` (fixed 15 threads, needs a device id; one random UUID per process). Later pages via `IGDThreadListOffMsysPaginationQuery`, which needs the mailbox id, so insta's cursor is `"<mailbox_id>|<end_cursor>"` (`encode_inbox_cursor`, `:301`).
- `thread` (`src/ig/http.rs:378`): `IGDMessageListOffMsysQuery` with `after: null` for the newest page; `after: end_cursor` pages older.
- `whoami` (`src/ig/http.rs:328`): inbox (validates the session) + one `IGDThreadDetailQuery` on the newest thread's `thread_key`, the only response carrying the viewer's username. Empty inbox → username falls back to the user id.
- `send_text` (`src/ig/http.rs:384`): `IGDirectTextSendMutation`; `offline_threading_id` = `(now_ms << 22) | random22` (`:311`).
- `mark_seen` (`src/ig/http.rs:397`): `useIGDMarkThreadAsReadMutation` with the `mid.$…` id of the newest message to mark.
- `fetch_media` (`src/ig/http.rs:406`): https only, hosts ending in `.fbsbx.com`, `.cdninstagram.com`, `.fbcdn.net`; **no cookies** (signed URLs don't need them); size cap enforced on `Content-Length` and while streaming.
- `pace` (`src/ig/http.rs:109`): every request (CDN included) waits a random 1.5–3 s after the previous one.
- `wire::parse_message` (`src/ig/wire.rs:186`): content `__typename` → `MessageKind`. Reel/post/story shares are "XMA" cards (`apply_xma`, `:230`): `/reel/` → Reel, `/p/` → Post, `MONTAGE_*` → Story (story replies keep `text_body`), other target → Link, none → `Other("unavailable_share")`. Reaction logs → Like; other admin lines → `Other("system")`; voice → Voice with `media_url` + `duration_ms`.
- Unread (`wire::parse_thread`, `src/ig/wire.rs:128`): `marked_as_unread`, or the newest incoming preview is newer than my `slide_read_receipts` watermark.

## Dependencies
reqwest (rustls), rand, tokio, serde_json, thiserror. Dev: wiremock, url.

## Gotchas & invariants
- Three thread ids exist: `id` (== `thread_fbid`) is insta's thread id and what messages/send/mark take; `thread_key` is only for `IGDThreadDetailQuery`; `thread_id` is unused.
- `doc_id`s are tied to Instagram's web build. When calls start returning "returned a web page instead of JSON" with a valid session, or GraphQL errors about variables, re-capture (docs/endpoints.md) and update the constants.
- The error mappings for a dead session and for checkpoints haven't been observed live yet (docs/endpoints.md "Errors"); they're the documented best guesses.
- `Message.sender_id` is the Instagram user id from `sender.igid` (matches `Thread.peer.id`); `sender_fbid` is an interop id used only to tell "from me". Verified on 941/977 captured messages (the rest were group members).
- Every request sends `Accept-Language: es-CL,es;q=0.9,en;q=0.8`; without it Instagram chose a locale itself (system lines came back in Thai). Reel/post links drop their tracking query string.
- Never log URLs (signed CDN links), cookies or bodies: `http_err` uses `without_url()`, and the only log line is op name + status.
- Don't add cookies to `fetch_media`: the session must never leave instagram.com.
- Mutations (`send_text`) never retry automatically, so a message is never sent twice.

## Testing
`cargo test ig`. `wire_tests.rs`: fixture JSON with fake ids for every content kind, unread/from-me logic, cursors, error classification. `http_tests.rs`: wiremock checks of the exact request (headers, form fields, variables), token caching, HTML retry, send-no-retry, challenge redirect, 429, checkpoint JSON, cursor round trip, media cap / host allowlist / no cookies. Pacing is off on the mock server (`without_pacing`); its timing is tested on a paused clock.
