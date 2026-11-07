//! `POST https://api.openai.com/v1/audio/transcriptions`, multipart with
//! `file`, `model=whisper-1` and `response_format=json`; the reply is `{"text": …}`.
//! Request shape per developers.openai.com/api/reference (audio transcriptions create).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::{MAX_AUDIO_BYTES, Transcriber, WhisperError};
use crate::secrets::{ACCOUNT_OPENAI_KEY, SecretStore};

pub const OPENAI_BASE: &str = "https://api.openai.com";
const MODEL: &str = "whisper-1";

pub struct OpenAiWhisper {
    http: reqwest::Client,
    base: String,
    secrets: Arc<dyn SecretStore>,
}

#[derive(Deserialize)]
struct TranscriptionReply {
    text: String,
}

#[derive(Deserialize)]
struct ErrorReply {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

impl OpenAiWhisper {
    /// `base` is `OPENAI_BASE` outside tests. The key is read from the keyring
    /// on every call, so `insta set-openai-key` works without a restart.
    pub fn new(base: &str, secrets: Arc<dyn SecretStore>) -> Result<OpenAiWhisper, WhisperError> {
        let http = reqwest::Client::builder()
            // Long voice notes take a while to transcribe.
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| WhisperError::Http(e.to_string()))?;
        Ok(OpenAiWhisper { http, base: base.trim_end_matches('/').to_string(), secrets })
    }

    async fn key(&self) -> Result<String, WhisperError> {
        match self.secrets.get(ACCOUNT_OPENAI_KEY).await {
            Ok(Some(k)) if !k.is_empty() => Ok(k),
            Ok(_) => Err(WhisperError::MissingKey),
            Err(e) => Err(WhisperError::Api(format!("reading the key from the keyring failed: {e}"))),
        }
    }
}

fn form(audio: Vec<u8>, file_name: &str) -> Result<Form, WhisperError> {
    let part = Part::bytes(audio)
        .file_name(file_name.to_string())
        .mime_str("application/octet-stream")
        .map_err(|e| WhisperError::Http(e.to_string()))?;
    Ok(Form::new().part("file", part).text("model", MODEL).text("response_format", "json"))
}

/// OpenAI's own error message (about the request, never the audio), or the status.
fn api_error(status: reqwest::StatusCode, body: &str) -> WhisperError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return WhisperError::BadKey;
    }
    match serde_json::from_str::<ErrorReply>(body) {
        Ok(e) => WhisperError::Api(format!("HTTP {status}: {}", e.error.message)),
        Err(_) => WhisperError::Api(format!("HTTP {status}")),
    }
}

#[async_trait]
impl Transcriber for OpenAiWhisper {
    async fn check_ready(&self) -> Result<(), WhisperError> {
        self.key().await.map(|_| ())
    }

    async fn transcribe(&self, audio: Vec<u8>, file_name: &str) -> Result<String, WhisperError> {
        if audio.len() > MAX_AUDIO_BYTES {
            return Err(WhisperError::TooLarge(audio.len()));
        }
        let key = self.key().await?;
        let resp = self
            .http
            .post(format!("{}/v1/audio/transcriptions", self.base))
            .bearer_auth(key)
            .multipart(form(audio, file_name)?)
            .send()
            .await
            .map_err(|e| WhisperError::Http(e.without_url().to_string()))?;
        let status = resp.status();
        tracing::debug!(status = status.as_u16(), "openai transcription");
        let body = resp.text().await.map_err(|e| WhisperError::Http(e.without_url().to_string()))?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        let reply: TranscriptionReply =
            serde_json::from_str(&body).map_err(|e| WhisperError::Api(format!("unexpected reply: {e}")))?;
        Ok(reply.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::tests_support::MemSecrets;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn whisper(server: &MockServer, key: Option<&str>) -> OpenAiWhisper {
        let secrets = Arc::new(MemSecrets::default());
        if let Some(k) = key {
            secrets.set(ACCOUNT_OPENAI_KEY, k).await.unwrap();
        }
        OpenAiWhisper::new(&server.uri(), secrets).unwrap()
    }

    #[tokio::test]
    async fn posts_multipart_with_model_and_returns_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(header("authorization", "Bearer sk-test"))
            .and(body_string_contains("name=\"model\""))
            .and(body_string_contains("whisper-1"))
            .and(body_string_contains("name=\"response_format\""))
            .and(body_string_contains("filename=\"voice.m4a\""))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"text":"fake transcript"}"#))
            .expect(1)
            .mount(&server)
            .await;
        let w = whisper(&server, Some("sk-test")).await;
        assert_eq!(w.transcribe(b"audio".to_vec(), "voice.m4a").await.unwrap(), "fake transcript");
    }

    #[tokio::test]
    async fn missing_key_says_how_to_set_it_and_makes_no_call() {
        let server = MockServer::start().await;
        let w = whisper(&server, None).await;
        let e = w.transcribe(b"a".to_vec(), "voice.mp4").await.unwrap_err();
        assert_eq!(e, WhisperError::MissingKey);
        assert!(e.to_string().contains("insta set-openai-key"));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn too_large_audio_is_refused_before_upload() {
        let server = MockServer::start().await;
        let w = whisper(&server, Some("sk-test")).await;
        let e = w.transcribe(vec![0; MAX_AUDIO_BYTES + 1], "voice.mp4").await.unwrap_err();
        assert!(matches!(e, WhisperError::TooLarge(_)));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn errors_map_to_bad_key_and_api_message() {
        let server = MockServer::start().await;
        Mock::given(header("authorization", "Bearer sk-bad"))
            .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":{"message":"bad key"}}"#))
            .mount(&server)
            .await;
        Mock::given(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"error":{"message":"Invalid file format."}}"#))
            .mount(&server)
            .await;
        let bad = whisper(&server, Some("sk-bad")).await;
        assert_eq!(bad.transcribe(b"a".to_vec(), "voice.mp4").await.unwrap_err(), WhisperError::BadKey);
        let w = whisper(&server, Some("sk-test")).await;
        let e = w.transcribe(b"a".to_vec(), "voice.mp4").await.unwrap_err().to_string();
        assert!(e.contains("Invalid file format."), "{e}");
        assert!(!e.contains("sk-test"));
    }
}
