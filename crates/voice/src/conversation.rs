//! Full-duplex voice conversation orchestrator.
//!
//! Manages the listen → transcribe → agent → TTS → playback loop:
//! 1. Receives PCM Int16 audio from the client
//! 2. Uses [`StreamingTranscriber`] to detect when the user finishes speaking
//! 3. Collects the full user utterance
//! 4. Sends it to the agent via a callback channel
//! 5. Receives the agent's text response
//! 6. Runs TTS via [`VoicePipeline::synthesize`]
//! 7. Streams TTS audio back as PCM Int16 chunks
//! 8. Signals playback end
//! 9. Returns to listening
//!
//! Supports barge-in: if the user speaks during playback, TTS stops and
//! the orchestrator immediately returns to listening mode.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::streaming::{StreamingConfig, StreamingTranscriber, TranscriptEvent};
use crate::VoicePipeline;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a voice conversation session.
#[derive(Debug, Clone)]
pub struct ConversationConfig {
    /// Streaming transcriber config (silence threshold, endpointing, etc.).
    pub streaming: StreamingConfig,
    /// Kokoro TTS voice/style name (e.g. "af_heart", "af_sky").
    pub voice: String,
    /// TTS speech speed multiplier.
    pub speed: f32,
    /// Size of audio chunks sent to client (in samples). Default: ~100ms at 24kHz.
    pub playback_chunk_samples: usize,
    /// Timeout waiting for agent response before giving up.
    pub agent_timeout: Duration,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            streaming: StreamingConfig::default(),
            voice: "af_heart".into(),
            speed: 1.0,
            playback_chunk_samples: 2400, // ~100ms at 24kHz
            agent_timeout: Duration::from_secs(30),
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events emitted by the conversation orchestrator to the client.
#[derive(Debug, Clone)]
pub enum ConversationEvent {
    /// Session is ready.
    SessionInitialized,
    /// User started speaking (transcript beginning).
    TranscriptionStart,
    /// Interim/partial transcript text.
    TranscriptionText(String),
    /// User utterance complete.
    TranscriptionEnd,
    /// TTS audio playback is starting.
    PlaybackStart,
    /// A chunk of TTS audio (PCM Int16, mono).
    AudioChunk(Bytes),
    /// TTS audio playback is complete.
    PlaybackEnd,
    /// Agent's text response (for display in the UI).
    ResponseText(String),
    /// An error occurred.
    Error(String),
}

/// Commands sent from the WebSocket handler to the orchestrator.
#[derive(Debug)]
pub enum ConversationCommand {
    /// Raw PCM Int16 audio from the client.
    Audio(Vec<i16>),
    /// User explicitly ended their input (push-to-talk release).
    ManualInputEnd,
    /// User interrupted (barge-in during playback).
    Interrupt,
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Conversation state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Listening for user speech.
    Listening,
    /// Waiting for the agent to respond.
    Processing,
    /// Streaming TTS audio to the client.
    Playing,
}

/// Request sent to the agent integration layer.
///
/// The handler streams text chunks back via `response_tx` as they arrive from
/// the LLM. Drop the sender when the response is complete.
pub struct AgentRequest {
    pub text: String,
    pub response_tx: mpsc::Sender<String>,
}

/// Manages one voice conversation session.
///
/// Create via [`ConversationOrchestrator::start`] which returns channels for
/// feeding commands and receiving events. The agent integration is handled
/// via a separate `mpsc::Sender<AgentRequest>` that the caller provides.
pub struct ConversationOrchestrator {
    config: ConversationConfig,
    pipeline: Arc<VoicePipeline>,
    agent_tx: mpsc::Sender<AgentRequest>,
}

impl ConversationOrchestrator {
    pub fn new(
        config: ConversationConfig,
        pipeline: Arc<VoicePipeline>,
        agent_tx: mpsc::Sender<AgentRequest>,
    ) -> Self {
        Self {
            config,
            pipeline,
            agent_tx,
        }
    }

    /// Start the conversation loop. Returns channels for commands (in) and events (out).
    ///
    /// - Send [`ConversationCommand`]s to drive the session.
    /// - Receive [`ConversationEvent`]s for the client.
    /// - Drop the command sender to end the session.
    pub fn start(
        self,
    ) -> (
        mpsc::Sender<ConversationCommand>,
        mpsc::Receiver<ConversationEvent>,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<ConversationCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<ConversationEvent>(64);

        tokio::spawn(self.run_loop(cmd_rx, event_tx));

        (cmd_tx, event_rx)
    }

