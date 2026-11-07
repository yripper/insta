use crate::ig::types::IgError;
use crate::whisper::WhisperError;

/// What a tool call can fail with. The Display text goes back to Claude, so it
/// says what to do next.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Ig(#[from] IgError),
    #[error(transparent)]
    Whisper(#[from] WhisperError),
    #[error("\"{query}\" matches several chats: {}. Ask the user which one and pass its id.", candidates.join(", "))]
    Ambiguous { query: String, candidates: Vec<String> },
    #[error("no 1:1 chat matches \"{0}\". Try list_chats or a shorter part of the name.")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("local store error: {0}")]
    Store(#[from] anyhow::Error),
}

impl Error {
    /// A label safe for logs (no names, no message text).
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Ig(IgError::NeedsLogin) => "needs_login",
            Error::Ig(IgError::Challenge) => "challenge",
            Error::Ig(IgError::RateLimited { .. }) => "rate_limited",
            Error::Ig(IgError::NotFound) => "ig_not_found",
            Error::Ig(IgError::Http(_)) => "http",
            Error::Ig(IgError::Decode(_)) => "decode",
            Error::Whisper(_) => "whisper",
            Error::Ambiguous { .. } => "ambiguous",
            Error::NotFound(_) => "not_found",
            Error::Invalid(_) => "invalid",
            Error::Store(_) => "store",
        }
    }

    /// The Instagram session is unusable (expired or challenged).
    pub fn is_fatal(&self) -> bool {
        matches!(self, Error::Ig(e) if e.is_fatal())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_carry_no_names_and_fatal_is_session_only() {
        let e = Error::Ambiguous { query: "ana".into(), candidates: vec!["@ana (TH001)".into()] };
        assert_eq!(e.kind(), "ambiguous");
        assert!(e.to_string().contains("@ana (TH001)"));
        assert!(Error::Ig(IgError::Challenge).is_fatal());
        assert!(Error::Ig(IgError::NeedsLogin).is_fatal());
        assert!(!Error::Ig(IgError::NotFound).is_fatal());
        assert!(!Error::Invalid("x".into()).is_fatal());
        assert_eq!(Error::Whisper(WhisperError::MissingKey).kind(), "whisper");
    }
}
