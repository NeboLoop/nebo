# Full-Duplex Voice System

Complete reference for evolving Nebo's voice from half-duplex HTTP round-trips to a Grok-style full-duplex WebSocket binary stream.

---

## 1. Architecture Overview

### Current: Half-Duplex HTTP Round-Trip (7 hops per voice turn)

```
Browser                           Server
───────                           ──────
1. getUserMedia({audio:true})
2. MediaRecorder.start(250ms)
3. Silence detect (RMS, 2.5s)
4. MediaRecorder.stop()
5. POST /api/v1/voice/transcribe ──→ 6. whisper-cli / OpenAI Whisper
                                  ←── 7. {text: "..."}
8. Send text via WebSocket chat   ──→ 9. runner.Run() (agentic loop)
                                  ←── 10. chat_stream events (text)
11. POST /api/v1/voice/tts        ──→ 12. ElevenLabs / macOS say
                                  ←── 13. audio/mpeg blob
14. new Audio(blob).play()
15. onended → goto 2
```

**Problems:** ~3-5s round-trip per turn. Browser owns the state machine (~500 lines). No overlap between ASR/LLM/TTS. User must wait for full response before speaking again.

### Target: Full-Duplex WebSocket Binary Stream

```
Browser                              Server
───────                              ──────
AudioWorklet CaptureProcessor        /ws/voice (gorilla/websocket)
  │ PCM Int16LE frames (20ms)          │
  │──────── binary ────────────────→   │
  │                                    ├─→ inAudio chan
  │                                    │     │
  │                                    │   noiseGate → VAD → asrLoop
  │                                    │                       │
  │                                    │                    asrText chan
  │                                    │                       │
  │                                    │                    llmLoop
  │                                    │                    (runner.Run)
  │                                    │                       │
  │                                    │                    ttsText chan
  │                                    │                       │
  │                                    │                    ttsLoop
  │                                    │                       │
  │                                    │                    outAudio chan
  │                                    │                       │
  │   ←──────── binary ───────────────┘   speakerLoop
  │
AudioWorklet PlaybackProcessor
  │ ring buffer → speakers
```

**Key insight:** Voice is just another channel feeding into `runner.Run()`. Like web UI, CLI, Telegram, or DMs — it produces a prompt string and consumes `StreamEvent`s. The difference is transport (binary WebSocket) and I/O (audio frames instead of text).

### Three-Hub Relationship

```
┌─────────────────────────────────────────────────────────────┐
│                     Nebo Server                              │
│                                                              │
│  /ws              → Client Hub (realtime/hub.go)             │
│                     Browser JSON WebSocket                   │
│                     Chat events, tool results, approvals     │
│                                                              │
│  /api/v1/agent/ws → Agent Hub (agenthub/hub.go)             │
│                     Agent JSON WebSocket                     │
│                     Frames: req/res/stream/event             │
│                                                              │
│  /ws/voice        → Voice Handler (voice/duplex.go) [NEW]   │
│                     Binary+JSON mixed WebSocket              │
│                     Audio frames in/out + control messages   │
│                     NOT routed through agent or client hub   │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

The voice WebSocket is independent — it has its own `readPump`/`writePump` goroutines modeled on the existing patterns in `realtime/client.go:74-134` and `agenthub/hub.go:477-555`. It does NOT route through either hub. It directly calls `runner.Run()` via lane enqueue.

---

## 2. Audio Front-End (Browser)

### Current vs Target

| Aspect | Current (`+page.svelte:1369-1842`) | Target (AudioWorklet + VoiceSession) |
|--------|-----------------------------------|--------------------------------------|
| Capture | `MediaRecorder` on `getUserMedia` stream | `AudioWorkletProcessor` (CaptureProcessor) |
| Format | webm/opus blob (250ms timeslice) | PCM Int16LE frames (20ms = 960 samples @48kHz) |
| VAD | Browser-side RMS (100ms poll interval) | Server-side (noise gate + VAD) |
| Silence detect | `SILENCE_TIMEOUT = 2500ms` | Server controls via VAD hangover |
| TTS playback | `new Audio(blob)` per sentence | `AudioWorkletProcessor` (PlaybackProcessor) ring buffer |
| AEC | None (relies on speaker distance) | `getUserMedia({echoCancellation: true})` |
| State machine | ~500 lines in `+page.svelte` | Server-driven; browser is simple "active" boolean |
| Transport | HTTP POST + JSON WebSocket | Binary WebSocket (`/ws/voice`) |

### Current Code Layout

The browser voice system lives entirely in `+page.svelte`:

- **L97-107:** TTS state variables (`voiceOutputEnabled`, `isSpeaking`, `ttsQueue`, `ttsCancelToken`, etc.)
- **L1369-1406:** Voice mode entry/exit (`toggleRecording`, debounce guard)
- **L1408-1463:** `enterVoiceMode()` — getUserMedia, AudioContext, AnalyserNode, MIME type detection
- **L1465-1483:** `exitVoiceMode()` — cleanup streams, AudioContext, analyser
- **L1486-1551:** `startListening()` — MediaRecorder setup, `ondataavailable`, `onstop` → transcription
- **L1553-1577:** `stopListening()`, `finishRecording()` — cleanup without/with transcription
- **L1579-1640:** `startVoiceMonitor()` — 100ms interval, RMS calculation, silence/interrupt detection
- **L1649-1668:** `handleRecordingComplete()` — auto-send transcribed text
- **L1671-1683:** `cleanTextForTTS()` — strip markdown for speech
- **L1686-1706:** `feedTTSStream()` — sentence splitting regex, queue sentences for TTS
- **L1708-1719:** `flushTTSBuffer()` — flush remaining text on stream complete
- **L1722-1777:** `playNextTTS()` — queue player, `speakTTS()` API call, Audio element playback
- **L1780-1795:** `stopTTSQueue()` — cancel token increment, drain queue, pause audio
- **L1797-1842:** `speakText()` — legacy non-streaming TTS with Web Speech API fallback

### Target: AudioWorklet Processors

**CaptureProcessor** (`app/src/lib/voice/capture-processor.ts` — CREATE):

```typescript
// AudioWorkletProcessor runs in a separate thread — no main-thread jank
class CaptureProcessor extends AudioWorkletProcessor {
  process(inputs: Float32Array[][], outputs: Float32Array[][], parameters: Record<string, Float32Array>) {
    const input = inputs[0]?.[0]; // mono channel
    if (!input || input.length === 0) return true;

    // Convert Float32 [-1.0, 1.0] to Int16LE [-32768, 32767]
    const pcm = new Int16Array(input.length);
    for (let i = 0; i < input.length; i++) {
      const s = Math.max(-1, Math.min(1, input[i]));
      pcm[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
    }

    // Post to main thread for WS send
    this.port.postMessage(pcm.buffer, [pcm.buffer]);
    return true; // keep processor alive
  }
}

registerProcessor('capture-processor', CaptureProcessor);
```

**PlaybackProcessor** (`app/src/lib/voice/playback-processor.ts` — CREATE):

```typescript
class PlaybackProcessor extends AudioWorkletProcessor {
  private buffer: Float32Array[] = [];

