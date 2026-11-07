//! `InstagramApi` over Instagram's web GraphQL endpoints with a browser
//! session. Every request and response shape here was captured from
//! instagram.com/direct and replayed outside the browser; see
//! `docs/endpoints.md` before changing anything.

use std::time::Duration;

use async_trait::async_trait;
use rand::Rng;
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, RETRY_AFTER};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::api::{IgResult, InstagramApi};
use super::types::{Credentials, IgError, InboxPage, Me, SentMessage, ThreadPage};
use super::wire::{self, Viewer};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const WEB_BASE: &str = "https://www.instagram.com";
const ACCEPT_LANGUAGE: &str = "es-CL,es;q=0.9,en;q=0.8";
/// The web client's app id; GraphQL calls carry it as `X-IG-App-ID`.
const IG_APP_ID: &str = "936619743392459";
/// Instagram flags accounts that burst like a scraper, so every call waits a
/// random 1.5–3 s after the previous one. Random so the rhythm isn't robotic.
pub const MIN_GAP_MS: u64 = 1_500;
pub const MAX_GAP_MS: u64 = 3_000;
/// Voice notes live on Meta's CDNs. Anything else is refused so a crafted
/// URL can't turn `fetch_media` into a fetch-anything.
const MEDIA_HOST_SUFFIXES: &[&str] = &[".fbsbx.com", ".cdninstagram.com", ".fbcdn.net"];

/// One GraphQL operation: the web client's friendly name and its persisted
/// query id. The ids are tied to Instagram's web bundle and change when it
/// deploys; re-capture them (docs/endpoints.md) when calls start failing.
#[derive(Debug, Clone, Copy)]
pub struct Op {
    pub name: &'static str,
    pub doc_id: &'static str,
}

pub const INBOX: Op = Op { name: "PolarisDirectInboxQuery", doc_id: "28881837051457834" };
pub const INBOX_PAGE: Op = Op { name: "IGDThreadListOffMsysPaginationQuery", doc_id: "29461858216751557" };
pub const MESSAGES: Op = Op { name: "IGDMessageListOffMsysQuery", doc_id: "28947835764870439" };
pub const THREAD_DETAIL: Op = Op { name: "IGDThreadDetailQuery", doc_id: "28655271404173792" };
pub const SEND_TEXT: Op = Op { name: "IGDirectTextSendMutation", doc_id: "28737274835943013" };
pub const MARK_SEEN: Op = Op { name: "useIGDMarkThreadAsReadMutation", doc_id: "27399783383056109" };

/// What the session taught us so far. All of it is re-derivable, so it only
/// lives in memory.
#[derive(Default)]
struct SessionCache {
    dtsg: Option<String>,
    viewer: Option<Viewer>,
}

pub struct HttpInstagram {
    http: reqwest::Client,
    creds: Credentials,
    base: String,
    media_suffixes: Vec<String>,
    /// The inbox query requires a device id; the web app makes a fresh one
    /// per page load, so one per process is the same behavior.
    device_id: String,
    cache: Mutex<SessionCache>,
    /// When the next call may start.
    next_slot: Mutex<Option<Instant>>,
    /// Random gap between calls, in ms. Only tests turn it off.
    gap_ms: (u64, u64),
}

impl HttpInstagram {
    pub fn new(creds: Credentials) -> IgResult<HttpInstagram> {
        HttpInstagram::with_base(creds, WEB_BASE, MEDIA_HOST_SUFFIXES.iter().map(|s| s.to_string()).collect())
    }

