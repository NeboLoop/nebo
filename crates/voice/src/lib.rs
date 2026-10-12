//! Voice — speech-to-speech conversation via the xAI Grok realtime API, and
//! dictation through NeboAI's streaming transcription.
//!
//! The local pipeline (whisper STT cascade + Kokoro TTS) was removed:
//! conversation was structurally too slow as a serialized STT → LLM → TTS
//! cascade. Desktop dictation is the OS's own (macOS dictation, Win+H)
//! typing straight into the composer; the phone app dictates through the
//! bot (`dictation`), which streams its microphone to the metered `/v1/stt`
//! relay and hands back live, punctuated text with no time limit per request.
//!
//! What remains is the realtime protocol client (`realtime`), the shared
//! event vocabulary (`conversation`) consumed by the server's
//! `/ws/voice/conversation` relay, and the dictation relay (`dictation`)
//! behind `/ws/voice/dictation`.

pub mod conversation;
pub mod dictation;
pub mod realtime;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("realtime voice error: {0}")]
    Realtime(String),
}