  constructor() {
    super();
    this.port.onmessage = (e) => {
      // Receive PCM frames from main thread (decoded from WS binary)
      this.buffer.push(new Float32Array(e.data));
    };
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][], parameters: Record<string, Float32Array>) {
    const output = outputs[0]?.[0];
    if (!output) return true;

    // Fill output from ring buffer
    let written = 0;
    while (written < output.length && this.buffer.length > 0) {
      const chunk = this.buffer[0];
      const needed = output.length - written;
      const available = chunk.length;

      if (available <= needed) {
        output.set(chunk, written);
        written += available;
        this.buffer.shift();
      } else {
        output.set(chunk.subarray(0, needed), written);
        this.buffer[0] = chunk.subarray(needed);
        written = output.length;
      }
    }

    // Zero-fill if buffer underrun (silence, no click)
    if (written < output.length) {
      output.fill(0, written);
    }

    return true;
  }
}

registerProcessor('playback-processor', PlaybackProcessor);
```

**VoiceSession** (`app/src/lib/voice/VoiceSession.ts` — CREATE):

Manages the binary WebSocket connection and AudioWorklet lifecycle. This replaces the ~500-line state machine in `+page.svelte`. The browser becomes a thin pipe: capture PCM → send binary frames, receive binary frames → play PCM. All intelligence (VAD, sentence splitting, state transitions) lives server-side.

### AEC: The One-Liner Fix

Current `enterVoiceMode()` at `+page.svelte:1427`:

```typescript
// Current — no AEC
voiceStream = await navigator.mediaDevices.getUserMedia({ audio: true });
```

Target:

```typescript
// AEC enabled — browser's WebRTC audio processing removes speaker echo
voiceStream = await navigator.mediaDevices.getUserMedia({
  audio: {
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true,
  }
});
```

This activates the browser's built-in WebRTC AEC, which runs at the audio driver level BEFORE the AudioWorklet capture processor sees the samples. For Phase 1, this handles ~90% of echo scenarios.

---

## 3. Transport — WebSocket Binary Protocol

### Endpoint Registration

New endpoint in `server.go`, registered alongside the existing voice HTTP routes (L162-164) and WebSocket mounts (L204-205):

```go
// Existing voice HTTP routes (KEEP — still used by non-duplex features)
r.Post("/voice/transcribe", voice.TranscribeHandler)
r.Post("/voice/tts", voice.TTSHandler)
r.Get("/voice/voices", voice.VoicesHandler)

// ... later, alongside existing WS routes:
r.Get("/ws", websocket.Handler(hub))             // Client hub (existing)
r.Get("/api/v1/agent/ws", agentWebSocketHandler(svcCtx))  // Agent hub (existing)
r.Get("/ws/voice", voice.DuplexHandler(svcCtx))  // Voice duplex (NEW)
```

The `/ws/voice` endpoint is a WebSocket upgrade, not a REST API. No `make gen` needed for this endpoint alone. If REST endpoints are added later (e.g., voice session management), then `make gen` must be run and frontend must use generated TS API functions from `$lib/api/`.

### Frame Types

The voice WebSocket carries mixed text (JSON control) and binary (audio) messages. This is native to gorilla/websocket — `TextMessage` vs `BinaryMessage` are distinct frame types.

| Type | Direction | Wire | Format | Description |
|------|-----------|------|--------|-------------|
| `audio_in` | client→server | Binary | PCM Int16LE or Opus | Captured audio frame (20ms) |
| `audio_out` | server→client | Binary | PCM Int16LE or Opus | TTS audio frame for playback |
| `session_start` | client→server | Text | `{"type":"session_start","sample_rate":48000,"codec":"pcm"}` | Initialize voice session |
| `session_started` | server→client | Text | `{"type":"session_started","session_id":"..."}` | Confirm session ready |
| `vad_state` | server→client | Text | `{"type":"vad_state","speaking":true}` | Server VAD speech detection |
| `transcript` | server→client | Text | `{"type":"transcript","text":"...","final":false}` | ASR result (partial or final) |
| `llm_text` | server→client | Text | `{"type":"llm_text","text":"...","done":false}` | LLM streaming text |
| `state_change` | server→client | Text | `{"type":"state_change","state":"speaking"}` | Server-driven state transition |
| `interrupt_ack` | server→client | Text | `{"type":"interrupt_ack"}` | Barge-in acknowledged |
| `error` | server→client | Text | `{"type":"error","message":"..."}` | Error notification |
| `codec_switch` | server→client | Text | `{"type":"codec_switch","codec":"opus"}` | Negotiate codec upgrade |
| `session_end` | either | Text | `{"type":"session_end"}` | Graceful close |

### readPump/writePump Pattern (Reuse)

The voice handler reuses the same goroutine pattern as `realtime/client.go:74-134` and `agenthub/hub.go:477-555`:

- **readPump:** Reads from WebSocket in a loop. Binary messages → `inAudio` channel. Text messages → JSON parse → control handler.
- **writePump:** Selects on `outAudio` channel (binary frames) and `controlOut` channel (JSON messages). Sends appropriate message type. Handles ping/pong keepalive.

Key difference from existing hubs: mixed binary+text writes. gorilla/websocket supports this natively via `conn.WriteMessage(websocket.BinaryMessage, data)` vs `conn.WriteMessage(websocket.TextMessage, data)`.

---

## 4. Opus Codec Integration

### Library

`github.com/hraban/opus` — Go bindings for libopus. CGO required (links against C libopus).

### Frame Size Trade-Offs

| Frame Size | Samples @48kHz | Latency | Quality | Use Case |
|------------|---------------|---------|---------|----------|
| 2.5ms | 120 | Ultra-low | Poor | Real-time gaming |
| 5ms | 240 | Very low | Fair | VoIP (aggressive) |
| 10ms | 480 | Low | Good | VoIP (standard) |
| **20ms** | **960** | **Good balance** | **Excellent** | **Recommended for Nebo** |
| 40ms | 1920 | Higher | Excellent | Music streaming |

**Recommended: 20ms frames (960 samples @48kHz)**

### Bandwidth Savings

| Format | Bitrate | Per-second | 10min conversation |
|--------|---------|------------|-------------------|
| PCM Int16 mono 48kHz | 768 kbps | 96 KB | 57.6 MB |
| Opus 24kbps | 24 kbps | 3 KB | 1.8 MB |
| **Reduction** | **32x** | | |

### Build Tag Strategy

```go
//go:build opus

