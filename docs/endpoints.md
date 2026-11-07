# Instagram web DM endpoints (captured 2026-11-07)

Ground truth for `src/ig/http.rs`. Captured from instagram.com/direct in a logged-in browser,
then replayed from a non-browser client (Playwright APIRequestContext) to confirm the minimal
request. All ids below are fake. When Instagram changes something, re-capture and update this
file first, then the code.

Web DMs no longer use the old REST API: `GET /api/v1/direct_v2/inbox/` returns 404 (an HTML page)
for a web session. Everything goes through GraphQL. The page also opens websockets
(gateway.instagram.com/ws/*, edge-chat), but reading, sending and marking seen are all plain
HTTP GraphQL calls; the websockets only carry realtime push, typing and presence, which insta
doesn't need.

## Auth

- Cookies: the browser session for `.instagram.com`. Confirmed with the full cookie jar; the
  minimal subset (`sessionid`, `csrftoken`, `ds_user_id`) is what `insta login` asks for and must
  be confirmed on the first real run (`insta status`).
- `ds_user_id` == the viewer's Instagram id (`viewer.id` in thread responses).
- `fb_dtsg`: a per-session CSRF token for GraphQL. Fetch `GET https://www.instagram.com/direct/inbox/`
  with the cookies and a browser User-Agent, then extract it from the HTML with
  `"DTSGInitialData",[],{"token":"<fb_dtsg>"` (a fallback pattern `"dtsg":{"token":"<fb_dtsg>"` also
  matched). Cache it in memory and refetch on failure. Not found in the HTML → treat as NeedsLogin.
- There's no refresh token. When `sessionid` dies, the user logs in again in the browser.

## GraphQL call

```
POST https://www.instagram.com/api/graphql
Content-Type: application/x-www-form-urlencoded
User-Agent: <desktop Chrome UA>
X-IG-App-ID: 936619743392459
Sec-Fetch-Site: same-origin        # REQUIRED: without it the response is an HTML page
Cookie: <session cookies>

fb_dtsg=<fb_dtsg>&doc_id=<doc_id>&variables=<json>&fb_api_req_friendly_name=<name>
```

Also send `Accept-Language` (insta: `es-CL,es;q=0.9,en;q=0.8`): without it system lines ("liked a message") come back in an arbitrary locale.

Bisected: Origin, Referer, Sec-Fetch-Mode/Dest, X-CSRFToken, X-FB-LSD/lsd and X-ASBD-ID are all
optional. Without `fb_dtsg`, or without `Sec-Fetch-Site`, the response is HTML. The response is JSON
`{data, extensions}`; on failure it's `{errors:[{message}]}` (e.g. "missing_required_variable_value").

`doc_id`s are tied to the web bundle and can change when Instagram deploys. They live in one table
in code (overridable from config.json) so a change is a config edit, not a release.

Every `__relay_internal__pv__*` variable below is a feature flag the web client sends; send the
same values.

### Inbox (first page)
name `PolarisDirectInboxQuery`, doc_id `28881837051457834`
vars: `{device_id_for_iris_subscription: <uuid v4, generate once per install and keep>,
  __relay_internal__pv__IGDIsProfessionalAccountGKrelayprovider: false,
  __relay_internal__pv__IGDPinnedThreadsRenderEnabledGKrelayprovider: true,
  __relay_internal__pv__IGDMaxUnreadMessagesCountrelayprovider: 5,
  __relay_internal__pv__IGDThreadListActionsEnabledGKrelayprovider: true}`
(no device id → `missing_required_variable_value`)
resp: `data.get_slide_mailbox_for_iris_subscription`:
- `id` — the **mailbox id** (needed for the next page; not the viewer id)
- `threads_by_folder.edges[].node.as_ig_direct_thread` → Thread (below), 15 per page
- `threads_by_folder.page_info {end_cursor, has_next_page}`

### Inbox (next pages)
name `IGDThreadListOffMsysPaginationQuery`, doc_id `29461858216751557`
vars: `{count: <n>, cursor: <end_cursor>, folder: "INBOX", newer_than_timestamp_ms: null, id: <mailbox id>,
  __relay_internal__pv__IGDPinnedThreadsRenderEnabledGKrelayprovider: true,
  __relay_internal__pv__IGDMaxUnreadMessagesCountrelayprovider: 5,
  __relay_internal__pv__IGDThreadListActionsEnabledGKrelayprovider: true}`
resp: `data.fetch__SlideMailbox.threads_by_folder` (same edges/page_info shape). Confirmed with count 10.

### Thread object (inbox and detail)
- `id` (== `thread_fbid`): the thread id for **messages, send, mark seen**. Use this as insta's thread id.
- `thread_key`: a second id; only `IGDThreadDetailQuery` takes it. Not needed (see message list).
- `thread_id`: a third (legacy) id; unused.
- `is_group: bool`; `thread_subtype` e.g. `IG_ONLY_ONE_TO_ONE`, `IG_BUSINESS_ACCOUNT_ONE_TO_ONE`, `IGD_GROUP`.
- `folder` (`PRIMARY`…), `system_folder` (`INBOX`).
- `last_activity_timestamp_ms: string(ms)`, `is_muted: bool`, `marked_as_unread: bool`, `is_pin`.
- `thread_title`
- `users[]` (without the viewer): `{id (== pk, IG id), interop_messaging_user_fbid, username, full_name, is_verified, friendship_status}`
- `viewer {id (IG id == ds_user_id), interop_messaging_user_fbid}`
- `slide_read_receipts[] {participant_fbid, watermark_timestamp_ms}`: one per participant, keyed by
  **interop fbid**. Unread for me = newest incoming `timestamp_ms` > my watermark, or `marked_as_unread`.
- `slide_messages.edges[].node`: the newest 5 messages in the inbox (preview nodes are lighter: `sender_fbid,
  timestamp_ms, id, content_type, text_body, igd_snippet, content.__typename`).

### Messages (first page and older pages)
name `IGDMessageListOffMsysQuery`, doc_id `28947835764870439`
vars: `{after: <null for the newest page | end_cursor>, before: null, first: 20, last: null,
  newer_than_message_id: null, older_than_message_id: null, id: <thread id>,
  __relay_internal__pv__IGDInitialMessagePageCountrelayprovider: 20}`
resp: `data.fetch__SlideThread.as_ig_direct_thread {id, slide_messages {edges[{node, cursor}], page_info {end_cursor, has_next_page, has_previous_page, start_cursor}}}`
- Newest first. `after = end_cursor` gives the next older page. Confirmed `after: null` returns the
  newest 20, so reading never needs `thread_key`.

### Message node
- `id` == `message_id`: `"mid.$…"` (34 chars)
- `sender_fbid`: compare with `viewer.interop_messaging_user_fbid` (from the inbox) → from_me. Not the IG id.
- `sender {name, id, igid, user_dict {username, full_name, …}}` (present in detail/message-list nodes). `sender.igid` == the user's Instagram id (`users[].id` / `viewer.id`): checked on 941 of 977 captured messages, the rest from group members.
- `timestamp_ms: string(ms)`, `text_body`, `content_type`, `content.__typename`
- `reactions[] {reaction, sender_fbid, …}`, `replied_to_message_id`, `igd_is_forwarded`, `is_ai_generated`

Content kinds seen (content_type | content.__typename):
| kind | where the useful bits are |
|---|---|
| `TEXT` \| `SlideMessageText` | `text_body` |
| `TEXT`/`REACTION_LOG_XMAT` \| `SlideMessageAdminText` | system line ("reacted …"): `content.text_fragments[].plaintext`; render as a system marker or skip |
| `MESSAGE_INLINE_SHARE` \| `SlideMessageXMAContent` + `xma.__typename` `SlideMessagePortraitXMA` | reel share: `xma.target_url` (path `/reel/…`), `xma.header_title_text` (author) |
| same + `SlideMessageStandardXMA` | post share: `xma.target_url` (path `/p/…`), `xma.title_text`, `xma.caption_body_text` |
| `MONTAGE_SHARE_XMA` \| `SlideMessageXMAContent` (Portrait / `SlideMessageExpiredPlaceholderXMA`) | story share / story reply; `text_body` holds the reply text; expired → placeholder |
| `…` + `SlideMessagePlaceholderXMA` | unavailable share: `xma.title_text` |
| `AUDIOS` \| `SlideMessageAudiosContent` | voice note: `content.audio_attachments[0] {attachment_cdn_url, playable_duration_ms}` |
| `IMAGES` \| `SlideMessageImageContent` | `content.attachments[] {attachment_cdn_url, preview_cdn_url}` |
| `RAVEN_IMAGE` \| `SlideMessageRavenImageContent` | view-once photo; `attachment` is null |
| `INSTAGRAM_MESSAGING_ANIMATED_IMAGE` \| `SlideMessageAnimatedMediaContent` | GIF/sticker: `animated_media[0] {is_sticker, attachment_mp4_url}` |
Anything else → `Other(content_type)`.

### Send text
name `IGDirectTextSendMutation`, doc_id `28737274835943013`
vars:
```json
{"ig_thread_igid": "<thread id>", "offline_threading_id": "<otid>", "recipient_igids": null,
 "replied_to_client_context": null, "replied_to_item_id": null, "reply_to_message_id": null,
 "sampled": null, "text": {"sensitive_string_value": "<text>"}, "mentions": [], "mentioned_user_ids": [],
 "commands": null, "forwarded_from_thread_id": null, "is_forwarded_from_own_message": null,
 "send_attribution": "igd_web_chat_tab:in_thread"}
```
- `offline_threading_id` = `(now_ms << 22) | random 22 bits`, as a decimal string (checked: the captured
  otid >> 22 was the send time in ms). It's the client's dedupe key; reuse it on a retry of the same message.
resp: `data.xig_direct_text_send_with_slide_messaging_response {message_id, id, timestamp_ms}`
Verified end to end from insta on 2026-11-07: the sent message came back in `IGDMessageListOffMsysQuery` with the returned `message_id`, marked from me.

### Mark seen
name `useIGDMarkThreadAsReadMutation`, doc_id `27399783383056109`
vars: `{"metadata": {"ig_thread_igid": "<thread id>"}, "data": {"item_id": "", "message_id": "<mid.$… of the newest message to mark>"}}`
resp: `data.xig_direct_item_seen_mutation_with_slide_messaging_response {participant_eimu_id, watermark_timestamp_ms}`
The web app also fires `useIGDMarkThreadAsReadValidationMutation` (doc `35211594988486314`,
`data {message_id, message_timestamp_ms}` → `{xig_direct_mark_read_id_validation: bool}`) at the same
moment; it's a client-side check, insta skips it. Opening a chat marks seen and so does sending: the
app fired a mark-seen right before the send mutation.

### Voice note download
`GET <attachment_cdn_url>` on `*.fbsbx.com`: a signed URL (has an `oe=` expiry), works **without cookies**
→ `video/mp4` (AAC audio, ~300 KB for a short note). Whisper accepts mp4. The URL expires, so fetch the
message fresh before downloading.

## Errors (not yet observed; best guesses to confirm)
- Logged out / dead session: the inbox HTML has no DTSG token (and the `<html>` class has `not-logged-in`)
  → NeedsLogin.
- Checkpoint / challenge: a redirect to `/challenge/` or `/accounts/suspended/`, or JSON with
  `checkpoint_required` / `challenge_required` / `"message":"feedback_required"` → Challenge (halt).
- 429 or `"Please wait a few minutes"` → RateLimited.
- GraphQL `errors[]` → Http(message). HTML where JSON was expected (with a valid session) → Decode("unexpected html").

## Viewer
- `Me.user_id` = `ds_user_id`. The viewer's interop fbid comes from any inbox thread's
  `viewer.interop_messaging_user_fbid`; cache it.
- The username isn't in the inbox HTML or the message-list response (`viewer.username` only appears
  in `IGDThreadDetailQuery`). Keep it optional.
