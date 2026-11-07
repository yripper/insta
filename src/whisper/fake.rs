//! Canned `Transcriber` for tests.

use std::sync::Mutex;

use async_trait::async_trait;

use super::{Transcriber, WhisperError};

pub struct FakeWhisper {
    pub reply: Result<String, WhisperError>,
    /// (bytes received, file name) per call.
    pub calls: Mutex<Vec<(usize, String)>>,
}

impl FakeWhisper {
    pub fn new(reply: Result<String, WhisperError>) -> FakeWhisper {
        FakeWhisper { reply, calls: Mutex::new(vec![]) }
    }
}

#[async_trait]
impl Transcriber for FakeWhisper {
    async fn check_ready(&self) -> Result<(), WhisperError> {
        match &self.reply {
            Err(WhisperError::MissingKey) => Err(WhisperError::MissingKey),
            _ => Ok(()),
        }
    }

    async fn transcribe(&self, audio: Vec<u8>, file_name: &str) -> Result<String, WhisperError> {
        self.calls.lock().unwrap().push((audio.len(), file_name.to_string()));
        self.reply.clone()
    }
}