package voice

import "gopkg.in/hraban/opus.v2"

type OpusEncoder struct { ... }
type OpusDecoder struct { ... }
```

Desktop builds get Opus (CGO is already enabled for desktop via `-tags desktop`). Headless/server builds get PCM-only (no CGO dependency). The codec is negotiated at session start — server advertises capabilities, client picks the best match.

---

## 5. Audio Input Pipeline: Noise Gate → VAD → Suppression

Three distinct layers in the server-side audio input pipeline. Each has a clear responsibility.

```
inAudio ──→ [Layer 1: Noise Gate] ──→ [Layer 2: VAD] ──→ ASR Buffer
              Phase 1 (pure Go)        Build-tagged:       accumulate on
              Discard sub-floor         Desktop → Silero    speech start,
              frames (fan, hum)         Headless → RMS      finalize on
                                                            speech end
```

### Layer 1 — Noise Gate (Phase 1, pure Go, zero deps)

**Purpose:** Discard frames below the ambient noise floor. Saves CPU (silent frames never reach VAD or ASR) and kills constant low-level noise like fan hum, AC, or electrical buzz.

**NOT a VAD** — it cannot distinguish speech from other sounds above the threshold. It only gates on energy level.

**Calibration:**

```go
// On connection: measure RMS of first 500ms of silence
// Set gate threshold at floor + 6dB headroom
func (ng *NoiseGate) Calibrate(frames [][]int16) {
    var totalRMS float64
    for _, frame := range frames {
        totalRMS += rms(frame)
    }
    avgRMS := totalRMS / float64(len(frames))
    ng.threshold = avgRMS * 2.0 // +6dB ≈ 2x amplitude
}

func (ng *NoiseGate) Process(frame []int16) bool {
    return rms(frame) > ng.threshold
}
```

### Layer 2 — VAD (build-tagged, both ship Phase 1)

Both implementations satisfy the same interface. Selected at init time based on build environment:

```go
// VAD interface — in vad.go
type VAD interface {
    ProcessFrame(frame []int16) bool
    Reset()
}
```

**File layout:**

| File | Build tag | Available when | Implementation |
|------|-----------|----------------|----------------|
| `vad.go` | none | always | VAD interface, NoiseGate, `rms()` utility |
| `vad_rms.go` | `!silero` | headless, CI, Docker, ARM, no CGO | RMS energy + hangover |
| `vad_silero.go` | `cgo && silero` | desktop builds with ONNX Runtime | Silero ONNX model |

**Selection at init:**

```go
// vad_rms.go
//go:build !silero

func NewDefaultVAD() VAD {
    return NewRMSVAD(0.06, 300)
}
```

```go
// vad_silero.go
//go:build cgo && silero

func NewDefaultVAD() VAD {
    vad, err := NewSileroVAD("silero_vad.onnx")
    if err != nil {
        // Fall back to RMS if model fails to load
        return NewRMSVAD(0.06, 300)
    }
    return vad
}
```

Desktop builds: `go build -tags "desktop silero"` → Silero VAD.
Headless builds: `go build` → RMS VAD. No CGO required.

#### RMS VAD (`vad_rms.go` — permanent fallback, not throwaway)

Pure Go, zero deps. The fallback for any environment without ONNX Runtime.

**Handles well:** Quiet room, clear speech starts/stops, long pauses.
**Fails on:** Keyboard typing (similar energy to speech), background music, TV/radio.

```go
type RMSVAD struct {
    threshold   float64
    hangoverMs  int
    speaking    bool
    silentSince time.Time
}

func (v *RMSVAD) ProcessFrame(frame []int16) bool {
    level := rms(frame)

    if level > v.threshold {
        v.speaking = true
        v.silentSince = time.Time{}
        return true
    }

    if v.speaking {
        if v.silentSince.IsZero() {
            v.silentSince = time.Now()
        }
        if time.Since(v.silentSince) < time.Duration(v.hangoverMs)*time.Millisecond {
            return true
        }
        v.speaking = false
    }
    return false
}
```

#### Silero VAD (`vad_silero.go` — desktop default)

**Model:** `silero_vad.onnx` (~900KB, MIT license)
- 30ms chunks (480 samples @16kHz, resample from 48kHz)
- ~1ms inference per frame on CPU
- Binary output: speech probability 0.0-1.0, threshold at 0.5

**Go ONNX runtime:** `github.com/yalue/onnxruntime_go` (CGO, bundles ONNX Runtime shared lib)

Handles all the edge cases RMS can't: keyboard typing, background music, non-speech vocalizations. Falls back to RMS VAD if the ONNX model fails to load.

### Layer 3 — Noise Suppression (Phase 3, deferred)

RNNoise or NSNet2 — cleans the speech signal, removes background noise from voiced frames. **Separate from VAD:** Layer 2 decides IF someone is speaking, Layer 3 cleans WHAT they said. This improves ASR accuracy in noisy environments.

---

## 6. Server Pipeline — Concurrent Goroutines

### Channel Architecture

```
                    readPump
                       │
                       ▼
                  ┌──────────┐
                  │ inAudio  │ chan []int16, buffered 50
                  └────┬─────┘
                       │
                  noiseGate.Process()
                       │
                   vad.ProcessFrame()
                       │
                  ┌──────────┐
                  │ asrText  │ chan string, buffered 1
                  └────┬─────┘
                       │
                  ┌──────────┐
                  │ llmLoop  │ runner.Run() via LaneMain
                  └────┬─────┘
                       │
                  ┌──────────┐
                  │ ttsText  │ chan string, buffered 10
                  └────┬─────┘
                       │
                  ┌──────────┐
                  │ outAudio │ chan []byte, buffered 50
                  └────┬─────┘
                       │
                   writePump
