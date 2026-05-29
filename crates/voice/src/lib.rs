//! Voice pipeline — local TTS (Kokoro) and STT (whisper.cpp via whisper-rs).
//!
//! TTS uses kokorox (Kokoro-82M via ONNX Runtime + espeak-ng phonemizer).
//! STT uses whisper-rs (Rust bindings to whisper.cpp) directly — no subprocess.
//! TTS models auto-download from HuggingFace on first use.

pub mod conversation;
pub mod streaming;

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
#[cfg(feature = "kokoro-tts")]
use kokorox::tts::koko::{ModelVariant, TTSKoko};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("encoding error: {0}")]
    Encoding(String),

    #[error("channel send failed")]
    ChannelClosed,

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("whisper error: {0}")]
    Whisper(String),

    #[error("text-to-speech is not available in this build")]
    TtsUnavailable,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_voice() -> String {
    "af_heart".into()
}

fn default_speed() -> f32 {
    1.0
}

#[derive(Debug, Clone)]
pub struct TtsChunk {
    pub sequence: u32,
    pub data: Bytes,
    pub sample_rate: u32,
    pub is_last: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscribeResult {
    pub text: String,
}

// ---------------------------------------------------------------------------
// WAV helpers
// ---------------------------------------------------------------------------

/// Generate a valid 44-byte WAV header for 16-bit mono PCM audio.
pub fn generate_wav_header(sample_rate: u32, num_samples: u32) -> Vec<u8> {
    let byte_rate = sample_rate * 2; // 16-bit mono = 2 bytes per sample
    let data_size = num_samples * 2;
    let file_size = 36 + data_size; // total - 8 bytes for RIFF header

    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&file_size.to_le_bytes());
    header.extend_from_slice(b"WAVE");

    // fmt sub-chunk
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes()); // sub-chunk size
    header.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    header.extend_from_slice(&1u16.to_le_bytes()); // mono
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes()); // block align
    header.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data sub-chunk
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_size.to_le_bytes());

    header
}

