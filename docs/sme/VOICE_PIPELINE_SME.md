# Voice Pipeline: Complete Architecture Reference

**Crates:** `crates/voice/` | **Frontend:** `app/src/lib/stores/`, `app/src/lib/components/chat/` | **Status:** Implemented (Rust + TypeScript)

This document is the authoritative reference for the Nebo voice pipeline. Covers local TTS (Piper subprocess), local STT (whisper-rs embedded FFI), streaming dictation via WebSocket, full-duplex voice conversation, the TipTap editor dictation integration, AudioWorklet playback, device management, and noise suppression.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Backend: Voice Crate](#2-backend-voice-crate)
3. [TTS Engine (Piper)](#3-tts-engine-piper)
4. [STT Engine (whisper-rs)](#4-stt-engine-whisper-rs)
5. [Streaming Transcriber](#5-streaming-transcriber)
6. [Conversation Orchestrator](#6-conversation-orchestrator)
7. [Noise Suppression](#7-noise-suppression)
8. [WAV Encoding & Decoding](#8-wav-encoding--decoding)
9. [Server Integration](#9-server-integration)
10. [HTTP & WebSocket API](#10-http--websocket-api)
11. [Frontend: Audio Infrastructure](#11-frontend-audio-infrastructure)
12. [Frontend: Dictation System](#12-frontend-dictation-system)
13. [Frontend: Voice Conversation](#13-frontend-voice-conversation)
14. [Frontend: TipTap Dictation Integration](#14-frontend-tiptap-dictation-integration)
15. [Frontend: Components](#15-frontend-components)
16. [Frontend: AudioWorklet Pipeline](#16-frontend-audioworklet-pipeline)
17. [Model Management](#17-model-management)
18. [Error Handling](#18-error-handling)
19. [Security Considerations](#19-security-considerations)
20. [Key Files Reference](#20-key-files-reference)

---

## 1. System Overview

### 1.1 Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Frontend (SvelteKit / Tauri WebView)                 │
│                                                                             │
│  ┌──────────────────┐  ┌─────────────────┐  ┌───────────────────────────┐  │
│  │ ChatComposer.svelte│ │ VoiceModeOverlay│  │  Chat components          │  │
│  │ (TipTap editor)  │  │ (full-screen    │  │  (call playTTS on reply)  │  │
│  │ + VoiceButton    │  │  conversation)  │  └────────────┬──────────────┘  │
│  │ + AudioLines btn │  └───────┬─────────┘               │                  │
│  └────────┬─────────┘          │                          │                  │
│           │                    │                          │                  │
│  ┌────────▼─────────┐  ┌──────▼──────────┐  ┌───────────▼──────────────┐  │
│  │ dictationStore   │  │ voiceSession    │  │  voiceStore              │  │
│  │ (4-state machine)│  │ (6-state machine│  │  (TTS playback)          │  │
│  │ + deviceManager  │  │  conversation)  │  │                          │  │
│  └───┬──────────────┘  └───────┬─────────┘  └──────────────────────────┘  │
│      │                         │                                           │
│  ┌───▼────────────┐    ┌───────▼─────────┐                                │
│  │ audio.ts       │    │ audio.ts        │                                │
│  │ (PCM capture   │    │ (PCM capture    │                                │
│  │  16kHz mono)   │    │  16kHz mono)    │                                │
│  └───┬────────────┘    └───────┬─────────┘                                │
│      │                         │                                           │
│  Binary PCM Int16          Binary PCM Int16                                │
│      │                         │                                           │
├──────┼─────────────────────────┼──────────────────────────────────────────┤
│      │          WebSocket      │          WebSocket                        │
│      ▼                         ▼                                           │
│  /ws/voice/dictation      /ws/voice/conversation                          │
│  (streaming STT)          (full-duplex: STT + Agent + TTS)                │
│                                                                            │
│  /api/v1/voice/tts        /api/v1/voice/transcribe                        │
│  (batch TTS)              (batch STT)                                      │
├────────────────────────────────────────────────────────────────────────────┤
│                     Server (Axum / crates/server/)                         │
│                                                                            │
│  ┌─────────────────────────────────────────────────────────────────────┐  │
│  │  AppState.voice: Arc<voice::VoicePipeline>                         │  │
│  └──────────────────────────┬──────────────────────────────────────────┘  │
│                              │                                             │
│  ┌───────────────────────────▼─────────────────────────────────────────┐  │
│  │                    VoicePipeline (crates/voice/)                     │  │
│  │                                                                      │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌────────────────────────────┐  │  │
│  │  │ Piper TTS   │  │ whisper-rs   │  │ StreamingTranscriber       │  │  │
│  │  │ (subprocess)│  │ (embedded)   │  │ (rolling-window inference) │  │  │
│  │  └─────────────┘  └──────────────┘  └────────────────────────────┘  │  │
│  │                                                                      │  │
│  │  ┌───────────────────────────────────────────────────────────────┐   │  │
│  │  │ ConversationOrchestrator                                      │   │  │
│  │  │ listen → transcribe → agent → TTS → playback → listen        │   │  │
│  │  └───────────────────────────────────────────────────────────────┘   │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────────┘
```

### 1.2 Three Voice Modes

| Mode | Trigger | Flow | WS Endpoint |
|------|---------|------|-------------|
| **Batch STT** | POST audio blob | Audio → whisper → text response | REST `/api/v1/voice/transcribe` |
| **Streaming Dictation** | VoiceButton click or Cmd+D | PCM stream → rolling whisper → live transcript → TipTap | `/ws/voice/dictation` |
| **Voice Conversation** | AudioLines button | PCM stream → whisper → agent pipeline → TTS → audio playback | `/ws/voice/conversation` |

### 1.3 Design Decisions

| Decision | Rationale |
|----------|-----------|
| Embedded whisper-rs (not CLI) | Eliminates temp file I/O, enables streaming STT via rolling windows |
| Lazy model loading via OnceCell | First-call init avoids startup delay; warmup forces Metal shader compilation |
| CLI subprocess for Piper TTS | Avoids C++ FFI build complexity; Piper lacks mature Rust bindings |
| Local-first (not cloud) | Zero latency, privacy, works offline |
| Rolling-window streaming | Simulates streaming from batch engine: ~750ms inference intervals, diff-based transcript updates |
| 4-state dictation machine | Ownership, auto-stop timers, push-to-talk support |
| 6-state conversation machine | Full-duplex voice with barge-in interruption |
| ScriptProcessorNode for capture | Direct PCM Int16 at 16kHz — matches whisper-rs expected format |
| Reactive $effect for TipTap | Replaced callback-based dictation with Svelte 5 reactive effects watching `combinedTranscript` |

---

## 2. Backend: Voice Crate

**File:** `crates/voice/src/lib.rs`

### 2.1 Modules

| Module | File | Purpose |
|--------|------|---------|
| `lib.rs` | `crates/voice/src/lib.rs` | VoicePipeline, TTS, batch STT, WAV helpers |
| `streaming` | `crates/voice/src/streaming.rs` | StreamingTranscriber, rolling-window inference, noise cleaning |
| `conversation` | `crates/voice/src/conversation.rs` | ConversationOrchestrator, full-duplex voice conversation |

### 2.2 VoicePipeline

```rust
pub struct VoicePipeline {
    config: VoicePipelineConfig,
    whisper_ctx: tokio::sync::OnceCell<Arc<WhisperContext>>,
}
```

### 2.3 Public API

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `(config) -> Self` | Constructor |
| `synthesize` | `(&self, req: TtsRequest) -> Result<Vec<u8>>` | Full WAV synthesis via Piper |
| `synthesize_stream` | `(&self, req, tx) -> Result<()>` | Chunked WAV over mpsc (~100ms chunks) |
| `transcribe` | `(&self, audio: &[u8]) -> Result<TranscribeResult>` | Batch audio-to-text via whisper-rs |
| `whisper_context` | `(&self) -> Result<Arc<WhisperContext>>` | Lazy-load shared whisper context |
| `status` | `(&self) -> VoiceStatus` | Engine availability report |

---

## 3. TTS Engine (Piper)

### 3.1 Synthesis Flow

```
synthesize(TtsRequest)
  ├─ piper_path() → Some(path)
  │   └─ synthesize_piper(path, req)
  │       ├─ voice_model_path(req.voice) → ~/.nebo/models/voice/piper/<voice>.onnx
  │       ├─ run_piper(): sh -c "echo '<escaped_text>' | piper --model <model> --output_file <tmp>"
  │       ├─ Read WAV from tmp → delete tmp
  │       └─ Return Vec<u8> (WAV bytes)
  └─ piper_path() → None
      └─ synthesize_placeholder() → 440Hz sine wave beep (0.5s)
```

### 3.2 Speed Control

Piper uses `--length_scale` (>1.0 = slower). Nebo's `speed` is inverted (>1.0 = faster):
```rust
let length_scale = if speed > 0.0 { 1.0 / speed } else { 1.0 };
```

---

## 4. STT Engine (whisper-rs)

### 4.1 Batch Transcription Flow

```
transcribe(audio_bytes)
  ├─ whisper_context() → lazy-load WhisperContext (OnceCell + warmup)
  ├─ spawn_blocking:
  │   ├─ decode_wav_to_f32(audio_bytes) → (sample_rate, mono_f32_samples)
  │   ├─ resample_linear(samples, src_rate, 16000)
  │   └─ transcribe_samples(ctx, samples_16k, language)
  │       ├─ FullParams::new(Greedy { best_of: 1 })
  │       ├─ set_suppress_nst(true)  ← suppress non-speech tokens
  │       ├─ state.full(params, samples)
  │       ├─ Iterate segments: state.full_n_segments() → get_segment(i) → to_str_lossy()
  │       └─ clean_transcript(text)  ← remove hallucination artifacts
  └─ TranscribeResult { text }
```

### 4.2 whisper-rs 0.16 API

| Method | Returns | Notes |
|--------|---------|-------|
| `WhisperContext::new_with_params(path, params)` | `Result<Self>` | Loads GGML model |
| `ctx.create_state()` | `Result<WhisperState>` | Per-inference state |
| `state.full(params, samples)` | `Result<()>` | Run inference |
| `state.full_n_segments()` | `i32` | Direct value (not Result) |
| `state.get_segment(i)` | `Option<WhisperSegment>` | Renamed from `full_get_segment_text` in 0.14 |
| `seg.to_str_lossy()` | `Result<Cow<str>>` | Text extraction |
| `params.set_suppress_nst(true)` | — | Suppress non-speech tokens (renamed from `set_suppress_non_speech_tokens`) |

---

## 5. Streaming Transcriber

**File:** `crates/voice/src/streaming.rs`

### 5.1 Overview

Simulates streaming STT from a batch inference engine. Accumulates PCM Int16 audio into a ring buffer, runs whisper periodically (~750ms), diffs the transcript, and emits interim/final results with silence-based endpointing.

### 5.2 StreamingConfig

| Field | Default | Description |
|-------|---------|-------------|
| `inference_interval` | 750ms | How often to run whisper on accumulated audio |
| `silence_threshold` | 0.01 | RMS energy below this = silence (0.0–1.0) |
| `endpointing_ms` | 300ms | Silence duration before emitting Endpoint |
| `utterance_end_ms` | 1000ms | Hard utterance boundary silence |
| `max_buffer_samples` | 480,000 | 30s at 16kHz |
| `overlap_samples` | 16,000 | 1s overlap kept when trimming processed audio |
| `language` | "en" | Whisper language code |

### 5.3 TranscriptEvent

```rust
pub enum TranscriptEvent {
    Interim(String),   // Partial, may be revised
    Text(String),      // Confirmed final segment
    Endpoint,          // Silence detected, utterance complete
    Error(String),
}
```

### 5.4 Processing Flow

```
StreamingTranscriber::start() → (audio_tx, event_rx)
  └─ spawns run_loop:
      loop {
        1. Receive audio chunk (100ms timeout)
        2. Convert Int16 → Float32, append to buffer
        3. Track silence via RMS energy
        4. Cap buffer at max_buffer_samples (trim from front)
        5. Every inference_interval: run whisper on buffer
           → diff against last transcript
           → emit Interim(new_portion)
        6. On silence >= endpointing_ms:
           → promote interim to Text(confirmed)
           → emit Endpoint
           → trim processed audio (keep overlap)
      }
```

### 5.5 Transcript Diffing

```rust
fn diff_transcript(confirmed: &str, full: &str) -> String {
    if full.starts_with(confirmed) {
        full[confirmed.len()..].trim().to_string()
    } else {
        full.trim().to_string() // Transcript diverged — return full
    }
}
```

---

## 6. Conversation Orchestrator

**File:** `crates/voice/src/conversation.rs`

### 6.1 Overview

Manages the full-duplex voice conversation loop: listen → transcribe → agent → TTS → playback → listen. Supports barge-in interruption.

### 6.2 State Machine

```
Phase::Listening  →  (utterance complete)  →  Phase::Processing
Phase::Processing →  (TTS starts)          →  Phase::Playing
Phase::Playing    →  (playback ends)       →  Phase::Listening
Phase::Playing    →  (user interrupts)     →  Phase::Listening
```

### 6.3 ConversationOrchestrator

```rust
pub struct ConversationOrchestrator {
    config: ConversationConfig,
    pipeline: Arc<VoicePipeline>,
    agent_tx: mpsc::Sender<AgentRequest>,
}
```

### 6.4 Agent Integration

The orchestrator sends `AgentRequest` objects via an mpsc channel. The server handler wires this to the real agent pipeline:

```rust
pub struct AgentRequest {
    pub text: String,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}
```

**Server-side handler** (`crates/server/src/handlers/voice.rs`):
- Creates a `RunRequest` with `max_iterations: 1` (single-turn, no tool loops)
- Runs through `runner.run()` and collects `StreamEventType::Text` events
- Returns accumulated text via the oneshot channel
- Stable session key for memory continuity across turns

### 6.5 ConversationEvent

```rust
pub enum ConversationEvent {
    SessionInitialized,
    TranscriptionStart,
    TranscriptionText(String),
    TranscriptionEnd,
    PlaybackStart,
    AudioChunk(Bytes),      // PCM Int16 TTS audio
    PlaybackEnd,
    ResponseText(String),   // Agent's text (for UI display)
    Error(String),
}
```

### 6.6 ConversationCommand

```rust
pub enum ConversationCommand {
    Audio(Vec<i16>),        // Raw PCM from client
    ManualInputEnd,         // Push-to-talk release
    Interrupt,              // Barge-in during playback
}
```

---

## 7. Noise Suppression

### 7.1 whisper-rs Non-Speech Token Suppression

Both streaming and batch transcription enable `set_suppress_nst(true)` in whisper params. This suppresses noise tokens like clicks, breathing, and non-speech sounds at the model level.

### 7.2 Post-Processing Artifact Removal

**Function:** `streaming::clean_transcript(text: &str) -> String`

Strips common whisper hallucination artifacts:

```rust
const ARTIFACTS: &[&str] = &[
    "[BLANK_AUDIO]", "[MUSIC]", "[APPLAUSE]", "[LAUGHTER]",
    "[NOISE]", "[SILENCE]", "[INAUDIBLE]", "[SOUND]",
    "(clicks)", "(clicking)", "(buzzing)", "(static)",
    "(background noise)", "(laughing)", "(sighing)", "(coughing)",
];
```

After stripping, collapses multiple spaces and trims. Used by both `streaming.rs` (rolling window) and `lib.rs` (batch transcription).

---

## 8. WAV Encoding & Decoding

### 8.1 Header: Standard 44-byte RIFF WAV (16-bit mono PCM)
### 8.2 Decoding: Supports 16-bit PCM, 32-bit float, mono/stereo (stereo averaged)
### 8.3 Resampling: `resample_linear()` — linear interpolation to whisper's 16kHz
### 8.4 Placeholder: 440Hz sine wave (0.5s) when Piper unavailable

---

## 9. Server Integration

### 9.1 AppState

```rust
pub voice: Arc<voice::VoicePipeline>,
```

### 9.2 Route Registration

**File:** `crates/server/src/lib.rs`

```rust
.route("/ws/voice/dictation", get(handlers::voice::dictation_ws_handler))
.route("/ws/voice/conversation", get(handlers::voice::conversation_ws_handler))
```

Plus REST routes: `/api/v1/voice/tts`, `/api/v1/voice/transcribe`, `/api/v1/voice/status`.

### 9.3 Handler Module

**File:** `crates/server/src/handlers/voice.rs`

| Handler | Route | Type | Description |
|---------|-------|------|-------------|
| `tts` | `/api/v1/voice/tts` | POST | JSON → WAV bytes |
| `transcribe` | `/api/v1/voice/transcribe` | POST | Audio bytes → JSON text |
| `status` | `/api/v1/voice/status` | GET | Engine availability |
| `dictation_ws_handler` | `/ws/voice/dictation` | WS | Streaming dictation |
| `conversation_ws_handler` | `/ws/voice/conversation` | WS | Full-duplex conversation |

---

## 10. HTTP & WebSocket API

### 10.1 POST /api/v1/voice/tts

Request: `{ "text": "...", "voice": "en_US-amy-medium", "speed": 1.0 }`
Response: `200 OK` with `Content-Type: audio/wav` body.

### 10.2 POST /api/v1/voice/transcribe

Request: Raw audio bytes. Response: `{ "text": "..." }`

### 10.3 GET /api/v1/voice/status

Response: `{ "piper_available": bool, "whisper_available": bool, "whisper_loaded": bool, ... }`

### 10.4 WS /ws/voice/dictation

Streaming speech-to-text with two routing modes (editor or agent).

**Client → Server:**

| Message | Format | Description |
|---------|--------|-------------|
| Start | JSON `{"type": "Start", "route": "editor"}` | Begin dictation (editor or agent mode) |
| Audio | Binary PCM Int16 (16kHz mono) | Audio chunks (~256ms each) |
| KeepAlive | JSON `{"type": "KeepAlive"}` | Keep connection alive |
| CloseStream | JSON `{"type": "CloseStream"}` | End dictation gracefully |

**Server → Client:**

| Message | Description |
|---------|-------------|
| `{"type": "TranscriptInterim", "text": "..."}` | Partial, may be revised |
| `{"type": "TranscriptText", "text": "..."}` | Confirmed segment |
| `{"type": "TranscriptEndpoint"}` | Utterance boundary (silence detected) |
| `{"type": "Error", "message": "..."}` | Error |

### 10.5 WS /ws/voice/conversation

Full-duplex voice conversation with agent integration.

**Client → Server:**

| Message | Format | Description |
|---------|--------|-------------|
| Audio | Binary PCM Int16 (16kHz mono) | Audio chunks |
| KeepAlive | JSON `{"type": "KeepAlive"}` | Keep alive |
| interrupt | JSON `{"type": "interrupt"}` | Barge-in during playback |
| manual_input_end | JSON `{"type": "manual_input_end"}` | Explicit end of input |

**Server → Client:**

| Message | Description |
|---------|-------------|
| `{"type": "session_initialized"}` | Session ready |
| `{"type": "transcription_start"}` | User started speaking |
| `{"type": "transcription_text", "text": "..."}` | Interim transcript |
| `{"type": "transcription_end"}` | User utterance complete |
| `{"type": "playback_start"}` | TTS audio starting |
| Binary PCM Int16 | TTS audio chunks |
| `{"type": "playback_end"}` | TTS audio complete |
| `{"type": "response_text", "text": "..."}` | Agent's text response |
| `{"type": "Error", "message": "..."}` | Error |

---

## 11. Frontend: Audio Infrastructure

### 11.1 PCM Capture (`app/src/lib/stores/audio.ts`)

Shared by both dictation and voice conversation. Creates AudioContext at 16kHz, ScriptProcessorNode (buffer 4096 = 256ms chunks), Float32→Int16 conversion, AnalyserNode for level monitoring.

```typescript
export function startPcmCapture(stream: MediaStream, callbacks: AudioCaptureCallbacks): AudioCaptureHandle
```

### 11.2 Device Manager (`app/src/lib/stores/devices.ts`)

Manages microphone enumeration, selection persistence (localStorage), hot-plug detection, and OverconstrainedError fallback.

| Method | Description |
|--------|-------------|
| `refresh()` | Re-enumerate devices |
| `selectMic(deviceId)` | Persist mic selection |
| `getMicConstraints()` | MediaStreamConstraints with selected device |
| `acquireMicStream()` | getUserMedia with fallback to default on OverconstrainedError |

---

## 12. Frontend: Dictation System

### 12.1 Dictation Store (`app/src/lib/stores/dictation.ts`)

4-state machine with ownership model:

```
idle → connecting → recording → idle
                  → error → idle (5s auto-clear)
```

**Key features:**
- **Ownership**: Each composer instance has a unique `ownerId`. Only the owning composer receives transcript events.
- **Push-to-talk**: 500ms hold threshold via `setPushToTalk()`. Short press = toggle, long press = PTT.
- **Hold-to-record**: Configurable via `setHoldToRecordEnabled()`. Persisted in localStorage.
- **Silence-based stop**: Backend handles endpointing via StreamingTranscriber.
- **Combined transcript**: `combinedTranscript` derived store joins `transcript + interimTranscript` with smart spacing.

### 12.2 Dictation Routes

| Route | Description |
|-------|-------------|
| `{ type: 'editor' }` | Transcript sent to client only (TipTap insertion) |
| `{ type: 'agent', agentId }` | Transcript also fed to agent via hub broadcast |

### 12.3 Exported Stores

| Store | Type | Description |
|-------|------|-------------|
| `dictationStore` | writable | Full state + methods |
| `dictationStatus` | derived | Current status string |
| `isDictating` | derived | true when recording |
| `combinedTranscript` | derived | transcript + interim with smart spacing |

---

## 13. Frontend: Voice Conversation

### 13.1 Voice Session Store (`app/src/lib/stores/voiceSession.ts`)

6-state machine for full-duplex voice conversation:

```
idle → connecting → listening → processing → speaking → listening (loop)
                              → error → idle (5s auto-clear)
```

**Key features:**
- WebSocket to `/ws/voice/conversation`
- PCM capture via shared `startPcmCapture()`
- TTS playback via AudioContext (24kHz), Int16→Float32 conversion
- Barge-in: if user speaks during playback, stops TTS and sends interrupt
- Keepalive: 4s interval
- Transcript history: `transcripts` array of `{ speaker: 'user'|'agent', text }` entries

### 13.2 Known Bug Fix: Race Condition

The `session_initialized` event arrives from the backend while the frontend is still awaiting `getUserMedia` permission. The status check after mic acquisition must accept both `connecting` and `listening` states:

```typescript
// WRONG: bails out because session_initialized already changed status to 'listening'
if (readState().status !== 'connecting') { ... }

// CORRECT: only bail on explicit stop or error
const currentStatus = readState().status;
if (currentStatus === 'idle' || currentStatus === 'error') { ... }
```

---

## 14. Frontend: TipTap Dictation Integration

### 14.1 Overview

The ChatComposer uses TipTap (ProseMirror-based editor) with a custom `dictation` mark for highlighting dictated text. Matches Claude Desktop's `KVt` pattern.

### 14.2 Reactive Approach (Svelte 5 $effect)

Two `$effect` blocks replace the old callback-based approach:

**Effect 1: Start/Stop Transitions**
- On start: captures cursor position, splits text into `dictationBefore` and `dictationAfter`
- On stop: clears dictation marks, restores cursor

**Effect 2: Transcript Updates**
- Watches `$combinedTranscript` reactively
- Rebuilds entire TipTap JSON doc via `buildDictationDoc(before, dictation, after)`
- Smart spacing: adds leading/trailing spaces based on context
- Sets cursor position via `textOffsetToDocPos()`

### 14.3 buildDictationDoc(before, dictationText, after)

Builds TipTap JSON with dictation marks per line:
- Splits text into lines, creates paragraph nodes
- Dictation segments get `{ type: 'text', text, marks: [{ type: 'dictation' }] }`
- Non-dictation segments are plain text nodes

### 14.4 textOffsetToDocPos(fullText, textOffset, docContentSize)

Converts plain-text offset to ProseMirror document position, accounting for paragraph boundaries (+1 per newline).

### 14.5 Cmd+D Hotkey

`handleDictationHotkey(e)` toggles dictation on Cmd+D (Mac) / Ctrl+D (Windows/Linux).

---

## 15. Frontend: Components

### 15.1 VoiceButton (`app/src/lib/components/chat/VoiceButton.svelte`)

Mic button with dropdown for voice settings.

**States:**

| State | Icon | Style | Action |
|-------|------|-------|--------|
| Idle | `Mic` | Default | Click → start recording |
| Recording | `MicOff` | `text-error animate-pulse` | Click → stop |
| Connecting | `Mic` | `text-warning animate-pulse` | — |
| Playing | `Volume2` | `text-primary voice-pulse` | Click → stop playback |

**Dropdown (chevron trigger):**
- Microphone list with selection checkmarks
- Hold-to-record toggle

**Hold-to-record logic:**
- `pointerdown` starts recording + 500ms timer
- If held >500ms: activates PTT mode (release to stop)
- If released <500ms: toggle mode (stays recording)

### 15.2 VoiceModeOverlay (`app/src/lib/components/chat/VoiceModeOverlay.svelte`)

Full-screen voice conversation overlay. Auto-starts session on mount.

**Layout:**
- Header: agent name + status text ("Listening...", "Thinking...", "Speaking...")
- Center: audio visualization circle with pulse ring (maps `audioLevel` to scale)
- Transcript area: scrollable history of user/agent turns + interim transcript
- Bottom controls: mute toggle + stop button

### 15.3 ChatComposer Toolbar

```
[Attach] [VoiceButton + chevron] [AudioLines conversation btn] ... [Send/Stop]
```

The AudioLines button opens VoiceModeOverlay for full-duplex voice conversation.

---

## 16. Frontend: AudioWorklet Pipeline

**File:** `app/static/audioSinkWorklet.js`

SharedArrayBuffer + Atomics ring buffer for lock-free TTS playback. See voiceStore for usage. RING_BUFFER_SAMPLES = 120,000 (5s at 24kHz).

---

## 17. Model Management

### 17.1 Directory Structure

```
~/.nebo/models/voice/
├── piper/
│   ├── en_US-amy-medium.onnx
│   └── en_US-amy-medium.onnx.json
└── whisper/
    └── ggml-tiny.en.bin   (75MB, default)
```

### 17.2 Whisper Models (from HuggingFace)

| Model | Size | Recommended |
|-------|------|-------------|
| `ggml-tiny.en.bin` | 75MB | Default |
| `ggml-base.en.bin` | 148MB | Better accuracy |
| `ggml-small.en.bin` | 488MB | Best if resources allow |

---

## 18. Error Handling

### 18.1 Backend: VoiceError enum

`Io`, `Encoding`, `ChannelClosed`, `BinaryNotFound`, `ModelNotFound`, `SubprocessFailed`, `Whisper`.

### 18.2 Fallback Behavior

| Engine | Missing | Behavior |
|--------|---------|----------|
| Piper TTS | not in PATH | 440Hz sine wave beep |
| whisper-rs | model file missing | `VoiceError::ModelNotFound` with download URL |

---

## 19. Security Considerations

- **Command injection**: Piper text shell-escaped via single-quote wrapping
- **Temp file cleanup**: Always attempted, even on error
- **No auth on voice routes**: Appropriate for local Tauri desktop
- **SharedArrayBuffer**: Requires cross-origin isolation (satisfied by Tauri WebView)

---

## 20. Key Files Reference

### 20.1 Backend

| File | Purpose |
|------|---------|
| `crates/voice/src/lib.rs` | VoicePipeline, batch TTS/STT, WAV helpers |
| `crates/voice/src/streaming.rs` | StreamingTranscriber, rolling-window inference, clean_transcript |
| `crates/voice/src/conversation.rs` | ConversationOrchestrator, full-duplex voice |
| `crates/voice/Cargo.toml` | Dependencies (whisper-rs 0.16) |
| `crates/server/src/handlers/voice.rs` | HTTP + WS handlers, agent integration |
| `crates/server/src/lib.rs` | Route registration |

### 20.2 Frontend

| File | Purpose |
|------|---------|
| `app/src/lib/stores/audio.ts` | Shared PCM capture (ScriptProcessorNode, Float32→Int16) |
| `app/src/lib/stores/devices.ts` | Mic enumeration, selection, hot-plug |
| `app/src/lib/stores/dictation.ts` | 4-state dictation machine, ownership, PTT |
| `app/src/lib/stores/voiceSession.ts` | 6-state conversation machine |
| `app/src/lib/stores/voice.ts` | TTS playback (AudioWorklet + SharedArrayBuffer) |
| `app/src/lib/components/chat/VoiceButton.svelte` | Mic button + dropdown |
| `app/src/lib/components/chat/VoiceModeOverlay.svelte` | Full-screen conversation overlay |
| `app/src/lib/components/chat/ChatComposer.svelte` | TipTap editor + dictation integration |
| `app/static/audioSinkWorklet.js` | AudioWorklet ring buffer |
| `app/src/app.css` | voice-pulse animation, dictation mark styles |
