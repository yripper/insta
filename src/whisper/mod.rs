//! Speech to text for voice notes. `OpenAiWhisper` calls OpenAI's
//! transcription endpoint; the key lives in the OS keyring. Nothing is
//! written to disk and neither audio nor key is ever logged.

pub mod openai;

#[cfg(test)]
pub mod fake;

use async_trait::async_trait;

pub use openai::OpenAiWhisper;

/// OpenAI's upload limit for transcriptions.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WhisperError {
    #[error("no OpenAI API key saved: run `insta set-openai-key` in a terminal")]
    MissingKey,
    #[error("audio is {0} bytes, over OpenAI's 25 MB limit")]
    TooLarge(usize),
    #[error("OpenAI rejected the API key: run `insta set-openai-key` with a valid key")]
    BadKey,
    #[error("OpenAI transcription failed: {0}")]
    Api(String),
    #[error("HTTP error talking to OpenAI: {0}")]
    Http(String),
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    /// Fails fast (e.g. no key) before insta downloads any audio.
    async fn check_ready(&self) -> Result<(), WhisperError> {
        Ok(())
    }
    /// `file_name`'s extension tells OpenAI the audio format.
    async fn transcribe(&self, audio: Vec<u8>, file_name: &str) -> Result<String, WhisperError>;
}

/// A file name whose extension matches the audio's container, from its magic
/// bytes, so OpenAI decodes it right. Unknown containers fall back to mp4,
/// the most common container for phone voice notes.
pub fn audio_file_name(audio: &[u8]) -> &'static str {
    let starts = |magic: &[u8]| audio.starts_with(magic);
    if audio.len() >= 8 && &audio[4..8] == b"ftyp" {
        "voice.m4a"
    } else if starts(b"OggS") {
        "voice.ogg"
    } else if starts(b"RIFF") {
        "voice.wav"
    } else if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "voice.webm"
    } else if starts(b"ID3") || (audio.len() >= 2 && audio[0] == 0xFF && audio[1] & 0xE0 == 0xE0) {
        "voice.mp3"
    } else {
        "voice.mp4"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_follows_magic_bytes() {
        assert_eq!(audio_file_name(b"\0\0\0\x18ftypM4A "), "voice.m4a");
        assert_eq!(audio_file_name(b"OggS\0\x02"), "voice.ogg");
        assert_eq!(audio_file_name(b"RIFF\0\0\0\0WAVE"), "voice.wav");
        assert_eq!(audio_file_name(&[0x1A, 0x45, 0xDF, 0xA3, 0]), "voice.webm");
        assert_eq!(audio_file_name(b"ID3\x04"), "voice.mp3");
        assert_eq!(audio_file_name(&[0xFF, 0xFB, 0x90]), "voice.mp3");
        assert_eq!(audio_file_name(b""), "voice.mp4");
        assert_eq!(audio_file_name(b"??"), "voice.mp4");
    }
}