```

### Goroutine Descriptions

**asrLoop** — Accumulates PCM frames during speech (VAD=true), finalizes when VAD transitions to false (speech end + hangover).

Phase 1 implementation **reuses** existing `transcribeLocal()` and `convertToWav()` from `transcribe.go:212,258`:

```go
func (vc *VoiceConn) asrLoop(ctx context.Context) {
    var speechBuf []int16

    for {
        select {
        case <-ctx.Done():
            return
        case frame := <-vc.inAudio:
            // Noise gate
            if !vc.noiseGate.Process(frame) {
                continue
            }

            // VAD
            isSpeech := vc.vad.ProcessFrame(frame)
            vc.sendControl("vad_state", map[string]any{"speaking": isSpeech})

            if isSpeech {
                speechBuf = append(speechBuf, frame...)
            } else if len(speechBuf) > 0 {
                // Speech ended — transcribe
                go func(audio []int16) {
                    // Write PCM to temp WAV file
                    wavPath, err := writeWavFile(audio, 16000)
                    if err != nil { return }
                    defer os.Remove(wavPath)

                    // REUSE: existing transcribeLocal() from transcribe.go:212
                    text, err := transcribeLocal(wavPath, defaultModelPath())
                    if err != nil { return }

                    text = strings.TrimSpace(text)
                    if text != "" && text != "[BLANK_AUDIO]" {
                        vc.asrText <- text
                        vc.sendControl("transcript", map[string]any{
                            "text": text, "final": true,
                        })
                    }
                }(speechBuf)
                speechBuf = nil
            }
        }
    }
}
```

Phase 2 upgrades to streaming ASR (Deepgram/Google WebSocket) — text arrives during speech, not after.

**llmLoop** — Receives transcribed text, feeds to `runner.Run()`, consumes `StreamEvent`s. **Reuses** the same event consumption pattern as `cmd/nebo/agent.go:1840-1902` (DM handler).

```go
func (vc *VoiceConn) llmLoop(ctx context.Context) {
    for {
        select {
        case <-ctx.Done():
            return
        case text := <-vc.asrText:
            vc.sendControl("state_change", map[string]any{"state": "processing"})

            // Run through the agentic loop — same as web UI and DMs
            // Enqueue in LaneMain (serialized with text chat)
            err := vc.lanes.Enqueue(ctx, agenthub.LaneMain, func(taskCtx context.Context) error {
                events, err := vc.runner.Run(taskCtx, &runner.RunRequest{
                    SessionKey: "companion-default",
                    Prompt:     text,
                    Origin:     tools.OriginUser,
                    Channel:    "voice",
                })
                if err != nil { return err }

                // Consume stream events — mirror agent.go DM pattern
                var sentenceBuf strings.Builder
                for event := range events {
                    switch event.Type {
                    case ai.EventTypeText:
                        vc.sendControl("llm_text", map[string]any{
                            "text": event.Text, "done": false,
                        })
                        // Sentence splitting for TTS
                        sentenceBuf.WriteString(event.Text)
                        vc.extractSentences(&sentenceBuf)
                    case ai.EventTypeDone:
                        vc.flushSentenceBuffer(&sentenceBuf)
                        vc.sendControl("llm_text", map[string]any{
                            "text": "", "done": true,
                        })
                    }
                }
                return nil
            }, agenthub.WithDescription("voice input"))

            if err != nil {
                vc.sendControl("error", map[string]any{"message": err.Error()})
            }
        }
    }
}
```

**Sentence splitting** — The regex logic currently in `+page.svelte:1686-1706` (`feedTTSStream()`) gets **moved** to server-side Go. When the half-duplex browser code is removed (see Section 10), the frontend version goes with it.

```go
// extractSentences pulls complete sentences from the buffer and sends to TTS.
// MOVED from +page.svelte feedTTSStream() — same regex, Go version.
var sentenceEnd = regexp.MustCompile(`([.!?])\s`)

func (vc *VoiceConn) extractSentences(buf *strings.Builder) {
    text := buf.String()
    for {
        loc := sentenceEnd.FindStringIndex(text)
        if loc == nil { break }

        sentence := strings.TrimSpace(text[:loc[1]])
        text = text[loc[1]:]

        clean := cleanForTTS(sentence)
        if len(clean) > 2 {
            vc.ttsText <- clean
        }
    }
    buf.Reset()
    buf.WriteString(text)
}
```

**ttsLoop** — Receives sentences, generates audio. Phase 1 **reuses** existing `serveElevenLabsTTS()` logic from `transcribe.go:119` (extracted to a callable function) with macOS `say` fallback.

```go
func (vc *VoiceConn) ttsLoop(ctx context.Context) {
    for {
        select {
        case <-ctx.Done():
            return
        case sentence := <-vc.ttsText:
            vc.sendControl("state_change", map[string]any{"state": "speaking"})

            // Phase 1: REUSE existing TTS backends from transcribe.go
            audioData, contentType, err := synthesizeSpeech(sentence)
            if err != nil { continue }

            // Send audio frames to browser
            vc.outAudio <- audioData
        }
    }
}
```

Phase 3 upgrades to ElevenLabs streaming WebSocket API — first audio byte arrives during LLM generation.

**speakerLoop** — Drains `outAudio` and writes binary frames to the WebSocket. Part of `writePump`.

### Lane Integration

Voice input is enqueued in `LaneMain` (concurrency 1) — serialized with text chat. This means a user cannot have a voice conversation AND a text chat running simultaneously on the same `companion-default` session. This is correct behavior: both are user input to the same conversation.

---

## 7. Echo Cancellation Deep Dive

### Phase 1: Browser WebRTC AEC (the one-liner)

```typescript
getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true } })
```

How it works: The browser's WebRTC audio processing module (APM) runs at the OS audio driver level. It captures the speaker output as a reference signal and subtracts it from the microphone input using an adaptive filter. This happens BEFORE the AudioWorklet capture processor sees the samples.

**Coverage:** ~90% of echo scenarios on laptop speakers. Fails on: external speakers at high volume, reverberant rooms, Bluetooth audio (variable latency confuses the filter).

### Phase 2: NLMS Adaptive Filter (server-side, deferred)

For cases where browser AEC isn't sufficient. The server has the reference signal (it knows exactly what audio it sent to the browser) and can run a Normalized Least Mean Squares (NLMS) filter:

```
mic_input ─────────────┐
                        ▼
                   ┌─────────┐
