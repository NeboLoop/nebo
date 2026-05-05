//! Rolling-window streaming transcription.
//!
//! Accumulates PCM Int16 audio chunks (16kHz mono) into a ring buffer.
//! Periodically runs whisper-rs on the accumulated audio and diffs the
//! transcript to emit interim and final results.
//!
//! This simulates streaming STT from a batch inference engine:
//! - Audio arrives continuously from the client
//! - Every ~1 second, whisper runs on the accumulated buffer
//! - Transcript changes are diffed and emitted as interim/final events
//! - On silence detection (energy below threshold for 300ms), an endpoint is emitted
//! - Processed audio is trimmed from the buffer (with ~1s overlap for context)

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext};

/// Events emitted by the streaming transcriber.
#[derive(Debug, Clone)]
pub enum TranscriptEvent {
    /// Partial/changing transcript (may be revised by next run).
    Interim(String),
    /// Confirmed final transcript segment.
    Text(String),
    /// Utterance boundary — silence detected, segment is complete.
    Endpoint,
    /// Error during transcription.
    Error(String),
}

/// Configuration for the streaming transcriber.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// How often to run whisper inference on accumulated audio (default: 750ms).
    pub inference_interval: Duration,
    /// Silence energy threshold (RMS below this = silence). Range 0.0–1.0.
    pub silence_threshold: f32,
    /// Duration of silence before emitting an endpoint (default: 300ms).
    pub endpointing_ms: u64,
    /// Hard utterance boundary after this much silence (default: 1000ms).
    pub utterance_end_ms: u64,
    /// Maximum audio buffer size in samples (default: 30s at 16kHz = 480,000).
    pub max_buffer_samples: usize,
    /// Overlap kept when trimming processed audio (default: 1s = 16,000 samples).
    pub overlap_samples: usize,
    /// Whisper language code (e.g., "en", "auto").
    pub language: String,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            inference_interval: Duration::from_millis(750),
            silence_threshold: 0.01,
            endpointing_ms: 300,
            utterance_end_ms: 1000,
            max_buffer_samples: 16_000 * 30, // 30 seconds
            overlap_samples: 16_000,          // 1 second
            language: "en".into(),
        }
    }
}

/// A streaming transcriber that processes audio chunks and emits transcript events.
///
/// Created per-session. Feed it PCM Int16 samples (16kHz mono) via the audio channel.
/// It runs whisper periodically and emits events via the output channel.
pub struct StreamingTranscriber {
    config: StreamingConfig,
    ctx: Arc<WhisperContext>,
}

impl StreamingTranscriber {
    pub fn new(ctx: Arc<WhisperContext>, config: StreamingConfig) -> Self {
        Self { config, ctx }
    }

    /// Start the transcription loop. Returns channels for feeding audio and receiving events.
    ///
    /// - Send PCM Int16 audio chunks (16kHz mono) to the audio_tx channel.
    /// - Receive `TranscriptEvent`s from the event_rx channel.
    /// - Drop the audio_tx to signal end-of-stream.
    pub fn start(self) -> (mpsc::Sender<Vec<i16>>, mpsc::Receiver<TranscriptEvent>) {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<i16>>(64);
        let (event_tx, event_rx) = mpsc::channel::<TranscriptEvent>(64);

        tokio::spawn(self.run_loop(audio_rx, event_tx));

        (audio_tx, event_rx)
    }