    /// `base` and `media_suffixes` are swappable so tests can point at a mock server.
    pub fn with_base(creds: Credentials, base: &str, media_suffixes: Vec<String>) -> IgResult<HttpInstagram> {
        let mut headers = reqwest::header::HeaderMap::new();
        // Without it Instagram picks a locale on its own (it answered system
        // lines like "liked a message" in Thai).
        headers.insert(reqwest::header::ACCEPT_LANGUAGE, reqwest::header::HeaderValue::from_static(ACCEPT_LANGUAGE));
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| IgError::Http(e.to_string()))?;
        Ok(HttpInstagram {
            http,
            creds,
            base: base.trim_end_matches('/').to_owned(),
            media_suffixes,
            device_id: random_uuid(),
            cache: Mutex::new(SessionCache::default()),
            next_slot: Mutex::new(None),
            gap_ms: (MIN_GAP_MS, MAX_GAP_MS),
        })
    }

    #[cfg(test)]
    pub fn without_pacing(mut self) -> HttpInstagram {
        self.gap_ms = (0, 0);
        self
    }

    /// The `Cookie` header value a browser sends for this session.
    pub fn cookie_header(&self) -> String {
        format!(
            "sessionid={}; csrftoken={}; ds_user_id={}",
            self.creds.sessionid, self.creds.csrftoken, self.creds.ds_user_id
        )
    }

    /// Waits until this call's slot, then books the next one a random gap later.
    pub async fn pace(&self) {
        let mut slot = self.next_slot.lock().await;
        if let Some(at) = *slot
            && at > Instant::now()
        {
            tokio::time::sleep_until(at).await;
        }
        let gap = rand::rng().random_range(self.gap_ms.0..=self.gap_ms.1);
        *slot = Some(Instant::now() + Duration::from_millis(gap));
    }

    /// For `insta probe`: the raw JSON behind one trait method.
    pub async fn raw(&self, what: &str) -> IgResult<Value> {
        match what {
            "inbox" => self.graphql(INBOX, self.inbox_vars(), true).await,
            "thread" => {
                let first = self.inbox_first().await?;
                let t = first.page.threads.first().ok_or(IgError::NotFound)?;
                self.graphql(MESSAGES, messages_vars(&t.id, None, 20), true).await
            }
            "whoami" => {
                let first = self.inbox_first().await?;
                let key = first.first_thread_key.ok_or(IgError::NotFound)?;
                self.graphql(THREAD_DETAIL, thread_detail_vars(&key), true).await
            }
            _ => Err(IgError::Http(format!("unknown probe target {what}"))),
        }
    }

    /// The `fb_dtsg` token, fetched from the inbox HTML once per session.
    async fn dtsg(&self) -> IgResult<String> {
        if let Some(t) = self.cache.lock().await.dtsg.clone() {
            return Ok(t);
        }
        self.pace().await;
        let resp = self
            .http
            .get(format!("{}/direct/inbox/", self.base))
            .header(COOKIE, self.cookie_header())
            .header(ACCEPT, "text/html")
            .header("Sec-Fetch-Site", "none")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Dest", "document")
            .send()
            .await
            .map_err(http_err)?;
        check_landing(&resp)?;
        let html = resp.text().await.map_err(http_err)?;
        // A logged-out inbox page renders the login form without the token.
        let token = wire::extract_dtsg(&html).ok_or(IgError::NeedsLogin)?;
        self.cache.lock().await.dtsg = Some(token.clone());
        Ok(token)
    }

    /// POSTs one GraphQL operation. An HTML answer usually means a stale
    /// `fb_dtsg`, so reads refetch it once. Sends don't (`retry_on_html`
    /// false): a second attempt of a mutation is never automatic.
    async fn graphql(&self, op: Op, vars: Value, retry_on_html: bool) -> IgResult<Value> {
        let attempts = if retry_on_html { 2 } else { 1 };
        for attempt in 1..=attempts {
            let dtsg = self.dtsg().await?;
            self.pace().await;
            let form = [
                ("fb_dtsg", dtsg.as_str()),
                ("doc_id", op.doc_id),
                ("variables", &vars.to_string()),
                ("fb_api_req_friendly_name", op.name),
            ];
            let resp = self
                .http
                .post(format!("{}/api/graphql", self.base))
                .header(COOKIE, self.cookie_header())
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("X-IG-App-ID", IG_APP_ID)
                // Without this Instagram answers with an HTML page.
                .header("Sec-Fetch-Site", "same-origin")
                .header("X-CSRFToken", &self.creds.csrftoken)
                .form(&form)
                .send()
                .await
                .map_err(http_err)?;
            check_landing(&resp)?;
            let status = resp.status();
            let text = resp.text().await.map_err(http_err)?;
            tracing::debug!(op = op.name, status = status.as_u16(), "instagram call");
            match serde_json::from_str::<Value>(text.trim_start_matches("for (;;);")) {
                Ok(body) => {
                    if let Some(err) = wire::graphql_error(&body) {
                        return Err(err);
                    }
                    if !status.is_success() {
                        return Err(IgError::Http(format!("{} returned {status}", op.name)));
                    }
                    return Ok(body);
                }
                Err(_) if attempt < attempts => {
                    self.cache.lock().await.dtsg = None;
                }
                Err(_) => {
                    return Err(IgError::Decode(format!(
                        "{} returned a web page instead of JSON (session expired, or Instagram changed its API)",
                        op.name
                    )));
                }
            }
        }
        Err(IgError::Decode(format!("{}: no response", op.name)))
    }

    fn inbox_vars(&self) -> Value {
        json!({
            "device_id_for_iris_subscription": self.device_id,
            "__relay_internal__pv__IGDIsProfessionalAccountGKrelayprovider": false,
            "__relay_internal__pv__IGDPinnedThreadsRenderEnabledGKrelayprovider": true,
            "__relay_internal__pv__IGDMaxUnreadMessagesCountrelayprovider": 5,
            "__relay_internal__pv__IGDThreadListActionsEnabledGKrelayprovider": true,
        })
    }

    async fn inbox_first(&self) -> IgResult<wire::ParsedInbox> {
        let body = self.graphql(INBOX, self.inbox_vars(), true).await?;
        let parsed = wire::parse_inbox(&body)?;
        if let Some(v) = &parsed.viewer {
            self.cache.lock().await.viewer = Some(v.clone());
        }
        Ok(parsed)
    }

    /// The viewer's interop id and mailbox id, learning them from the inbox
    /// the first time.
    async fn viewer(&self) -> IgResult<Viewer> {
        if let Some(v) = self.cache.lock().await.viewer.clone() {
            return Ok(v);
        }
        self.inbox_first()
            .await?
            .viewer
            .ok_or_else(|| IgError::Decode("inbox is empty, can't tell which messages are yours".into()))
    }

    fn media_host_allowed(&self, url: &reqwest::Url) -> bool {
        let host = url.host_str().unwrap_or_default();
        let https_or_test = url.scheme() == "https" || self.base.starts_with("http://");
        https_or_test
            && self.media_suffixes.iter().any(|s| host.ends_with(s.as_str()) || host == s.trim_start_matches('.'))
    }
}

