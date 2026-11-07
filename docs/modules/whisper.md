# whisper

Last verified: 2026-11-07

## Purpose
Voice-note transcription with OpenAI. Audio stays in memory; nothing is written to disk and neither audio, transcript nor key is logged.

## Structure
- `src/whisper/mod.rs`: `Transcriber` trait (`src/whisper/mod.rs:32`) with `check_ready` (default Ok) and `transcribe`; `WhisperError` (`:18`); `MAX_AUDIO_BYTES = 25 MiB` (`:15`); `audio_file_name` (`:44`).
- `src/whisper/openai.rs`: `OpenAiWhisper`.
- `src/whisper/fake.rs` (`#[cfg(test)]`): `FakeWhisper` (canned reply, records byte count + file name).

## Endpoint
`POST {base}/v1/audio/transcriptions` with `Authorization: Bearer <key>` and multipart form fields `file`, `model=whisper-1`, `response_format=json`; reply `{"text": "..."}`. Files up to 25 MB; formats mp3, mp4, mpeg, mpga, m4a, wav, webm. Verified 2026-11-07 against OpenAI's API reference (developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create) and the speech-to-text guide (developers.openai.com/api/docs/guides/speech-to-text), via Context7.

## Key methods
- `OpenAiWhisper::new` (`src/whisper/openai.rs:42`): takes the base URL (`OPENAI_BASE`, `:15`, overridable for tests) and the secret store. 120 s timeout.
- `check_ready` (`src/whisper/openai.rs:81`): fails with `MissingKey` ("run `insta set-openai-key`") so no audio is downloaded without a key.
- `transcribe` (`src/whisper/openai.rs:85`): refuses >25 MiB before uploading, reads the key from the keyring on every call (no restart after `set-openai-key`), maps 401 to `BadKey` and other errors to OpenAI's own `error.message`.
- `audio_file_name` (`src/whisper/mod.rs:44`): extension from magic bytes (`ftyp` → m4a, OggS, RIFF, EBML/webm, ID3/MPEG frame → mp3, else mp4), since OpenAI uses the extension to pick the decoder.

## Dependencies
reqwest `multipart` feature, serde_json, secrets.

## Gotchas & invariants
- What container Instagram voice notes use is not verified yet; the magic-byte sniffing handles the common ones. `ogg` isn't in OpenAI's documented list.
- Transcription costs money: the MCP instructions say to call it only when the user wants.

## Testing
`cargo test whisper`: wiremock checks path, bearer header and multipart fields; missing key and oversized audio make no request; 401/400 mapping never leaks the key.