    async fn run_loop(
        self,
        mut audio_rx: mpsc::Receiver<Vec<i16>>,
        event_tx: mpsc::Sender<TranscriptEvent>,
    ) {
        let mut buffer: Vec<f32> = Vec::with_capacity(self.config.max_buffer_samples);
        let mut last_inference = Instant::now();
        let mut last_transcript = String::new();
        let mut confirmed_text = String::new();
        let mut silence_start: Option<Instant> = None;
        let mut endpoint_emitted = false;

        loop {
            // Wait for audio with a timeout to allow periodic inference
            let chunk = tokio::time::timeout(
                Duration::from_millis(100),
                audio_rx.recv(),
            )
            .await;

            match chunk {
                Ok(Some(pcm_i16)) => {
                    // Convert Int16 → Float32 and append to buffer
                    let float_samples: Vec<f32> =
                        pcm_i16.iter().map(|&s| s as f32 / 32768.0).collect();

                    // Energy detection for silence tracking
                    let energy = rms_energy(&float_samples);
                    if energy < self.config.silence_threshold {
                        if silence_start.is_none() {
                            silence_start = Some(Instant::now());
                        }
                    } else {
                        silence_start = None;
                        endpoint_emitted = false;
                    }

                    buffer.extend_from_slice(&float_samples);

                    // Cap buffer at max size — trim from front
                    if buffer.len() > self.config.max_buffer_samples {
                        let trim = buffer.len() - self.config.max_buffer_samples;
                        buffer.drain(..trim);
                    }
                }
                Ok(None) => {
                    // Channel closed — run final inference and exit
                    if !buffer.is_empty() {
                        if let Some(text) = self.run_inference(&buffer, &self.config.language).await
                        {
                            let new_text = diff_transcript(&confirmed_text, &text);
                            if !new_text.is_empty() {
                                let _ = event_tx.send(TranscriptEvent::Text(new_text)).await;
                            }
                        }
                        let _ = event_tx.send(TranscriptEvent::Endpoint).await;
                    }
                    break;
                }
                Err(_) => {
                    // Timeout — no audio received in 100ms, continue to check timers
                }
            }

            // Check if it's time to run inference
            if last_inference.elapsed() >= self.config.inference_interval && !buffer.is_empty() {
                last_inference = Instant::now();

                if let Some(text) = self.run_inference(&buffer, &self.config.language).await {
                    if text != last_transcript {
                        // Determine what's new beyond the confirmed portion
                        let new_portion = diff_transcript(&confirmed_text, &text);
                        if !new_portion.is_empty() {
                            let _ = event_tx.send(TranscriptEvent::Interim(new_portion.clone())).await;
                        }
                        last_transcript = text;
                    }
                }
            }

            // Check silence for endpointing
            if let Some(start) = silence_start {
                let silence_ms = start.elapsed().as_millis() as u64;

                if silence_ms >= self.config.endpointing_ms && !endpoint_emitted {
                    // Promote interim to final
                    let new_text = diff_transcript(&confirmed_text, &last_transcript);
                    if !new_text.is_empty() {
                        let _ = event_tx.send(TranscriptEvent::Text(new_text.clone())).await;
                    }
                    let _ = event_tx.send(TranscriptEvent::Endpoint).await;
                    endpoint_emitted = true;

                    // Clear buffer and reset text state for next utterance.
                    // No overlap needed across utterance boundaries — the user
                    // stopped speaking, so there's no continuity to preserve.
                    buffer.clear();
                    confirmed_text.clear();
                    last_transcript.clear();
                }
            }
        }
    }

    /// Run whisper inference on a blocking thread. Returns the full transcript or None on error.
    async fn run_inference(&self, samples: &[f32], language: &str) -> Option<String> {
        let ctx = self.ctx.clone();
        let samples = samples.to_vec();
        let language = language.to_string();

        tokio::task::spawn_blocking(move || {
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_special(false);
            params.set_print_timestamps(false);
            params.set_no_context(true); // Each window is independent
            params.set_suppress_nst(true);

            if language != "auto" && !language.is_empty() {
                params.set_language(Some(&language));
            }

            let mut state = match ctx.create_state() {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "failed to create whisper state");
                    return None;
                }
            };

            if let Err(e) = state.full(params, &samples) {
                tracing::error!(error = %e, "whisper inference failed");
                return None;
            }

            let n_segments = state.full_n_segments();
            let mut text = String::new();
            for i in 0..n_segments {
                if let Some(seg) = state.get_segment(i) {
                    if let Ok(s) = seg.to_str_lossy() {
                        text.push_str(&s);
                    }
                }
                if !text.ends_with(' ') {
                    text.push(' ');
                }
            }

            let trimmed = clean_transcript(text.trim());
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
        .await
        .ok()
        .flatten()
    }
}

/// Calculate RMS energy of audio samples (0.0–1.0 range).
fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Given previous confirmed text and a new full transcript, return the new portion.
fn diff_transcript(confirmed: &str, full: &str) -> String {
    if full.starts_with(confirmed) {
        full[confirmed.len()..].trim().to_string()
    } else {
        // Transcript diverged — return full text (whisper revised its output)
        full.trim().to_string()
    }
}

/// Strip whisper hallucination artifacts and noise tokens from transcript text.
pub fn clean_transcript(text: &str) -> String {
    // Remove bracketed artifacts: [BLANK_AUDIO], [MUSIC], [APPLAUSE], etc.
    let mut result = text.to_string();
    for artifact in &[
        "[BLANK_AUDIO]", "[MUSIC]", "[APPLAUSE]", "[LAUGHTER]",
        "[NOISE]", "[SILENCE]", "[INAUDIBLE]", "[SOUND]",
        "(clicks)", "(clicking)", "(buzzing)", "(static)",
        "(background noise)", "(laughing)", "(sighing)", "(coughing)",
    ] {
        result = result.replace(artifact, "");
    }
    // Collapse multiple spaces and trim
    let result: String = result.split_whitespace().collect::<Vec<_>>().join(" ");
    result.trim().to_string()
}