reference ────────→│  NLMS   │──→ cleaned signal
(outAudio copy)    │ filter  │
                   └─────────┘
```

- Cross-correlation estimates speaker-to-mic delay (typically 20-80ms)
- NLMS subtracts the delayed reference from mic input
- Adaptive — converges as room acoustics change

### Phase 3: Neural Post-Filter (deferred)

RNNoise or WebRTC APM neural model — cleans residual echo that the linear NLMS filter misses. Pairs with Layer 3 noise suppression from Section 5.

---

## 8. Interrupt Handling (Barge-In) — The Hard Problem

### Current Implementation

Browser-side in `+page.svelte`:
- `stopTTSQueue()` at L1780 — increments `ttsCancelToken` (L1781), clears queue, pauses `currentAudio`
- `INTERRUPT_THRESHOLD = 0.02` at L1388 — very low, user must easily interrupt
- Voice monitor at L1603-1607 — detects RMS > threshold during `isSpeaking`, calls `stopSpeaking()`

### Target: Server-Driven 5-State Machine

```
                    ┌──────────┐
           ┌───────│   IDLE   │◄────────────────────────┐
           │       └────┬─────┘                         │
           │            │ session_start                  │ session_end
           │            ▼                                │
           │       ┌──────────┐                         │
           │       │LISTENING │◄─────────┐              │
           │       └────┬─────┘          │              │
           │            │ speech_end     │              │
           │            ▼                │              │
           │       ┌──────────┐          │              │
           │       │PROCESSING│          │ llm_done     │
           │       └────┬─────┘          │ (no speech)  │
           │            │ first_tts_byte │              │
           │            ▼                │              │
           │       ┌──────────┐          │              │
           │       │ SPEAKING │──────────┘              │
           │       └────┬─────┘                         │
           │            │ VAD detects speech             │
           │            ▼                                │
           │       ┌───────────────┐                    │
           └───────│ INTERRUPTING  │────────────────────┘
                   └───────────────┘
                     flush + restart
```

### Flush Sequence (the critical detail)

When the server detects speech during SPEAKING state:

1. **Server Silero VAD detects speech** during SPEAKING state (mic stays hot during playback)
2. **Server sends `interrupt_ack`** to browser
3. **Browser stops queueing audio** — drains PlaybackProcessor ring buffer (play what's already buffered, ~20-40ms tail)
4. **Browser AEC continues** removing echo from the tail audio during the adaptation window
5. **Server drains channels** — discard pending `outAudio` and `ttsText` (TTS sentences not yet synthesized)
6. **Server cancels runner context** — stops LLM generation. Same pattern as `CancelActive()` in `lane.go:437-459`:

```go
// In VoiceConn interrupt handler:
vc.lanes.CancelActive(agenthub.LaneMain)

// Drain pending TTS
for len(vc.ttsText) > 0 { <-vc.ttsText }
for len(vc.outAudio) > 0 { <-vc.outAudio }
```

7. **Server starts accumulating new speech** from `inAudio` → VAD → ASR
8. **Leaked echo** during 20-40ms window → Silero VAD may false-positive, but real speech resets the state naturally

**Phase 1 reality:** The user will hear a brief tail (~50ms) of the previous response during barge-in. This is acceptable for a desktop companion. Desktop builds get Silero VAD for reliable speech detection during playback; headless builds use RMS VAD which may false-trigger on echo. Phase 3 NLMS AEC makes this seamless everywhere.

---

## 9. Integration with Nebo

### Channel

```go
RunRequest{
    SessionKey: "companion-default",
    Prompt:     transcribedText,
    Origin:     tools.OriginUser,
    Channel:    "voice",
}
```

No runner changes needed. `Channel` is already a field on `RunRequest` (L88 in `runner.go`).

### Steering

Add a voice entry to the existing `channelTemplates` map in `steering/templates.go:20-25`:

```go
var channelTemplates = map[string]string{
    "telegram": "Responding via Telegram. Keep responses concise ...",
    "discord":  "Responding via Discord. Moderate length OK ...",
    "slack":    "Responding via Slack. Moderate length OK ...",
    "cli":      "Responding via CLI terminal. Plain text only ...",
    "voice":    "Responding via voice. Keep responses brief and conversational (1-3 sentences). Avoid code blocks, markdown, lists, and URLs — they don't render in speech. Use natural spoken language. Prefer concrete answers over hedging.",
}
```

This is an **edit** to an existing file — add one map entry. No new generator, no new function.

### Session: `companion-default`

Voice shares the **same companion session** as text chat and owner DMs. When you text about a project then switch to voice, Nebo remembers everything. The companion session is resolved via `GetOrCreateCompanionChat("companion-default")` — same as `cmd/nebo/agent.go:1735` (DM handler).

This is a convention, not a code change. The `RunRequest.SessionKey` is set to `"companion-default"` by the voice handler.

### Lane

`LaneMain` (concurrency 1) — serialized with text chat. A voice input and a text input cannot run simultaneously. The voice handler enqueues in main lane, same as web UI chat and owner DMs.

### Origin

`tools.OriginUser` — voice is direct user interaction. No tool restrictions. Same as web UI and CLI.

### Memory Extraction

Normal — not skipped. `SkipMemoryExtract` defaults to false. Voice conversations are remembered like any other conversation.

### Local/Offline Voice (Phase 1 default)

Phase 1 voice works fully offline:

| Component | Offline Provider | Reference |
|-----------|-----------------|-----------|
| ASR | `whisper-cli` (already primary) | `transcribe.go:212` — `transcribeLocal()` |
| TTS | macOS `say` / espeak / SAPI | `transcribe.go:66-71` (fallback chain), `tts.go:59-69` |
| LLM | Ollama (already supported) | Provider system handles routing |

ElevenLabs and cloud ASR are quality upgrades, not requirements. The fallback chain mirrors the existing pattern in `transcribe.go`: try cloud → fall back to local.

**Phase 1 voice works on an airplane.**

---

## 10. Gap Analysis & Code Disposition

| Component | Current State | Action | Effort | Details |
|-----------|--------------|--------|--------|---------|
| AudioWorklet Capture | None | **CREATE** `capture-processor.ts` | Medium | Float32→Int16LE conversion, postMessage to main thread |
| AudioWorklet Playback | None | **CREATE** `playback-processor.ts` | Medium | Ring buffer, zero-fill underruns |
| VoiceSession (browser) | 500-line state machine in +page.svelte | **CREATE** `VoiceSession.ts` | Medium | Binary WS client, WorkletNode wiring |
| WS Binary Transport | None | **CREATE** `voice/duplex.go` | High | readPump/writePump (reuse pattern from `realtime/client.go`), frame routing |
| Noise Gate | None | **CREATE** `voice/vad.go` | Low | Pure Go, RMS threshold + calibration |
| VAD (RMS fallback) | Browser-side (`+page.svelte:1583-1640`) | **CREATE** `voice/vad_rms.go` | Low | Permanent fallback for headless/no-CGO. Build tag: `!silero` |
| VAD (Silero desktop) | None | **CREATE** `voice/vad_silero.go` | Medium | ONNX runtime, `//go:build cgo && silero`. Desktop default. |
| Server ASR pipeline | `transcribeLocal()` + `convertToWav()` | **REUSE** from `transcribe.go:212,258` | Low | Call existing functions, add WAV writer |
| Server TTS pipeline | `serveElevenLabsTTS()` + `serveMacTTS()` | **REUSE** logic from `transcribe.go:119,75` | Low | Extract to callable functions |
| Sentence splitting | `feedTTSStream()` at `+page.svelte:1686-1706` | **MOVE** to Go, then **REMOVE** frontend | Low | Same regex, Go version |
| LLM integration | `runner.Run()` | **REUSE** unchanged | Zero | Already returns `<-chan StreamEvent` |
| Steering template | `channelTemplates` in `templates.go:20-25` | **EDIT** (add one entry) | Zero | Add `"voice"` key |
| Session convention | `companion-default` | **REUSE** unchanged | Zero | Convention only |
| Browser voice code | `+page.svelte:1369-1842` | **REMOVE** when duplex ships | — | Replaced by AudioWorklet + VoiceSession |
| HTTP voice endpoints | `/voice/transcribe`, `/voice/tts`, `/voice/voices` | **KEEP** | — | Still used by non-duplex features |
| Voice API functions | `speakTTS()`, `transcribeAudio()` in `api/index.ts:32,41` | **KEEP** | — | Still used by non-duplex TTS toggle |

