use async_trait::async_trait;

use super::types::{IgError, InboxPage, Me, SentMessage, ThreadPage};

pub type IgResult<T> = Result<T, IgError>;

/// Everything insta needs from Instagram. `HttpInstagram` talks to the real
/// web endpoints; `fake::FakeInstagram` backs the tests.
#[async_trait]
pub trait InstagramApi: Send + Sync {
    /// The logged-in account. Also how a session is validated.
    async fn whoami(&self) -> IgResult<Me>;
    /// Threads newest first. May include groups; the domain keeps 1:1 only.
    async fn inbox(&self, cursor: Option<&str>, limit: usize) -> IgResult<InboxPage>;
    /// Messages newest first, plus a cursor for older ones.
    async fn thread(&self, thread_id: &str, cursor: Option<&str>, limit: usize) -> IgResult<ThreadPage>;
    async fn send_text(&self, thread_id: &str, text: &str) -> IgResult<SentMessage>;
    /// Shows "Seen" to the other person, up to `item_id`.
    async fn mark_seen(&self, thread_id: &str, item_id: &str) -> IgResult<()>;
    /// Downloads a media URL Instagram returned (voice notes). Fails past `max_bytes`.
    async fn fetch_media(&self, url: &str, max_bytes: usize) -> IgResult<Vec<u8>>;
}
