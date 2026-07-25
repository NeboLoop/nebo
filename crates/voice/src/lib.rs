//! Voice — speech-to-speech conversation via the xAI Grok realtime API.
//!
//! The local pipeline (whisper STT cascade + Kokoro TTS) was removed:
//! conversation was structurally too slow as a serialized STT → LLM → TTS
//! cascade, and dictation is covered natively — and on-device — by the OS
//! (macOS dictation, Win+H) typing straight into the composer. A local
//! whisper pathway was a worse competing implementation of both.
//!
//! What remains is the realtime protocol client (`realtime`) and the shared
//! event vocabulary (`conversation`) consumed by the server's
//! `/ws/voice/conversation` relay.

pub mod conversation;
pub mod realtime;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("realtime voice error: {0}")]
    Realtime(String),
}