/// Instagram parks a broken session on a login, challenge or suspension page.
fn check_landing(resp: &reqwest::Response) -> IgResult<()> {
    let path = resp.url().path();
    if path.starts_with("/challenge") || path.starts_with("/accounts/suspended") {
        return Err(IgError::Challenge);
    }
    if path.starts_with("/accounts/login") {
        return Err(IgError::NeedsLogin);
    }
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_secs =
            resp.headers().get(RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        return Err(IgError::RateLimited { retry_after_secs });
    }
    if matches!(resp.status().as_u16(), 401 | 403) {
        return Err(IgError::NeedsLogin);
    }
    Ok(())
}

/// Never let a URL (signed CDN links, query strings) reach an error or a log.
fn http_err(e: reqwest::Error) -> IgError {
    IgError::Http(e.without_url().to_string())
}

fn messages_vars(thread_id: &str, cursor: Option<&str>, limit: usize) -> Value {
    json!({
        "after": cursor, "before": null, "first": limit, "last": null,
        "newer_than_message_id": null, "older_than_message_id": null, "id": thread_id,
        "__relay_internal__pv__IGDInitialMessagePageCountrelayprovider": limit,
    })
}

fn thread_detail_vars(thread_key: &str) -> Value {
    json!({
        "min_uq_seq_id": null, "thread_fbid": thread_key,
        "__relay_internal__pv__IGDEnableOffMsysChatThemesQErelayprovider": false,
        "__relay_internal__pv__IGDInitialMessagePageCountrelayprovider": 1,
    })
}

/// The inbox's next page needs the mailbox id as well as the cursor, so both
/// travel in the cursor insta hands out. Mailbox ids are digits, so the first
/// `|` is the separator.
fn encode_inbox_cursor(mailbox_id: &str, cursor: &str) -> String {
    format!("{mailbox_id}|{cursor}")
}

fn decode_inbox_cursor(c: &str) -> IgResult<(&str, &str)> {
    c.split_once('|').ok_or_else(|| IgError::Decode("invalid inbox cursor".into()))
}

/// The web client's dedupe key for a send: send time in ms shifted left 22
/// bits plus 22 random bits (checked against a captured send).
fn offline_threading_id(now_ms: u64, random22: u64) -> String {
    ((now_ms << 22) | (random22 & 0x3F_FFFF)).to_string()
}