---

## 11. Implementation Roadmap (4 Phases)

### Phase 1: PCM WebSocket MVP — "Push-to-talk without the button"

**Goal:** Mic stays open, server detects speech, transcribes, thinks, speaks. No manual record button.

**Create:**
- `internal/voice/duplex.go` — VoiceConn struct, readPump/writePump, channel architecture
- `internal/voice/vad.go` — NoiseGate, VAD interface, `rms()` utility
- `internal/voice/vad_rms.go` — RMS VAD (pure Go, `//go:build !silero`, headless fallback)
- `internal/voice/vad_silero.go` — Silero ONNX VAD (`//go:build cgo && silero`, desktop default)
- `app/src/lib/voice/capture-processor.ts` — AudioWorklet Float32→Int16LE
- `app/src/lib/voice/playback-processor.ts` — AudioWorklet ring buffer playback
- `app/src/lib/voice/VoiceSession.ts` — Binary WS client, WorkletNode lifecycle

**Reuse (edit):**
- `transcribe.go` — extract `transcribeLocal()` and ElevenLabs/macOS TTS logic for pipeline use
- `server.go` — add `/ws/voice` route alongside existing voice routes
- `steering/templates.go` — add `"voice"` entry to `channelTemplates` map

**Remove:**
- Nothing yet in Phase 1. Browser half-duplex code stays until Phase 1 is stable.

**Latency reality: 1500-3000ms from end-of-speech to first audio.**

| Stage | Duration | Notes |
|-------|----------|-------|
| Speech accumulation + silence hangover | 300-800ms | VAD hangover before finalizing |
| whisper-cli batch transcription | 500-2000ms | Depends on utterance length, model size |
| LLM TTFT (Janus/local) | 200-1000ms | First token from provider |
| TTS generation (ElevenLabs/say) | 300-800ms | Per-sentence, non-streaming |
| WS frame + playback start | ~20ms | Negligible |
| **Total** | **~1.5-3s** | |

This is walkie-talkie, not phone call. Acceptable for a desktop companion that does real work (writes emails, searches files, schedules meetings).

**UX mitigation during the gap:** The browser shows ASR partial text ("I heard you say...") via `transcript` control messages, then LLM streaming text via `llm_text` messages. Audio follows as the third layer. It's a text waterfall that becomes speech — not silence.

**Works fully offline** with whisper-cli + macOS `say` + Ollama.

### Phase 2: Streaming ASR + Opus — reduces latency by ~1s

**Goal:** Reduce latency by ~1s. Streaming ASR overlaps with speech (text arrives during speech, not after). Opus codec cuts bandwidth 32x.

**One CGO dep lands:**
- `github.com/hraban/opus` — Opus codec, 32x bandwidth reduction

**Create:**
- `internal/voice/opus.go` — Encoder/decoder (build tagged `//go:build opus`)

**Edit:**
- `duplex.go` — add Opus encode/decode in pipeline, codec negotiation
- Streaming ASR integration (Deepgram or Google Speech-to-Text WebSocket)

**Remove:**
- Nothing — RMS VAD stays as permanent headless fallback

### Phase 3: Streaming TTS + Low Latency — gets to <1s

**Goal:** First audio byte arrives during LLM generation, not after.

