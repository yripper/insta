//! Domain-facing Instagram types. Wire formats get parsed in `http.rs` and
//! mapped into these, so the rest of the crate never sees raw Instagram JSON.

use serde::{Deserialize, Serialize};

/// Unix time in microseconds, the resolution Instagram's direct items use.
pub type Micros = i64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Me {
    pub user_id: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub full_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub is_group: bool,
    /// The other person in a 1:1 thread. `None` for groups.
    pub peer: Option<User>,
    /// Timestamp of the newest item in the thread.
    pub last_activity_ts: Micros,
    pub last_item_from_me: bool,
    /// The user hasn't seen the newest incoming item.
    pub unread: bool,
    /// Muted in the Instagram app itself.
    pub muted_in_app: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    Link,
    Reel,
    Post,
    Story,
    Gif,
    Voice,
    Image,
    Video,
    Like,
    /// A type insta doesn't map, with Instagram's own type name.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub ts: Micros,
    pub from_me: bool,
    pub sender_id: String,
    pub kind: MessageKind,
    pub text: Option<String>,
    /// Shared reel/post/story or link URL.
    pub link: Option<String>,
    /// Voice note audio URL (on Instagram's CDN).
    pub media_url: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboxPage {
    pub threads: Vec<Thread>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadPage {
    /// Newest first.
    pub messages: Vec<Message>,
    pub older_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMessage {
    pub item_id: String,
    pub ts: Micros,
}

/// Browser session cookies. Never printed: `Debug` redacts all of them.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub sessionid: String,
    pub csrftoken: String,
    pub ds_user_id: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials { sessionid: <redacted>, csrftoken: <redacted>, ds_user_id: <redacted> }")
    }
}

/// The text the stub returns until the real endpoints land.
pub const NOT_IMPLEMENTED: &str = "not implemented yet";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IgError {
    #[error("Instagram session expired or invalid: run `insta login`")]
    NeedsLogin,
    #[error(
        "Instagram asked for a security check (challenge / \"please wait a few minutes\"). insta stopped all calls. Open Instagram in the browser, clear the check, then run `insta login`"
    )]
    Challenge,
    #[error("Instagram rate limit hit{}", retry_after_secs.map(|s| format!("; retry in {s}s")).unwrap_or_default())]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("Instagram says that item or thread doesn't exist")]
    NotFound,
    #[error("HTTP error talking to Instagram: {0}")]
    Http(String),
    #[error("unexpected Instagram response: {0}")]
    Decode(String),
}

impl IgError {
    /// The session can't be used any more: stop sending, wait for `insta login`.
    pub fn is_fatal(&self) -> bool {
        matches!(self, IgError::NeedsLogin | IgError::Challenge)
    }

    /// True for the stub's placeholder error, so `insta login` can tell
    /// "invalid cookies" from "validation not possible yet".
    pub fn is_not_implemented(&self) -> bool {
        matches!(self, IgError::Http(m) if m == NOT_IMPLEMENTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_redacts_everything() {
        let c = Credentials { sessionid: "s3cr3t".into(), csrftoken: "c5rf".into(), ds_user_id: "U001".into() };
        let out = format!("{c:?}");
        for v in ["s3cr3t", "c5rf", "U001"] {
            assert!(!out.contains(v), "{out}");
        }
    }

    #[test]
    fn error_classes() {
        assert!(IgError::NeedsLogin.is_fatal());
        assert!(IgError::Challenge.is_fatal());
        assert!(!IgError::RateLimited { retry_after_secs: None }.is_fatal());
        assert!(IgError::Http(NOT_IMPLEMENTED.into()).is_not_implemented());
        assert!(!IgError::Http("timeout".into()).is_not_implemented());
        assert_eq!(
            IgError::RateLimited { retry_after_secs: Some(9) }.to_string(),
            "Instagram rate limit hit; retry in 9s"
        );
        assert_eq!(IgError::RateLimited { retry_after_secs: None }.to_string(), "Instagram rate limit hit");
        assert!(IgError::NeedsLogin.to_string().contains("insta login"));
    }
}