    async fn run_loop(
        self,
        mut cmd_rx: mpsc::Receiver<ConversationCommand>,
        event_tx: mpsc::Sender<ConversationEvent>,
    ) {
        // Initialize whisper context
        let ctx = match self.pipeline.whisper_context().await {
            Ok(ctx) => ctx,
            Err(e) => {
                let _ = event_tx
                    .send(ConversationEvent::Error(format!(
                        "failed to load whisper: {e}"
                    )))
                    .await;
                return;
            }
        };

        let _ = event_tx
            .send(ConversationEvent::SessionInitialized)
            .await;

        let mut phase = Phase::Listening;
        let mut turn: u32 = 0;
        let mut utterance = String::new();
        let mut heard_speech = false;

        // Create the streaming transcriber
        let transcriber =
            StreamingTranscriber::new(ctx.clone(), self.config.streaming.clone());
        let (audio_tx, mut transcript_rx) = transcriber.start();

        tracing::info!("conversation orchestrator started");

        loop {
            tokio::select! {
                // Transcript events from the streaming transcriber
                event = transcript_rx.recv() => {
                    match event {
                        Some(TranscriptEvent::Interim(text)) if phase == Phase::Listening => {
                            if !heard_speech {
                                heard_speech = true;
                                let _ = event_tx.send(ConversationEvent::TranscriptionStart).await;
                            }
                            let _ = event_tx.send(ConversationEvent::TranscriptionText(text)).await;
                        }
                        Some(TranscriptEvent::Text(text)) if phase == Phase::Listening => {
                            if !heard_speech {
                                heard_speech = true;
                                let _ = event_tx.send(ConversationEvent::TranscriptionStart).await;
                            }
                            if !text.is_empty() {
                                if !utterance.is_empty() {
                                    utterance.push(' ');
                                }
                                utterance.push_str(&text);
                            }
                            let _ = event_tx.send(ConversationEvent::TranscriptionText(text)).await;
                        }
                        Some(TranscriptEvent::Endpoint) if phase == Phase::Listening => {
                            if !utterance.trim().is_empty() {
                                // User finished speaking — process the utterance
                                let _ = event_tx.send(ConversationEvent::TranscriptionEnd).await;
                                phase = Phase::Processing;
                                turn += 1;
                                tracing::info!(turn, utterance = %utterance, "user utterance complete");

                                self.process_and_respond(
                                    &utterance, &event_tx, &mut phase,
                                ).await;

                                // Reset for next turn
                                utterance.clear();
                                heard_speech = false;
                                phase = Phase::Listening;
                            }
                        }
                        Some(TranscriptEvent::Error(msg)) => {
                            tracing::error!(error = %msg, "transcription error");
                            let _ = event_tx.send(ConversationEvent::Error(msg)).await;
                        }
                        None => {
                            // Transcriber channel closed
                            tracing::info!("transcriber channel closed, ending conversation");
                            break;
                        }
                        _ => {
                            // Events during non-listening phases are ignored
                        }
                    }
                }

                // Commands from the WebSocket handler
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(ConversationCommand::Audio(samples)) => {
                            // Always feed audio to the transcriber
                            if audio_tx.send(samples).await.is_err() {
                                tracing::warn!("audio channel closed — transcriber died");
                                break;
                            }
                        }
                        Some(ConversationCommand::ManualInputEnd) => {
                            if phase == Phase::Listening && !utterance.trim().is_empty() {
                                let _ = event_tx.send(ConversationEvent::TranscriptionEnd).await;
                                phase = Phase::Processing;
                                turn += 1;
                                tracing::info!(turn, utterance = %utterance, "manual input end");

                                self.process_and_respond(
                                    &utterance, &event_tx, &mut phase,
                                ).await;

                                utterance.clear();
                                heard_speech = false;
                                phase = Phase::Listening;
                            }
                        }
                        Some(ConversationCommand::Interrupt) => {
                            if phase == Phase::Playing {
                                tracing::info!("barge-in interrupt — stopping playback");
                                // Signal playback end and return to listening
                                let _ = event_tx.send(ConversationEvent::PlaybackEnd).await;
                                phase = Phase::Listening;
                                heard_speech = false;
                            }
                        }
                        None => {
                            // Command channel closed — session ended
                            tracing::info!("command channel closed, ending conversation");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!(turns = turn, "conversation orchestrator ended");
    }

    /// Send utterance to agent, stream TTS at sentence boundaries.
    ///
    /// Instead of waiting for the full response, we buffer text as it streams
    /// from the LLM and synthesize+play each sentence as soon as it's complete.
    /// This dramatically reduces time-to-first-audio.
    async fn process_and_respond(
        &self,
        utterance: &str,
        event_tx: &mpsc::Sender<ConversationEvent>,
        phase: &mut Phase,
    ) {
        // Create a streaming channel for agent text chunks
        let (resp_tx, mut resp_rx) = mpsc::channel::<String>(32);
        let request = AgentRequest {
            text: utterance.to_string(),
            response_tx: resp_tx,
        };

        if self.agent_tx.send(request).await.is_err() {
            tracing::error!("agent channel closed");
            let _ = event_tx
                .send(ConversationEvent::Error("agent unavailable".into()))
                .await;
            return;
        }

        // Buffer text and synthesize at sentence boundaries
        let mut buffer = String::new();
        let mut full_response = String::new();
        let mut playback_started = false;
        let mut timed_out = false;

        let timeout = tokio::time::sleep(self.config.agent_timeout);
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                chunk = resp_rx.recv() => {
                    match chunk {
                        Some(text) => {
                            buffer.push_str(&text);
                            full_response.push_str(&text);

                            // Check for sentence boundaries and synthesize each one
                            while let Some((sentence, rest)) = split_at_sentence(&buffer) {
                                if !sentence.trim().is_empty() {
                                    if !playback_started {
                                        *phase = Phase::Playing;
                                        let _ = event_tx.send(ConversationEvent::PlaybackStart).await;
                                        playback_started = true;
                                    }

                                    if *phase != Phase::Playing {
                                        break; // Interrupted
                                    }

                                    self.synthesize_and_stream(&sentence, event_tx, phase).await;
                                }
                                buffer = rest;
                            }
                        }
                        None => {
                            // Agent response stream ended — synthesize any remaining text
                            break;
                        }
                    }
                }
                _ = &mut timeout => {
                    tracing::error!("agent response timed out");
                    timed_out = true;
                    break;
                }
            }
        }

        if timed_out {
            let _ = event_tx
                .send(ConversationEvent::Error("agent response timed out".into()))
                .await;
            return;
        }

        // Synthesize any remaining buffered text
        if !buffer.trim().is_empty() && *phase != Phase::Playing || !playback_started {
            if !playback_started {
                *phase = Phase::Playing;
                let _ = event_tx.send(ConversationEvent::PlaybackStart).await;
                playback_started = true;
            }

            if *phase == Phase::Playing {
                self.synthesize_and_stream(&buffer, event_tx, phase).await;
            }
        }

        // Send the full text response for UI display
        if !full_response.trim().is_empty() {
            let _ = event_tx
                .send(ConversationEvent::ResponseText(full_response))
                .await;
        }

        if playback_started && *phase == Phase::Playing {
            let _ = event_tx.send(ConversationEvent::PlaybackEnd).await;
        }
    }

    /// Synthesize a text segment and stream the PCM audio chunks.
    async fn synthesize_and_stream(
        &self,
        text: &str,
        event_tx: &mpsc::Sender<ConversationEvent>,
        phase: &mut Phase,
    ) {
        let tts_req = crate::TtsRequest {
            text: text.to_string(),
            voice: self.config.voice.clone(),
            speed: self.config.speed,
        };

        let wav = match self.pipeline.synthesize(tts_req).await {
            Ok(wav) => wav,
            Err(e) => {
                tracing::error!(error = %e, text = %text, "TTS synthesis failed for segment");
                return;
            }
        };

        // Skip WAV header (44 bytes), send raw PCM Int16
        let pcm = if wav.len() > 44 { &wav[44..] } else { &wav[..] };
        let chunk_bytes = self.config.playback_chunk_samples * 2;

        for chunk in pcm.chunks(chunk_bytes.max(1)) {
            if *phase != Phase::Playing {
                break; // Interrupted
            }
            let _ = event_tx
                .send(ConversationEvent::AudioChunk(Bytes::copy_from_slice(chunk)))
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Split text into a synthesis-ready chunk, returning (chunk, remainder).
///
/// Returns `None` if there's not enough text to synthesize yet.
///
/// Strategy: buffer until we have a natural pause point with enough context
/// for good prosody. We wait for either:
/// - A paragraph break (\n\n)
/// - At least 2 complete sentences (better prosody than single sentences)
/// - A single sentence if it's already long (>120 chars)
///
/// This balances latency (don't wait too long) with quality (enough context
/// for natural intonation).
fn split_at_sentence(text: &str) -> Option<(String, String)> {
    let bytes = text.as_bytes();

    // Always split on paragraph breaks
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            let chunk = text[..i].trim().to_string();
            let rest = text[i + 2..].trim_start().to_string();
            if !chunk.is_empty() {
                return Some((chunk, rest));
            }
        }
    }

    // Find sentence boundaries
    let mut sentence_ends: Vec<usize> = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'.' || b == b'!' || b == b'?' {
            let next = bytes.get(i + 1);
            if next.is_none() || next == Some(&b' ') || next == Some(&b'\n') {
                sentence_ends.push(i + 1);
            }
        }
    }

    // If we have 2+ complete sentences, split after the second one
    if sentence_ends.len() >= 2 {
        let split = sentence_ends[1];
        let chunk = text[..split].trim().to_string();
        let rest = text[split..].trim_start().to_string();
        return Some((chunk, rest));
    }

    // If we have 1 long sentence (>120 chars), go ahead and synthesize it
    if let Some(&end) = sentence_ends.first() {
        if end > 120 {
            let chunk = text[..end].trim().to_string();
            let rest = text[end..].trim_start().to_string();
            return Some((chunk, rest));
        }
    }

    None
}