fn random_uuid() -> String {
    let b: [u8; 16] = rand::rng().random();
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    // Version 4, RFC 4122 variant.
    format!("{}-{}-4{}-{:x}{}-{}", &h[0..8], &h[8..12], &h[13..16], (b[8] & 0x3) | 0x8, &h[17..20], &h[20..32])
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[async_trait]
impl InstagramApi for HttpInstagram {
    async fn whoami(&self) -> IgResult<Me> {
        let first = self.inbox_first().await?;
        // Only the thread-detail query carries the viewer's username; an
        // empty inbox falls back to the id.
        let username = match first.first_thread_key {
            Some(key) => {
                let body = self.graphql(THREAD_DETAIL, thread_detail_vars(&key), true).await?;
                wire::parse_viewer_username(&body)
            }
            None => None,
        };
        Ok(Me {
            user_id: self.creds.ds_user_id.clone(),
            username: username.unwrap_or_else(|| self.creds.ds_user_id.clone()),
        })
    }

    async fn inbox(&self, cursor: Option<&str>, limit: usize) -> IgResult<InboxPage> {
        let parsed = match cursor {
            None => self.inbox_first().await?,
            Some(c) => {
                let (mailbox_id, end_cursor) = decode_inbox_cursor(c)?;
                let vars = json!({
                    "count": limit.clamp(1, 50), "cursor": end_cursor, "folder": "INBOX",
                    "newer_than_timestamp_ms": null, "id": mailbox_id,
                    "__relay_internal__pv__IGDPinnedThreadsRenderEnabledGKrelayprovider": true,
                    "__relay_internal__pv__IGDMaxUnreadMessagesCountrelayprovider": 5,
                    "__relay_internal__pv__IGDThreadListActionsEnabledGKrelayprovider": true,
                });
                let body = self.graphql(INBOX_PAGE, vars, true).await?;
                // Pages after the first may come back without a viewer; parse
                // "from me" with the one we already know.
                let mut p = wire::parse_inbox(&body)?;
                if p.viewer.is_none() {
                    let v = self.viewer().await?;
                    p = wire::parse_inbox_with(&body, &v)?;
                }
                p
            }
        };
        let mailbox_id = match parsed.viewer {
            Some(v) => v.mailbox_id,
            None => self.cache.lock().await.viewer.as_ref().map(|v| v.mailbox_id.clone()).unwrap_or_default(),
        };
        let mut page = parsed.page;
        page.threads.truncate(limit.max(1));
        page.next_cursor = page.next_cursor.map(|c| encode_inbox_cursor(&mailbox_id, &c));
        Ok(page)
    }

    async fn thread(&self, thread_id: &str, cursor: Option<&str>, limit: usize) -> IgResult<ThreadPage> {
        let viewer = self.viewer().await?;
        let body = self.graphql(MESSAGES, messages_vars(thread_id, cursor, limit.clamp(1, 50)), true).await?;
        wire::parse_messages(&body, &viewer.interop_fbid)
    }

    async fn send_text(&self, thread_id: &str, text: &str) -> IgResult<SentMessage> {
        let otid = offline_threading_id(now_ms(), rand::rng().random_range(0..(1 << 22)));
        let vars = json!({
            "ig_thread_igid": thread_id, "offline_threading_id": otid, "recipient_igids": null,
            "replied_to_client_context": null, "replied_to_item_id": null, "reply_to_message_id": null,
            "sampled": null, "text": {"sensitive_string_value": text}, "mentions": [], "mentioned_user_ids": [],
            "commands": null, "forwarded_from_thread_id": null, "is_forwarded_from_own_message": null,
            "send_attribution": "igd_web_chat_tab:in_thread",
        });
        let body = self.graphql(SEND_TEXT, vars, false).await?;
        wire::parse_sent(&body)
    }

    async fn mark_seen(&self, thread_id: &str, item_id: &str) -> IgResult<()> {
        let vars = json!({"metadata": {"ig_thread_igid": thread_id}, "data": {"item_id": "", "message_id": item_id}});
        let body = self.graphql(MARK_SEEN, vars, true).await?;
        body.pointer("/data/xig_direct_item_seen_mutation_with_slide_messaging_response")
            .filter(|v| !v.is_null())
            .map(|_| ())
            .ok_or_else(|| IgError::Decode("mark seen: no response object".into()))
    }

    async fn fetch_media(&self, url: &str, max_bytes: usize) -> IgResult<Vec<u8>> {
        let parsed = reqwest::Url::parse(url).map_err(|_| IgError::Decode("media url is not a URL".into()))?;
        if !self.media_host_allowed(&parsed) {
            return Err(IgError::Decode("media url is not on Instagram's CDN".into()));
        }
        self.pace().await;
        // Signed CDN URLs work without cookies, so none are sent: the
        // session never leaves instagram.com.
        let mut resp = self.http.get(parsed).send().await.map_err(http_err)?;
        if !resp.status().is_success() {
            // Signed URLs expire; the caller refetches the message for a fresh one.
            return Err(if resp.status().as_u16() == 404 || resp.status().as_u16() == 403 {
                IgError::NotFound
            } else {
                IgError::Http(format!("media download returned {}", resp.status()))
            });
        }
        if resp.content_length().is_some_and(|n| n as usize > max_bytes) {
            return Err(IgError::Http(format!("media is larger than {max_bytes} bytes")));
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(http_err)? {
            if out.len() + chunk.len() > max_bytes {
                return Err(IgError::Http(format!("media is larger than {max_bytes} bytes")));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