/// Parse a WAV file's raw bytes and return (sample_rate, mono f32 samples).
/// Supports 16-bit and 32-bit float PCM, mono and stereo (stereo → averaged to mono).
fn decode_wav_to_f32(wav_bytes: &[u8]) -> Result<(u32, Vec<f32>), VoiceError> {
    if wav_bytes.len() < 44 {
        return Err(VoiceError::Encoding("WAV data too short".into()));
    }

    // Parse header fields
    let channels = u16::from_le_bytes([wav_bytes[22], wav_bytes[23]]) as usize;
    let sample_rate =
        u32::from_le_bytes([wav_bytes[24], wav_bytes[25], wav_bytes[26], wav_bytes[27]]);
    let bits_per_sample = u16::from_le_bytes([wav_bytes[34], wav_bytes[35]]);

    // Find "data" sub-chunk
    let mut offset = 12; // skip RIFF header
    let data_start;
    let data_size;
    loop {
        if offset + 8 > wav_bytes.len() {
            return Err(VoiceError::Encoding("no data chunk found in WAV".into()));
        }
        let chunk_id = &wav_bytes[offset..offset + 4];
        let chunk_size = u32::from_le_bytes([
            wav_bytes[offset + 4],
            wav_bytes[offset + 5],
            wav_bytes[offset + 6],
            wav_bytes[offset + 7],
        ]) as usize;
        if chunk_id == b"data" {
            data_start = offset + 8;
            data_size = chunk_size.min(wav_bytes.len() - data_start);
            break;
        }
        offset += 8 + chunk_size;
    }

    let pcm = &wav_bytes[data_start..data_start + data_size];

    let interleaved: Vec<f32> = match bits_per_sample {
        16 => pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect(),
        32 => pcm
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        other => {
            return Err(VoiceError::Encoding(format!(
                "unsupported bits_per_sample: {other}"
            )));
        }
    };

    // Mix down to mono if stereo
    let mono = if channels == 1 {
        interleaved
    } else {
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    Ok((sample_rate, mono))
}

/// Resample mono f32 audio from `from_rate` to `to_rate` using linear interpolation.
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (samples.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = (src - idx as f64) as f32;
            let a = samples.get(idx).copied().unwrap_or(0.0);
            let b = samples.get(idx + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// Configuration for the voice pipeline.
#[derive(Debug, Clone)]
pub struct VoicePipelineConfig {
    /// Sample rate for TTS output audio (Kokoro outputs at 24kHz).
    pub sample_rate: u32,
    /// Directory for voice models. Defaults to `~/.nebo/models/voice`.
    pub models_dir: PathBuf,
    /// Default Kokoro voice/style name (e.g. "af_heart", "af_sky").
    pub tts_voice: String,
    /// Whisper model name (e.g. "ggml-tiny.en.bin", "ggml-base.en.bin").
    pub whisper_model: String,
    /// Language for whisper transcription (e.g. "en", "auto").
    pub whisper_language: String,
}

impl Default for VoicePipelineConfig {
    fn default() -> Self {
        let models_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".nebo/models/voice");
        Self {
            sample_rate: 24_000,
            models_dir,
            tts_voice: "af_heart".into(),
            whisper_model: "ggml-tiny.en.bin".into(),
            whisper_language: "en".into(),
        }
    }
}

/// Local voice pipeline using Kokoro TTS and whisper-rs (embedded whisper.cpp).
///
/// TTS uses kokorox (Kokoro-82M, ONNX Runtime + espeak-ng phonemizer).
/// Models auto-download from HuggingFace on first use (~460MB total).
/// Whisper model is loaded lazily on first transcription request.
/// Kokoro model is loaded lazily on first TTS request.
pub struct VoicePipeline {
    config: VoicePipelineConfig,
    /// Lazily-loaded whisper context (loaded on first transcribe call).
    whisper_ctx: tokio::sync::OnceCell<Arc<WhisperContext>>,
    /// Lazily-loaded Kokoro TTS instance (loaded on first synthesize call).
    #[cfg(feature = "kokoro-tts")]
    kokoro: tokio::sync::OnceCell<Arc<TTSKoko>>,
}

impl VoicePipeline {
    pub fn new(config: VoicePipelineConfig) -> Self {
        Self {
            config,
            whisper_ctx: tokio::sync::OnceCell::new(),
            #[cfg(feature = "kokoro-tts")]
            kokoro: tokio::sync::OnceCell::new(),
        }
    }

    /// Resolve the whisper model path.
    fn whisper_model_path(&self) -> PathBuf {
        self.config
            .models_dir
            .join("whisper")
            .join(&self.config.whisper_model)
    }

    /// Load or return the cached Kokoro TTS instance. The model auto-downloads
    /// from HuggingFace on first call and is reused for all subsequent requests.
    #[cfg(feature = "kokoro-tts")]
    async fn kokoro_instance(&self) -> Result<Arc<TTSKoko>, VoiceError> {
        self.kokoro
            .get_or_try_init(|| async {
                tracing::info!("loading kokoro TTS model (auto-downloads on first use)");
                let tts =
                    TTSKoko::new_with_variant(None, None, None, ModelVariant::V1English).await;
                let voices = tts.get_available_voices();
                tracing::info!(voices = voices.len(), "kokoro TTS loaded");
                Ok(Arc::new(tts))
            })
            .await
            .cloned()
    }

    /// Load or return the cached whisper context. The model is loaded on first call
    /// and reused for all subsequent transcriptions. Loading happens on a blocking
    /// thread to avoid stalling the async runtime.
    pub async fn whisper_context(&self) -> Result<Arc<WhisperContext>, VoiceError> {
        self.whisper_ctx
            .get_or_try_init(|| async {
                let model_path = self.whisper_model_path();
                if !model_path.exists() {
                    return Err(VoiceError::ModelNotFound(format!(
                        "{} — download from https://huggingface.co/ggerganov/whisper.cpp/tree/main to {}",
                        self.config.whisper_model,
                        self.config.models_dir.join("whisper").display()
                    )));
                }

                let path = model_path.to_string_lossy().to_string();
                let ctx = tokio::task::spawn_blocking(move || {
                    tracing::info!(model = %path, "loading whisper model");
                    let ctx = WhisperContext::new_with_params(
                        &path,
                        WhisperContextParameters::default(),
                    )
                    .map_err(|e| VoiceError::Whisper(format!("failed to load model: {e}")))?;

                    // Warmup: run inference on 0.5s of silence to force Metal shader
                    // compilation and memory allocation.
                    let silence = vec![0.0f32; 8000]; // 0.5s at 16kHz
                    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
                    params.set_print_progress(false);
                    params.set_print_realtime(false);
                    params.set_print_special(false);
                    params.set_print_timestamps(false);

                    let mut state = ctx
                        .create_state()
                        .map_err(|e| VoiceError::Whisper(format!("warmup state error: {e}")))?;
                    let _ = state.full(params, &silence);

                    tracing::info!("whisper model loaded and warmed up");
                    Ok::<_, VoiceError>(Arc::new(ctx))
                })
                .await
                .map_err(|e| VoiceError::Whisper(format!("spawn_blocking failed: {e}")))??;

                Ok(ctx)
            })
            .await
            .cloned()
    }

    // -----------------------------------------------------------------------
    // TTS (Kokoro via kokorox)
    // -----------------------------------------------------------------------

    /// Synthesize speech using Kokoro TTS (87M parameter neural model).
    /// Returns WAV bytes (44-byte header + PCM Int16 mono at 24kHz).
    ///
    /// When the `kokoro-tts` feature is disabled, returns
    /// [`VoiceError::TtsUnavailable`] so callers can fall back to system TTS.
    #[cfg(not(feature = "kokoro-tts"))]
    pub async fn synthesize(&self, _req: TtsRequest) -> Result<Vec<u8>, VoiceError> {
        Err(VoiceError::TtsUnavailable)
    }

    /// Synthesize speech using Kokoro TTS (87M parameter neural model).
    /// Returns WAV bytes (44-byte header + PCM Int16 mono at 24kHz).
    #[cfg(feature = "kokoro-tts")]
    pub async fn synthesize(&self, req: TtsRequest) -> Result<Vec<u8>, VoiceError> {
        let kokoro = self.kokoro_instance().await?;
        let text = req.text.clone();
        let voice = req.voice.clone();
        let speed = req.speed;

        let samples = tokio::task::spawn_blocking(move || {
            kokoro
                .tts_raw_audio(
                    &text,
                    "en-us",
                    &voice,
                    speed,
                    Some(0), // initial_silence
                    false,   // auto_detect_language
                    false,   // force_style
                    false,   // phonemes
                )
                .map_err(|e| VoiceError::Encoding(format!("kokoro synthesis failed: {e}")))
        })
        .await
        .map_err(|e| VoiceError::Encoding(format!("spawn_blocking failed: {e}")))??;

        // Kokoro outputs f32 samples at 24kHz — convert to Int16 PCM WAV
        let sample_rate = 24_000u32;
        let num_samples = samples.len() as u32;
        let mut wav = generate_wav_header(sample_rate, num_samples);
        for s in &samples {
            let clamped = s.clamp(-1.0, 1.0);
            let pcm_sample = if clamped < 0.0 {
                (clamped * 32768.0) as i16
            } else {
                (clamped * 32767.0) as i16
            };
            wav.extend_from_slice(&pcm_sample.to_le_bytes());
        }

        tracing::info!(
            samples = num_samples,
            sample_rate,
            bytes = wav.len(),
            "kokoro TTS synthesis complete"
        );
        Ok(wav)
    }

    /// Stream TTS audio as `TtsChunk` messages over a channel.
    pub async fn synthesize_stream(
        &self,
        req: TtsRequest,
        tx: mpsc::Sender<TtsChunk>,
    ) -> Result<(), VoiceError> {
        let wav = self.synthesize(req).await?;

        // Skip WAV header (44 bytes), stream PCM in ~100ms chunks
        let pcm = if wav.len() > 44 { &wav[44..] } else { &wav[..] };
        let chunk_bytes = (self.config.sample_rate as usize / 10) * 2; // 100ms of 16-bit mono
        let chunks: Vec<&[u8]> = pcm.chunks(chunk_bytes.max(1)).collect();
        let total = chunks.len() as u32;

        for (i, chunk) in chunks.iter().enumerate() {
            let tts_chunk = TtsChunk {
                sequence: i as u32,
                data: Bytes::copy_from_slice(chunk),
                sample_rate: self.config.sample_rate,
                is_last: i as u32 == total - 1,
            };
            tx.send(tts_chunk)
                .await
                .map_err(|_| VoiceError::ChannelClosed)?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // STT (embedded whisper-rs)
    // -----------------------------------------------------------------------

    /// Transcribe audio bytes to text using embedded whisper.cpp.
    ///
    /// Accepts raw audio in any format whisper understands (WAV preferred).
    /// The whisper model is loaded lazily on first call and reused.
    /// Inference runs on a blocking thread via `spawn_blocking`.
    pub async fn transcribe(&self, audio: &[u8]) -> Result<TranscribeResult, VoiceError> {
        let model_path = self.whisper_model_path();
        if !model_path.exists() {
            return Err(VoiceError::ModelNotFound(format!(
                "{} — download from https://huggingface.co/ggerganov/whisper.cpp/tree/main to {}",
                self.config.whisper_model,
                self.config.models_dir.join("whisper").display()
            )));
        }

        let ctx = self.whisper_context().await?;
        let language = self.config.whisper_language.clone();

        // Decode input audio to f32 samples at 16kHz (whisper's expected rate)
        let audio_owned = audio.to_vec();
        let text = tokio::task::spawn_blocking(move || {
            // Decode WAV → mono f32
            let (src_rate, samples) = decode_wav_to_f32(&audio_owned)
                .map_err(|e| VoiceError::Whisper(format!("audio decode failed: {e}")))?;

            // Resample to 16kHz if needed (whisper expects 16kHz)
            let samples_16k = resample_linear(&samples, src_rate, 16_000);

            // Run whisper inference
            transcribe_samples(&ctx, &samples_16k, &language)
        })
        .await
        .map_err(|e| VoiceError::Whisper(format!("spawn_blocking failed: {e}")))??;

        tracing::info!(chars = text.len(), "whisper transcription complete");
        Ok(TranscribeResult { text })
    }

    /// Check which local voice engines are available.
    pub fn status(&self) -> VoiceStatus {
        let whisper_model_exists = self.whisper_model_path().exists();
        #[cfg(feature = "kokoro-tts")]
        let (tts_engine, tts_loaded) = ("kokoro".to_string(), self.kokoro.initialized());
        #[cfg(not(feature = "kokoro-tts"))]
        let (tts_engine, tts_loaded) = ("system".to_string(), false);
        VoiceStatus {
            tts_engine,
            tts_loaded,
            tts_voice: self.config.tts_voice.clone(),
            whisper_available: whisper_model_exists,
            whisper_loaded: self.whisper_ctx.initialized(),
            models_dir: self.config.models_dir.clone(),
            whisper_model: if whisper_model_exists {
                Some(self.whisper_model_path())
            } else {
                None
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceStatus {
    pub tts_engine: String,
    pub tts_loaded: bool,
    pub tts_voice: String,
    pub whisper_available: bool,
    pub whisper_loaded: bool,
    pub models_dir: PathBuf,
    pub whisper_model: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Whisper transcription (runs on blocking thread)
// ---------------------------------------------------------------------------

/// Transcribe f32 PCM samples (16kHz mono) using a loaded whisper context.
/// Matches the pattern from hotword-core.
fn transcribe_samples(
    ctx: &WhisperContext,
    samples: &[f32],
    language: &str,
) -> Result<String, VoiceError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_suppress_nst(true);

    if language != "auto" && !language.is_empty() {
        params.set_language(Some(language));
    }

    let mut state = ctx
        .create_state()
        .map_err(|e| VoiceError::Whisper(format!("failed to create whisper state: {e}")))?;

    state
        .full(params, samples)
        .map_err(|e| VoiceError::Whisper(format!("whisper inference failed: {e}")))?;

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

    Ok(streaming::clean_transcript(text.trim()))
}
