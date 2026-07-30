//! Conversation events — the shared vocabulary between a voice engine and the
//! `/ws/voice/conversation` WebSocket handler.
//!
//! Conversation used to be a local STT → LLM → TTS cascade orchestrated here;
//! the serialized three-hop architecture was structurally too slow for live
//! speech and was removed. Speech-to-speech now runs through the xAI realtime
//! client (`crate::realtime`), which emits these same events — the downstream
//! wire protocol never changed. Dictation is handled natively by the OS
//! (macOS dictation, Win+H) typing straight into the composer; no local
//! whisper pathway remains.

use bytes::Bytes;

/// Events emitted by a voice conversation engine to the client.
#[derive(Debug, Clone)]
pub enum ConversationEvent {
    /// Session is ready.
    SessionInitialized,
    /// User started speaking (transcript beginning).
    TranscriptionStart,
    /// User transcript text. From the realtime engine this is CUMULATIVE
    /// (includes upstream corrections) — consumers replace, never append.
    TranscriptionText(String),
    /// User utterance complete.
    TranscriptionEnd,
    /// Model audio playback is starting.
    PlaybackStart,
    /// A chunk of model audio (PCM Int16 LE, mono, 24kHz).
    AudioChunk(Bytes),
    /// Model audio playback is complete.
    PlaybackEnd,
    /// Model's text response delta (for display in the UI).
    ResponseText(String),
    /// The model wants a client-side tool executed. The handler runs it
    /// through the tools registry (policy engine, origin tagging) and feeds
    /// the result back — every parallel call's output must be submitted
    /// before a single response continuation.
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// The upstream conversation id (xAI resumption) — reconnect with it to
    /// resume history within its 30-minute expiry window.
    ConversationId(String),
    /// An error occurred.
    Error(String),
}