**Create/Edit:**
- ElevenLabs streaming WebSocket API integration in ttsLoop
- Server-side NLMS echo cancellation (reference signal subtraction)
- Move `feedTTSStream()` sentence splitting fully to server (it's already there from Phase 1), **remove** the frontend version when browser half-duplex code is deleted

**Remove:**
- `+page.svelte` lines 1369-1842 — browser voice state machine, VAD, TTS queue, sentence splitting. Replaced by AudioWorklet + VoiceSession + server-driven state.
- `feedTTSStream()`, `flushTTSBuffer()`, `playNextTTS()`, `stopTTSQueue()` — all moved to server

**Target latency: <1000ms** (streaming ASR + streaming TTS overlap with LLM TTFT)

### Phase 4: Production Hardening

- WebSocket reconnection with session resumption (buffer audio during reconnect)
- Graceful codec degradation (Opus → PCM fallback if CGO unavailable)
- Rate limiting on `/ws/voice` (prevent abuse)
- Metrics: end-to-end latency histogram, ASR/TTS duration tracking
- Desktop app microphone permission prompt (macOS TCC, Windows privacy settings)
- Voice mode UI polish: waveform visualization, state indicators, volume meter
- Multi-language ASR (whisper-cli `--language auto`)

---

## 12. Reference Implementation Snippets

### Go `VoiceConn` Struct Skeleton

```go
package voice

import (
    "context"
    "encoding/json"
    "sync"
    "time"

    "github.com/gorilla/websocket"
    "github.com/neboloop/nebo/internal/agent/ai"
    "github.com/neboloop/nebo/internal/agent/runner"
    "github.com/neboloop/nebo/internal/agent/tools"
    "github.com/neboloop/nebo/internal/agenthub"
)

// VoiceConn manages a full-duplex voice WebSocket session.
// Modeled on readPump/writePump pattern from realtime/client.go and agenthub/hub.go.
type VoiceConn struct {
    conn   *websocket.Conn
    runner *runner.Runner
    lanes  *agenthub.LaneManager

    // Audio pipeline channels
    inAudio  chan []int16  // mic → server (PCM frames)
    asrText  chan string   // ASR → LLM
    ttsText  chan string   // LLM → TTS (sentences)
    outAudio chan []byte   // TTS → speaker (encoded frames)

    // Control channel for JSON messages to browser
    controlOut chan []byte

    // Pipeline components
    noiseGate *NoiseGate
    vad       VAD // interface: RMSVAD (Phase 1) or SileroVAD (Phase 2)

    // State
    state      VoiceState
    stateMu    sync.RWMutex
    cancelFunc context.CancelFunc

    // Config
    sampleRate int    // 48000 (capture) or 16000 (ASR)
    codec      string // "pcm" or "opus"
}

type VoiceState string

const (
    StateIdle         VoiceState = "idle"
    StateListening    VoiceState = "listening"
    StateProcessing   VoiceState = "processing"
    StateSpeaking     VoiceState = "speaking"
    StateInterrupting VoiceState = "interrupting"
)

// VAD interface — swappable between RMS (Phase 1) and Silero (Phase 2)
type VAD interface {
    ProcessFrame(frame []int16) bool
    Reset()
}

func NewVoiceConn(conn *websocket.Conn, r *runner.Runner, lanes *agenthub.LaneManager) *VoiceConn {
    return &VoiceConn{
        conn:       conn,
        runner:     r,
        lanes:      lanes,
        inAudio:    make(chan []int16, 50),
        asrText:    make(chan string, 1),
        ttsText:    make(chan string, 10),
        outAudio:   make(chan []byte, 50),
        controlOut: make(chan []byte, 20),
        noiseGate:  NewNoiseGate(),
        vad:        NewDefaultVAD(), // Silero on desktop, RMS on headless (build-tagged)
        state:      StateIdle,
        sampleRate: 48000,
        codec:      "pcm",
    }
}

// Serve runs the voice connection — starts all goroutines.
func (vc *VoiceConn) Serve(ctx context.Context) {
    ctx, vc.cancelFunc = context.WithCancel(ctx)

    go vc.readPump(ctx)
    go vc.writePump(ctx)
    go vc.asrLoop(ctx)
    go vc.llmLoop(ctx)
    go vc.ttsLoop(ctx)

    <-ctx.Done()
    vc.conn.Close()
}

// readPump reads from WebSocket — binary frames to inAudio, text to control handler.
func (vc *VoiceConn) readPump(ctx context.Context) {
    defer vc.cancelFunc()

    vc.conn.SetReadLimit(64 * 1024) // 64KB max (audio frames are small)
    vc.conn.SetReadDeadline(time.Now().Add(60 * time.Second))
    vc.conn.SetPongHandler(func(string) error {
        vc.conn.SetReadDeadline(time.Now().Add(60 * time.Second))
        return nil
    })

    for {
        msgType, data, err := vc.conn.ReadMessage()
        if err != nil {
            return
        }
        vc.conn.SetReadDeadline(time.Now().Add(60 * time.Second))

        switch msgType {
        case websocket.BinaryMessage:
            // Decode PCM Int16LE frames
            frame := decodePCM(data)
            select {
            case vc.inAudio <- frame:
            default:
                // Drop frame if pipeline is backed up
            }
        case websocket.TextMessage:
            vc.handleControl(data)
        }
    }
}

// writePump writes to WebSocket — binary audio out + JSON control messages.
func (vc *VoiceConn) writePump(ctx context.Context) {
    ticker := time.NewTicker(30 * time.Second)
    defer func() {
        ticker.Stop()
        vc.conn.Close()
    }()

    for {
        select {
        case <-ctx.Done():
            return
        case audio := <-vc.outAudio:
            vc.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
            if err := vc.conn.WriteMessage(websocket.BinaryMessage, audio); err != nil {
                return
            }
        case control := <-vc.controlOut:
            vc.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
            if err := vc.conn.WriteMessage(websocket.TextMessage, control); err != nil {
                return
            }
        case <-ticker.C:
            vc.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
            if err := vc.conn.WriteMessage(websocket.PingMessage, nil); err != nil {
                return
            }
        }
    }
}

// sendControl sends a JSON control message to the browser.
func (vc *VoiceConn) sendControl(msgType string, data map[string]any) {
    data["type"] = msgType
    if b, err := json.Marshal(data); err == nil {
        select {
        case vc.controlOut <- b:
        default:
        }
    }
}

func (vc *VoiceConn) handleControl(data []byte) {
    var msg struct {
        Type string `json:"type"`
    }
    if json.Unmarshal(data, &msg) != nil {
        return
    }

    switch msg.Type {
    case "session_start":
        vc.stateMu.Lock()
        vc.state = StateListening
        vc.stateMu.Unlock()
        vc.sendControl("session_started", map[string]any{
            "session_id": "companion-default",
        })
    case "session_end":
        vc.cancelFunc()
    }
}
```

### WebSocket Handler Registration in `server.go`

```go
// DuplexHandler returns an HTTP handler that upgrades to a voice WebSocket.
func DuplexHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
    upgrader := websocket.Upgrader{
        ReadBufferSize:  4096,
        WriteBufferSize: 4096,
        CheckOrigin: func(r *http.Request) bool {
            origin := r.Header.Get("Origin")
            return origin == "" || middleware.IsLocalhostOrigin(origin)
        },
    }

    return func(w http.ResponseWriter, r *http.Request) {
        conn, err := upgrader.Upgrade(w, r, nil)
        if err != nil {
            return
        }

        vc := NewVoiceConn(conn, svcCtx.Runner, svcCtx.Lanes)
        go vc.Serve(r.Context())
    }
}
```

---

## 13. Go Libraries & Latency Targets

### Libraries

| Library | Phase | CGO | Already in go.mod | Purpose |
|---------|-------|-----|-------------------|---------|
| `github.com/gorilla/websocket` | 1 | No | **Yes** | WS binary transport |
| `whisper-cli` (external binary) | 1 | N/A | **Yes** (called via exec) | Batch ASR |
| `github.com/yalue/onnxruntime_go` | 1 | **Yes** (desktop only) | No | Silero VAD inference. Build tag: `cgo && silero` |
| `github.com/hraban/opus` | 2 | **Yes** | No | Opus encode/decode |
| Deepgram Go SDK | 2 | No | No | Streaming ASR |
| ElevenLabs WS API | 3 | No | No | Streaming TTS |

### Phase 1 Latency Budget (honest)

| Stage | Min | Max | Notes |
|-------|-----|-----|-------|
| Speech accumulation + silence hangover | 300ms | 800ms | RMS VAD, 300ms hangover |
| whisper-cli batch transcription | 500ms | 2000ms | ~3s utterance on base.en model |
| LLM TTFT (Janus or Ollama) | 200ms | 1000ms | Depends on provider, prompt length |
| TTS generation (ElevenLabs or say) | 300ms | 800ms | Per-sentence, non-streaming |
| WS frame + AudioWorklet playback | ~20ms | ~20ms | Negligible |
| **Total** | **~1.5s** | **~3s** | |

**Phase 1 UX:** Show ASR text immediately via `transcript` message, then LLM streaming text via `llm_text` messages. Audio is the third layer — not the only feedback channel. The user sees their words confirmed, then sees Nebo thinking, then hears the response.

### Phase 3 Target: <1000ms

| Optimization | Savings |
|-------------|---------|
| Streaming ASR (Deepgram) — text during speech | −500-1500ms (overlaps with speech) |
| Streaming TTS (ElevenLabs WS) — audio during LLM | −300-800ms (overlaps with LLM generation) |
| Silero VAD — faster speech endpoint detection | −100-200ms (tighter hangover) |
| Opus — smaller frames, less WS overhead | −10-50ms |
| **Net result** | **<1000ms end-of-speech to first audio** |

---

## 14. Key Files Reference — Reuse / Create / Remove Matrix

### Reuse (edit existing files)

| File | What to edit | Phase |
|------|-------------|-------|
| `internal/voice/transcribe.go` | Extract `transcribeLocal()` (L212) and `serveElevenLabsTTS()` (L119) into callable functions for pipeline use. HTTP handlers stay intact. | 1 |
| `internal/server/server.go` | Add `r.Get("/ws/voice", voice.DuplexHandler(svcCtx))` near L204-205 | 1 |
| `internal/agent/steering/templates.go` | Add `"voice"` entry to `channelTemplates` map at L20-25 | 1 |
| `app/src/routes/(app)/agent/+page.svelte` | Wire VoiceSession into existing voice toggle button (replace enterVoiceMode/exitVoiceMode) | 1 |

### Create (new files)

| File | Purpose | Phase |
|------|---------|-------|
| `internal/voice/duplex.go` | VoiceConn, readPump/writePump, DuplexHandler, channel architecture | 1 |
| `internal/voice/vad.go` | VAD interface, NoiseGate, `rms()` utility | 1 |
| `internal/voice/vad_rms.go` | RMS VAD (`//go:build !silero`). Permanent fallback for headless/no-CGO. | 1 |
| `internal/voice/vad_silero.go` | Silero ONNX VAD (`//go:build cgo && silero`). Desktop default. | 1 |
| `internal/voice/opus.go` | Opus encoder/decoder (build tagged `//go:build opus`) | 2 |
| `app/src/lib/voice/capture-processor.ts` | AudioWorklet: Float32→Int16LE, postMessage | 1 |
| `app/src/lib/voice/playback-processor.ts` | AudioWorklet: ring buffer playback, zero-fill | 1 |
| `app/src/lib/voice/VoiceSession.ts` | Binary WS client, WorkletNode lifecycle, thin state | 1 |

### Remove (when full-duplex replaces half-duplex)

| Code | Location | When | Replaced by |
|------|----------|------|-------------|
| Voice state machine | `+page.svelte:1369-1842` | Phase 3 | VoiceSession.ts + server-driven state |
| Voice monitor (RMS loop) | `+page.svelte:1579-1640` | Phase 3 | Server-side VAD in vad.go |
| Browser TTS queue | `+page.svelte:1686-1795` | Phase 3 | Server-side sentence splitting + ttsLoop |
| `feedTTSStream()` | `+page.svelte:1686-1706` | Phase 3 | `extractSentences()` in duplex.go |
| `flushTTSBuffer()` | `+page.svelte:1708-1719` | Phase 3 | `flushSentenceBuffer()` in duplex.go |
| `playNextTTS()` | `+page.svelte:1722-1777` | Phase 3 | ttsLoop goroutine in duplex.go |
| `stopTTSQueue()` | `+page.svelte:1780-1795` | Phase 3 | Interrupt handler in duplex.go |
| `speakText()` | `+page.svelte:1797-1842` | Phase 3 | Fully replaced by streaming TTS |

### Keep unchanged

| File | Why |
|------|-----|
| `internal/voice/transcribe.go` HTTP handlers | `/voice/transcribe`, `/voice/tts`, `/voice/voices` still used by non-voice-mode features (TTS toggle in text chat) |
| `internal/agent/tools/tts.go` | System-native TTS agent tool — independent of voice mode |
| `internal/agent/runner/runner.go` | `Run()` unchanged — voice is just another channel |
| `internal/agent/ai/provider.go` | StreamEvent types unchanged |
| `internal/agenthub/lane.go` | Lane system unchanged — voice enqueues in LaneMain |
| `app/src/lib/api/index.ts` | `speakTTS()`, `transcribeAudio()` stay for non-duplex use |
