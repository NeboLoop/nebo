# Nebo Voice System - Comprehensive Logic Deep-Dive

Source: `/Users/almatuck/workspaces/nebo/nebo/internal/voice/`

This document captures every detail of the Go voice subsystem: structs, function signatures, constants, algorithms, audio processing parameters, and control flow.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [ASR (Automatic Speech Recognition)](#2-asr-automatic-speech-recognition)
3. [TTS (Text-to-Speech)](#3-tts-text-to-speech)
4. [VAD (Voice Activity Detection)](#4-vad-voice-activity-detection)
5. [Wakeword Detection](#5-wakeword-detection)
6. [Duplex Audio Pipeline](#6-duplex-audio-pipeline)
7. [Audio Codecs and Formats](#7-audio-codecs-and-formats)
8. [Model Management](#8-model-management)
9. [WebSocket Transport Protocol](#9-websocket-transport-protocol)
10. [Voice Handler HTTP Endpoints](#10-voice-handler-http-endpoints)
11. [ONNX Runtime Integration](#11-onnx-runtime-integration)
12. [Phonemization Pipeline](#12-phonemization-pipeline)
13. [Error Handling and Fallback Behavior](#13-error-handling-and-fallback-behavior)

---

## 1. System Overview

The voice system is a full-duplex, bidirectional audio pipeline that runs over WebSocket (or NeboLoop comms relay). It comprises five concurrent goroutines connected by channels:

```
Browser/Phone
    |
    v
[ReadPump] --(audioCh: []byte)--> [asrLoop] --(textCh: string)--> [llmLoop] --(ttsCh: string)--> [ttsLoop] --(audioOutCh: []byte)--> [WritePump]
    ^                                                                                                                                      |
    |______________________________________________________________________________________________________________________________________|
                                                WebSocket / Comms Transport
```

### Build Tags

The system uses conditional compilation:
- **`cgo` build tag**: Enables native whisper.cpp via CGO, Silero VAD via ONNX, Kokoro TTS via ONNX, and the phonemization pipeline.
- **`!cgo` build tag**: Falls back to whisper-cli subprocess, RMS-based VAD, and delegates TTS to ElevenLabs/macOS `say`.

### Files

| File | Build Tag | Purpose |
|------|-----------|---------|
| `duplex.go` | none | Core VoiceConn, state machine, DuplexHandler, WakeWordHandler, serve loop |
| `pipeline.go` | none | asrLoop, llmLoop, ttsLoop, sentence splitting, audio conversion, resampling |
| `transport.go` | none | VoiceTransport interface |
| `transport_ws.go` | none | wsTransport (direct WebSocket) |
| `transport_comms.go` | none | CommsTransport (NeboLoop relay) |
| `vad.go` | none | VAD interface, NoiseGate, rms(), writeWavFile(), decodePCM() |
| `vad_rms.go` | `!cgo` | RMSVAD pure-Go fallback |
| `vad_silero.go` | `cgo` | SileroVAD via ONNX, RMSVAD duplicate, ONNX runtime init |
| `asr_whisper.go` | `cgo` | Embedded whisper.cpp via CGO |
| `asr_fallback.go` | `!cgo` | TranscribePCM/TranscribeFile via whisper-cli subprocess |
| `tts_kokoro.go` | `cgo` | Kokoro ONNX TTS engine, voice embedding loader, NPY parser |
| `tts_fallback.go` | `!cgo` | SynthesizeSpeechForDuplex delegates to SynthesizeSpeech |
| `transcribe.go` | none | HTTP TranscribeHandler, TTSHandler, VoicesHandler, ElevenLabs/macOS TTS |
| `phonemize.go` | `cgo` | English text-to-phoneme pipeline for Kokoro |
| `wakeword.go` | none | WakeWordDetector, Levenshtein distance |
| `models.go` | none | Model manifest, download, cache, status |
| `duplex_test.go` | none | Tests for auth flows |

---

## 2. ASR (Automatic Speech Recognition)

### 2.1 Embedded Whisper.cpp (cgo build)

**File:** `asr_whisper.go` (build tag: `cgo`)

#### CGO Bindings

```c
#cgo CFLAGS: -I${SRCDIR}/../../third_party/whisper/include
#cgo LDFLAGS: -L${SRCDIR}/../../third_party/whisper/lib -lwhisper -lggml -lggml-base -lggml-cpu -lm -lstdc++
#cgo darwin LDFLAGS: -lggml-metal -lggml-blas -framework Accelerate -framework Foundation -framework Metal -framework MetalKit
```

On macOS (darwin), links against Metal and Accelerate frameworks for GPU-accelerated inference.

A static C helper `nebo_default_params()` wraps `whisper_full_default_params(WHISPER_SAMPLING_GREEDY)` because Go/CGO cannot easily handle C functions that return structs by value.

#### Global State

```go
var (
    whisperCtx  *C.struct_whisper_context  // Loaded model context (singleton)
    whisperOnce sync.Once                   // Ensures single initialization
    whisperErr  error                       // Captures init error
    whisperMu   sync.Mutex                  // Serializes inference calls
)
```

The whisper context is a process-wide singleton, loaded once via `sync.Once`. All inference calls are serialized via `whisperMu` because whisper.cpp is not thread-safe for concurrent calls on the same context.

#### `loadWhisperModel() error`

- Called lazily on first transcription.
- Uses `whisper_context_default_params()` for context creation params.
- Model path: `ModelsDir()/ggml-base.en.bin`
- Calls `whisper_init_from_file_with_params(cPath, params)`.
- Returns error if model file missing or loading fails.

#### `TranscribePCM(pcm []float32) (string, error)`

- Input: float32 PCM samples normalized to [-1.0, 1.0], 16kHz mono.
- Calls `loadWhisperModel()` (lazy init).
- Returns empty string for empty input.
- Acquires `whisperMu` lock.
- Configures inference params via `nebo_default_params()`:
  - `language = "en"`
  - `n_threads = 4`
  - `no_timestamps = true`
  - `print_progress = false`
  - `print_realtime = false`
  - `print_special = false`
  - `print_timestamps = false`
- Calls `whisper_full(whisperCtx, params, &pcm[0], len(pcm))`.
- Extracts segments via `whisper_full_n_segments()` and `whisper_full_get_segment_text()`.
- Joins segments with spaces, trims whitespace.

#### `TranscribeFile(path string) (string, error)`

- Reads WAV file bytes from disk.
- Calls `wavToFloat32(data)` to extract float32 samples.
- Delegates to `TranscribePCM()`.

#### `wavToFloat32(data []byte) ([]float32, error)`

- Scans raw bytes for the `"data"` chunk marker.
- Skips 8 bytes after marker (4 for "data" + 4 for size).
- Reads Int16LE samples, converts to float32 by dividing by 32768.0.

### 2.2 Fallback: whisper-cli Subprocess (!cgo build)

**File:** `asr_fallback.go` (build tag: `!cgo`)

#### `TranscribePCM(pcm []float32) (string, error)`

- Converts float32 to int16 with clamping at [-1.0, 1.0].
- Writes to temp WAV file via `writeWavFile()`.
- Delegates to `TranscribeFile()`.

#### `TranscribeFile(path string) (string, error)`

- Model path: `ModelsDir()/ggml-base.en.bin`.
- If model file missing but `whisperModelData` (compile-time injected byte slice) is non-empty, extracts embedded model to disk.
- Tries `exec.LookPath("whisper-cli")` to find the binary.
- If found and model exists, delegates to `transcribeLocal(path, modelPath)`.
- Returns error if neither whisper-cli nor model is available.

Note: `whisperModelData` is referenced but never defined in the voice package. It is expected to be provided via a build-tag-specific file or link-time injection for headless builds.

### 2.3 Local Whisper CLI

**File:** `transcribe.go`

#### `transcribeLocal(audioPath, modelPath string) (string, error)`

- Locates `whisper-cli` via `exec.LookPath`.
- Executes: `whisper-cli --model <modelPath> --file <audioPath> --no-timestamps --language en --threads 4`
- Parses stdout: filters out lines starting with `whisper_`, `main:`, `ggml_`, `system_info:`, `output_`.
- Strips timestamp brackets like `[00:00:00.000 --> 00:00:05.000]`.
- Joins remaining lines with spaces.

### 2.4 OpenAI Whisper API Fallback

**File:** `transcribe.go`

Used as a last resort in `TranscribeHandler` when local transcription fails:
- Requires `OPENAI_API_KEY` environment variable.
- Sends multipart POST to `https://api.openai.com/v1/audio/transcriptions`.
- Model: `whisper-1`.
- Max upload size: 25MB.

### 2.5 Audio Conversion

#### `convertToWav(inputPath string) (string, error)`

- Uses `ffmpeg` subprocess.
- Command: `ffmpeg -i <input> -ar 16000 -ac 1 -y <output.wav>`
- Converts webm/ogg/m4a to 16kHz mono WAV.

---

## 3. TTS (Text-to-Speech)

### 3.1 Kokoro ONNX TTS (cgo build)

**File:** `tts_kokoro.go` (build tag: `cgo`)

#### Structs

```go
type kokoroTTS struct {
    session *ort.DynamicAdvancedSession
    voices  map[string][]float32  // voice name -> flat embedding [510*256]
}

var globalKokoro *kokoroTTS

const styleFrameSize = 256  // embedding dimension per frame
```

Global singleton, lazily initialized.

#### `SynthesizeSpeechForDuplex(text, voice string) ([]byte, error)`

- Entry point for duplex pipeline TTS.
- Calls `getKokoro()` for lazy initialization.
- Returns raw 24kHz Int16LE PCM bytes.

#### `getKokoro() (*kokoroTTS, error)`

- Returns cached `globalKokoro` if already initialized.
- Checks for model files: `ModelsDir()/kokoro-v1.0.onnx` and `ModelsDir()/voices-v1.0.bin`.
- Returns `nil, nil` (not an error) if models are missing.
- Calls `newKokoroTTS()` on first use, caches globally.

#### `newKokoroTTS(modelPath, voicesPath string) (*kokoroTTS, error)`

- Calls `initONNXRuntime()` (shared with Silero VAD).
- Creates ONNX session with:
  - Inputs: `["tokens", "style", "speed"]`
  - Outputs: `["audio"]`
- Loads voice embeddings from ZIP archive via `loadVoices()`.

#### `(k *kokoroTTS) synthesize(text, voice string) ([]byte, error)`

**Full synthesis pipeline:**

1. **Voice selection**: Default voice is `"af_heart"`. Falls back to first available voice if requested voice not found.

2. **Phonemization**: Calls `Phonemize(text)` to convert English text to Kokoro token IDs (see Section 12).

3. **Style frame selection**: Voice packs have shape `(510, 1, 256)`. The frame index is chosen based on token count:
   ```
   frameIdx = min(len(tokens), numFrames - 1)
   styleFrame = pack[frameIdx*256 : (frameIdx+1)*256]
   ```

4. **Tensor creation**:
   - `tokens`: shape `[1, len(tokens)]`, dtype `int64`
   - `style`: shape `[1, 256]`, dtype `float32`
   - `speed`: shape `[1]`, dtype `float32`, value `1.0`

5. **Inference**: Runs with `nil` output tensor (dynamic allocation by ONNX runtime).

6. **Output conversion**: Extracts float32 audio, clamps to [-1.0, 1.0], converts to Int16LE PCM bytes.

7. **Output format**: Raw 24kHz mono Int16LE PCM.

#### Voice Embedding Loading

##### `loadVoices(path string) (map[string][]float32, error)`

- Opens ZIP archive at `path`.
- Iterates entries, looking for `.npy` files.
- Voice name = filename without `.npy` extension.
- Parses each NPY file via `parseNPY()`.
- Returns error if no voices found.

##### `parseNPY(data []byte) ([]float32, error)`

- Validates magic bytes: `\x93NUMPY`.
- Supports NPY v1 (2-byte header length at offset 8) and v2+ (4-byte header length at offset 8).
- Reads float32 values in little-endian format starting after the header.

#### File Paths

```go
func kokoroModelPath() string { return filepath.Join(ModelsDir(), "kokoro-v1.0.onnx") }
func kokoroVoicesPath() string { return filepath.Join(ModelsDir(), "voices-v1.0.bin") }
```

### 3.2 ElevenLabs Cloud TTS

**File:** `transcribe.go`

#### Voice Map

```go
var elevenLabsVoices = map[string]string{
    "rachel": "21m00Tcm4TlvDq8ikWAM",
    "domi":   "AZnzlk1XvdvUeBnXmlld",
    "bella":  "EXAVITQu4vr4xnSDxMaL",
    "antoni": "ErXwobaYiN019PkySvjV",
    "elli":   "MF3mGyEYCl7XYWbV9V6O",
    "josh":   "TxGEqnHWrfWFTfGW9XjX",
    "arnold": "VR6AewLTigWG4xSOukaG",
    "adam":   "pNInz6obpgDQGcFmaJgB",
    "sam":    "yoZ06aMxZJJ28mfd3POQ",
}
```

#### `elevenLabsTTS(text, voice string, speed float64, apiKey string) ([]byte, error)`

- Resolves voice name to ID (default: "rachel").
- If voice string is not in the map, treats it as a raw voice ID.
- Default speed: 1.0.
- API request body:
  ```json
  {
    "text": "<text>",
    "model_id": "eleven_turbo_v2_5",
    "voice_settings": {
      "stability": 0.5,
      "similarity_boost": 0.75,
      "speed": <speed>
    }
  }
  ```
- POST to `https://api.elevenlabs.io/v1/text-to-speech/<voiceID>`.
- Headers: `Content-Type: application/json`, `xi-api-key: <apiKey>`, `Accept: audio/mpeg`.
- Returns `nil, nil` on any failure (silent fallback, never returns errors).
- Output format: MP3 bytes.

### 3.3 macOS `say` Fallback

**File:** `transcribe.go`

#### `macTTS(text, voice string, speed float64) ([]byte, error)`

- Creates temp file with `.aiff` extension.
- Default voice: `"Shelley (English (US))"`.
- Rate calculation: `175 * speed` words per minute (default ~175 wpm).
- Command: `say -v <voice> -o <tmpPath> [-r <rate>] <text>`.
- Output format: AIFF bytes.

### 3.4 Fallback TTS for !cgo Duplex

**File:** `tts_fallback.go` (build tag: `!cgo`)

```go
func SynthesizeSpeechForDuplex(text, voice string) ([]byte, error) {
    data, _, err := SynthesizeSpeech(text, voice, 1.0)
    return data, err
}
```

Delegates to `SynthesizeSpeech()` which tries ElevenLabs then macOS `say`. Returns MP3 or AIFF -- note that these encoded formats are **not usable** in the duplex pipeline (the pipeline's `audioToPCM()` rejects them).

### 3.5 Unified SynthesizeSpeech

**File:** `transcribe.go`

```go
func SynthesizeSpeech(text, voice string, speed float64) ([]byte, string, error)
```

- Returns `(audioData, contentType, error)`.
- Priority: ElevenLabs (`ELEVENLABS_API_KEY`) -> macOS `say` (darwin only).
- Content types: `"audio/mpeg"` (ElevenLabs) or `"audio/aiff"` (macOS).

---

## 4. VAD (Voice Activity Detection)

### 4.1 VAD Interface

**File:** `vad.go`

```go
type VAD interface {
    IsSpeech(pcm []int16) bool
    Reset()
}
```

### 4.2 NoiseGate

**File:** `vad.go`

```go
type NoiseGate struct {
    threshold   float64
    calibrated  bool
    calibFrames int
    calibSum    float64
}
```

#### `NewNoiseGate() *NoiseGate`

Returns a zero-value gate (auto-calibrates on use).

#### `(g *NoiseGate) Filter(pcm []int16) []int16`

- **Calibration phase** (first 20 frames):
  - Accumulates RMS values.
  - After 20 frames: `threshold = avg * 2.5` (2.5x the ambient floor).
  - Minimum threshold: `0.005`.
  - Returns `nil` during calibration (suppresses all audio).
- **Active phase**: Returns `nil` if `rms(pcm) < threshold`, otherwise returns the PCM unchanged.

### 4.3 RMS Computation

**File:** `vad.go`

```go
func rms(pcm []int16) float64
```

- Normalizes each sample by dividing by 32768.0 (to [-1.0, 1.0] range).
- Computes `sqrt(sum(v^2) / N)`.
- Returns 0 for empty input.

### 4.4 RMSVAD (Pure Go Fallback)

**File:** `vad_rms.go` (build tag: `!cgo`) and duplicated in `vad_silero.go` (build tag: `cgo`)

```go
type RMSVAD struct {
    speechThreshold  float64  // RMS level to start speech
    silenceThreshold float64  // RMS level to end speech
    speechFrames     int      // consecutive speech frames needed to trigger
    silenceFrames    int      // consecutive silence frames needed to end
    inSpeech         bool
    speechCount      int
    silenceCount     int
}
```

#### Default Parameters

| Build | speechThreshold | silenceThreshold | speechFrames | silenceFrames |
|-------|-----------------|------------------|--------------|---------------|
| `!cgo` | 0.015 | 0.008 | 3 (~60ms) | 30 (~600ms) |
| `cgo` | 0.006 | 0.003 | 3 (~60ms) | 10 (~200ms) |

The `cgo` build uses lower thresholds because it is tuned for browser audio with `noiseSuppression: true` (very clean but quiet signal).

#### Hysteresis Algorithm

```
IsSpeech(pcm):
    level = rms(pcm)

    IF inSpeech:
        IF level < silenceThreshold:
            silenceCount++, speechCount=0
            IF silenceCount >= silenceFrames: inSpeech=false, silenceCount=0
        ELSE:
            silenceCount=0

    ELSE (not in speech):
        IF level >= speechThreshold:
            speechCount++, silenceCount=0
            IF speechCount >= speechFrames: inSpeech=true, speechCount=0
        ELSE:
            speechCount=0

    return inSpeech
```

Uses dual thresholds (speech onset threshold is higher than silence threshold) to prevent flickering.

#### `Reset()`

Clears `inSpeech`, `speechCount`, `silenceCount` to zero.

### 4.5 SileroVAD (ONNX Model)

**File:** `vad_silero.go` (build tag: `cgo`)

```go
type SileroVAD struct {
    session   *ort.DynamicAdvancedSession
    state     *ort.Tensor[float32]  // hidden state [2, 1, 128]
    threshold float32               // 0.25
    inSpeech  bool
    broken    bool     // set true on first inference error
    maxProb   float32  // diagnostic: highest probability seen
}
```

**NOTE:** As of the current code, `NewDefaultVAD()` in the `cgo` build returns an RMSVAD, **not** SileroVAD. A TODO comment reads: "Re-enable Silero VAD once ONNX model output is debugged (returns near-zero probs)."

```go
func NewDefaultVAD() VAD {
    voiceLog.Info("using RMS VAD")
    return newRMSVAD()
}
```

#### `newSileroVAD(modelPath string) (*SileroVAD, error)`

- Initializes ONNX Runtime via `initONNXRuntime()`.
- Creates hidden state tensor: shape `[2, 1, 128]`, zero-initialized float32.
- Creates ONNX session with:
  - Inputs: `["input", "state", "sr"]`
  - Outputs: `["output", "stateN"]`
- Threshold: `0.25`.

#### `(v *SileroVAD) IsSpeech(pcm []int16) bool`

- **Broken mode fallback**: If `v.broken` is true, uses `rms(pcm) >= 0.006`.
- Converts int16 to float32 (divide by 32768.0).
- Creates tensors:
  - `input`: shape `[1, len(pcm)]`, float32
  - `sr` (sample rate): shape `[1]`, int64 value `16000`
  - `output`: shape `[1, 1]`, float32 (pre-allocated)
  - `stateN`: shape `[2, 1, 128]`, float32 (pre-allocated)
- Runs inference: `session.Run(inputs, outputs)`.
- Copies new state data back to `v.state` for next call.
- Returns `prob >= threshold` where `prob = output[0]`.
- Logs when `maxProb` exceeds 0.05 (diagnostic).
- On any tensor/inference error: sets `v.broken = true`, falls back to RMS.

#### `(v *SileroVAD) Reset()`

Zeroes the hidden state tensor and clears `inSpeech`.

#### Model/Runtime Paths

```go
func sileroModelPath() string { return filepath.Join(ModelsDir(), "silero_vad.onnx") }
```

```go
func onnxRuntimeLibPath() string
```

Priority for ONNX Runtime shared library:
1. Downloaded in `ModelsDir()` (e.g., `libonnxruntime.arm64.dylib`)
2. Bundled at `<exe>/../Frameworks/libonnxruntime.dylib` (macOS app bundle)
3. System: `/usr/lib/libonnxruntime.so` (Linux), `onnxruntime.dll` (Windows)

---

## 5. Wakeword Detection

**File:** `wakeword.go`

### Structs

```go
type WakeWordDetector struct {
    vad           VAD
    gate          *NoiseGate
    onDetect      func()
    speechBuf     []int16
    inSpeech      bool
    silenceFrames int
}
```

### Constants

```go
const (
    wakeMinSamples = 16000 * 3 / 10   // 4800 samples = 0.3s at 16kHz
    wakeMaxSamples = 16000 * 2         // 32000 samples = 2.0s at 16kHz
    wakeSilenceMax = 15                // ~300ms at 20ms frames
)
```

### `NewWakeWordDetector(onDetect func()) *WakeWordDetector`

Creates a detector with its own VAD instance (`NewDefaultVAD()`) and NoiseGate.

### `(w *WakeWordDetector) Feed(pcm []int16) bool`

**Per-frame processing:**

1. Apply noise gate. If gated (nil), return false.
2. Run VAD on filtered audio.
3. **If speech detected:**
   - Start new utterance if not already in speech (reset buffer).
   - Append filtered audio to `speechBuf`.
   - Reset silence counter.
   - If buffer exceeds `wakeMaxSamples` (2s): abort -- too long for a wake word. Reset all state.
4. **If silence during speech:**
   - Append filtered audio to buffer.
   - Increment silence counter.
   - If silence >= `wakeSilenceMax` (15 frames, ~300ms):
     - End utterance.
     - If buffer >= `wakeMinSamples` (0.3s):
       - Convert int16 to float32 (divide by 32768.0).
       - Call `TranscribePCM()` to get text.
       - Call `isWakeWord(text)` to check.
       - If wake word detected, fire `onDetect()` callback.
     - Reset buffer and VAD state.

### `isWakeWord(text string) bool`

**Text normalization:**
- Lowercase, trim whitespace.
- Strip all non-letter, non-space characters.
- Collapse multiple spaces to single space.

**Exact match triggers:**
```go
triggers := []string{
    "hey nebo",
    "hey nemo",
    "hey neighbor",
    "a nebo",
    "hey nebbo",
    "hey nebow",
    "he nebo",
}
```

Matches if `text == trigger` or `strings.HasPrefix(text, trigger+" ")`.

**Fuzzy match:** Levenshtein edit distance <= 3 from `"hey nebo"`.

### `levenshtein(a, b string) int`

Standard single-row dynamic programming implementation. Operates on byte-level (not rune-level).

---

## 6. Duplex Audio Pipeline

**File:** `duplex.go` and `pipeline.go`

### 6.1 VoiceConn

```go
type VoiceConn struct {
    transport  VoiceTransport
    deps       DuplexDeps

    // Pipeline channels
    audioCh    chan []byte    // readPump -> asrLoop: raw Int16LE PCM (buffer: 100)
    textCh     chan string    // asrLoop -> llmLoop: transcribed text (buffer: 10)
    ttsCh      chan string    // llmLoop -> ttsLoop: sentences to speak (buffer: 20)
    audioOutCh chan []byte    // ttsLoop -> writePump: PCM audio to send (buffer: 200)

    // State
    state      atomic.Int32  // VoiceState as int32
    cancel     context.CancelFunc
    gate       *NoiseGate
    vad        VAD
    voice      atomic.Value  // string, default "rachel"
}
```

### 6.2 VoiceState

```go
const (
    StateIdle         VoiceState = 0  // No active voice session
    StateListening    VoiceState = 1  // Receiving and processing user audio
    StateProcessing   VoiceState = 2  // ASR->LLM pipeline running
    StateSpeaking     VoiceState = 3  // TTS audio being sent to client
    StateInterrupting VoiceState = 4  // User interrupted during speaking
)
```

State transitions are atomic via `atomic.Int32`. The `interrupt()` method uses `CompareAndSwap` to prevent race conditions between five concurrent goroutines.

### 6.3 DuplexDeps

```go
type DuplexDeps struct {
    RunnerFunc func(ctx context.Context, sessionKey, prompt, channel string) (<-chan string, error)
    SendFrame  func(frame map[string]any) error
    SampleRate int  // default 16000
}
```

- `RunnerFunc`: Runs a prompt through the agentic loop. Session key is `"companion-default"`, channel is `"voice"`.
- `SendFrame`: Broadcasts events to web UI via the agent hub.

### 6.4 serve() - Pipeline Launch

```go
func (vc *VoiceConn) serve(ctx context.Context)
```

Launches 5 goroutines via `sync.WaitGroup`:
1. `transport.ReadPump(ctx, cancel, audioCh, handleControl)` -- reads from client
2. `transport.WritePump(ctx, audioOutCh)` -- writes to client
3. `asrLoop(ctx)` -- audio -> text
4. `llmLoop(ctx)` -- text -> LLM -> sentences
5. `ttsLoop(ctx)` -- sentences -> audio

Blocks on `<-ctx.Done()`. Then closes all channels and transport, waits for goroutines.

### 6.5 asrLoop - Audio to Text

**File:** `pipeline.go`

#### Constants

```go
const maxSilenceFrames = 8     // ~256ms at 32ms VAD chunks
const gatedSilenceFrames = 60  // ~480ms of gated frames = silence (60 x 8ms)
const vadChunkSize = 512       // 32ms at 16kHz -- Silero-compatible
```

#### Algorithm

```
FOR EACH raw frame from audioCh:
    pcm = decodePCM(raw)  // Int16LE bytes -> []int16

    // 1. Noise Gate
    filtered = gate.Filter(pcm)
    IF filtered == nil:
        IF inSpeech AND consecutiveGated >= 60:
            // Silence detected via gated frames (VAD never runs)
            Flush vadBuf into speechBuf
            IF speechBuf > 0.5s: transcribe(ctx, speechBuf)
            Reset
        CONTINUE

    // 2. Accumulate for VAD
    vadBuf = append(vadBuf, filtered...)
    IF len(vadBuf) < 512:
        IF inSpeech: append filtered to speechBuf
        CONTINUE

    // 3. Run VAD on accumulated chunk
    speech = vad.IsSpeech(chunk)
    Send vad_state control message to client

    // 4. Handle interruption
    IF speech AND state == StateSpeaking:
        interrupt()

    // 5. Speech state machine
    IF speech:
        IF !inSpeech: start new utterance
        Append chunk to speechBuf
    ELSE IF inSpeech:
        Append chunk (trailing silence)
        silenceFrames++
        IF silenceFrames >= 8:
            // End of utterance
            IF speechBuf > 0.5s: transcribe(ctx, speechBuf)
            Reset
```

Browser AudioWorklet sends 128 samples per frame (~8ms at 16kHz). The pipeline accumulates 4 frames into vadBuf to reach 512 samples (32ms) before running VAD, which is the minimum chunk size for Silero compatibility.

Minimum utterance length to transcribe: 0.5 seconds (`SampleRate / 2` samples).

### 6.6 transcribe() - PCM to Text

```go
func (vc *VoiceConn) transcribe(ctx context.Context, pcm []int16)
```

1. Sets state to `StateProcessing`.
2. Converts int16 to float32 (divide by 32768.0).
3. Calls `TranscribePCM(float32Pcm)`.
4. On error: sends error control message, returns to `StateListening`.
5. Filters out blank results: `""`, `"[BLANK_AUDIO]"`, `"(silence)"`.
6. Sends `transcript` control message to client.
7. Forwards text to `textCh` for LLM processing.

### 6.7 llmLoop - Text to Sentences

```go
func (vc *VoiceConn) llmLoop(ctx context.Context)
```

For each text from `textCh`:
1. Sets state to `StateProcessing`.
2. Broadcasts `dm_user_message` event to web UI (source: `"voice_duplex"`).
3. Calls `RunnerFunc(ctx, "companion-default", text, "voice")` to get a `<-chan string` of response chunks.
4. Calls `streamToSentences(ctx, chunks)`.

### 6.8 streamToSentences - Sentence Splitting for Low-Latency TTS

```go
func (vc *VoiceConn) streamToSentences(ctx context.Context, chunks <-chan string)
```

**Strategy:** Split LLM output into sentence-sized pieces as early as possible for low-latency speech output.

**Timer-based flush:**
- First segment: 400ms timeout.
- Subsequent segments: 800ms timeout.
- Timer fires if no punctuation-based flush occurs.

**For each chunk received from the LLM:**
1. Broadcasts `chat_stream` event to web UI.
2. Appends to buffer.
3. Tries sentence boundary splitting.
4. If no sentence boundary found, tries clause boundary splitting (if buffer > 20 chars).
5. If no flush, starts/resets the timer.

**On stream end:** Flushes remaining buffer. If nothing was sent to TTS, returns to `StateListening`.

#### `findSentenceEnd(text string) int`

Returns index of sentence-ending punctuation (`.`, `!`, `?`) followed by whitespace or uppercase letter (to avoid splitting abbreviations like "Dr.").

#### `findClauseEnd(text string) int`

Checks for clause-ending punctuation followed by space: `,`, `;`, `:`.
Also checks for em-dash ` -- ` and double-hyphen ` -- `.

### 6.9 ttsLoop - Text to Audio

```go
func (vc *VoiceConn) ttsLoop(ctx context.Context)
```

For each sentence from `ttsCh`:
1. Sets state to `StateSpeaking`.
2. Calls `SynthesizeSpeechForDuplex(sentence, voice)`.
3. Converts audio to PCM at pipeline sample rate via `audioToPCM()`.
4. Chunks into 20ms frames: `frameSize = SampleRate * 2 * 20 / 1000` bytes.
5. For each frame:
   - Checks for interruption (`getState() != StateSpeaking`) -- breaks but does not kill goroutine.
   - Sends frame to `audioOutCh`.
   - Paces output: `time.Sleep(18ms)` per frame (~20ms real-time).
6. After all frames: if no more sentences pending in `ttsCh`, returns to `StateListening`.

### 6.10 Audio Conversion and Resampling

#### `audioToPCM(data []byte, targetRate int) []byte`

Converts raw audio to PCM at target sample rate. **Rejects encoded formats:**
- AIFF: detected by `"FORM"` magic bytes.
- MP3: detected by `"ID3"` or `0xFF 0xE0` sync bytes.
- WAV: detected by `"RIFF"` magic bytes.

Assumes raw input is Kokoro's 24kHz Int16LE PCM:
- If `targetRate == 24000`: returns data as-is.
- Otherwise: converts to float32, resamples, converts back to Int16LE.

#### `resample(samples []float32, srcRate, dstRate int) []float32`

Linear interpolation resampling:
```
ratio = srcRate / dstRate
outLen = ceil(len(samples) / ratio)
for i in 0..outLen:
    srcIdx = i * ratio
    idx = floor(srcIdx)
    frac = srcIdx - idx
    out[i] = samples[idx] * (1 - frac) + samples[idx+1] * frac
```

#### `float32ToPCM(samples []float32) []byte`

Clamps to [-1.0, 1.0], multiplies by 32767, writes as Int16LE.

#### `pcmToFloat32(pcm []int16) []float32`

Divides by 32768.0.

### 6.11 Interrupt Handling

```go
func (vc *VoiceConn) interrupt()
```

Uses `CompareAndSwap(StateSpeaking, StateInterrupting)` to atomically claim the transition. If CAS fails, another goroutine already handled it.

On successful CAS:
1. Sends `StateInterrupting` state notification.
2. Drains `ttsCh` channel (pending sentences).
3. Drains `audioOutCh` channel (pending audio frames).
4. Resets VAD.
5. Transitions to `StateListening`.

---

## 7. Audio Codecs and Formats

| Context | Format | Sample Rate | Channels | Bit Depth |
|---------|--------|-------------|----------|-----------|
| WebSocket audio frames | Raw Int16LE PCM | 16kHz | Mono | 16-bit |
| Kokoro TTS output | Raw Int16LE PCM | 24kHz | Mono | 16-bit |
| ElevenLabs TTS | MP3 | varies | varies | varies |
| macOS say TTS | AIFF | varies | varies | varies |
| Whisper input | Float32 PCM | 16kHz | Mono | 32-bit float |
| WAV files (temp) | PCM in RIFF container | 16kHz | Mono | 16-bit |
| Comms transport | Base64-encoded Int16LE PCM | 16kHz | Mono | 16-bit |

**Important:** Only raw Int16LE PCM works in the duplex pipeline. Encoded formats (MP3, AIFF, WAV) are rejected by `audioToPCM()`. The `!cgo` fallback TTS (ElevenLabs/macOS) returns encoded formats, which means the duplex pipeline's ttsLoop will produce no output in headless mode.

### WAV File Writing

```go
func writeWavFile(path string, pcm []int16) error
```

Writes standard RIFF WAV:
- Format: PCM (1)
- Channels: 1 (mono)
- Sample rate: 16000 Hz
- Byte rate: 32000 (16000 * 2)
- Block align: 2
- Bits per sample: 16

### PCM Decoding

```go
func decodePCM(raw []byte) []int16
```

Converts raw Int16LE bytes to `[]int16` slice. Two bytes per sample, little-endian.

---

## 8. Model Management

**File:** `models.go`

### 8.1 ModelManifest

```go
type ModelManifest struct {
    Name        string `json:"name"`         // e.g. "ggml-base.en.bin"
    URL         string `json:"url"`          // Primary download URL
    FallbackURL string `json:"fallbackUrl"`  // Backup CDN URL
    Size        int64  `json:"size"`         // Expected size in bytes
    SHA256      string `json:"sha256"`       // Hex-encoded checksum
}
```

### 8.2 Required Models

```go
func RequiredModels() []ModelManifest
```

| Model | Primary URL | CDN Fallback | Size |
|-------|-------------|--------------|------|
| `ggml-base.en.bin` | `huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin` | `cdn.neboloop.com/voice/ggml-base.en.bin` | ~142MB (147,951,465 bytes) |
| `silero_vad.onnx` | `github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx` | `cdn.neboloop.com/voice/silero_vad.onnx` | ~2MB (2,167,808 bytes) |
| `kokoro-v1.0.onnx` | `github.com/thewh1teagle/kokoro-onnx/releases/latest/download/kokoro-v1.0.onnx` | `cdn.neboloop.com/voice/kokoro-v1.0.onnx` | ~80MB (83,886,080 bytes) |
| `voices-v1.0.bin` | `github.com/thewh1teagle/kokoro-onnx/releases/latest/download/voices-v1.0.bin` | `cdn.neboloop.com/voice/voices-v1.0.bin` | ~50MB (52,428,800 bytes) |
| Platform ONNX Runtime | CDN only | - | varies |

CDN base: `https://cdn.neboloop.com/voice/`

### 8.3 ONNX Runtime Model

```go
func onnxRuntimeModel() ModelManifest
```

Platform-specific shared library names:
- **macOS:** `libonnxruntime.<GOARCH>.dylib`
- **Linux:** `libonnxruntime.<GOARCH>.so`
- **Windows:** `onnxruntime.<GOARCH>.dll`

CDN-only (upstream ships as `.tgz` requiring extraction).

### 8.4 Models Directory

```go
func ModelsDir() string
```

Returns `<DataDir>/voice/`. Creates directory if needed.
- DataDir comes from `defaults.DataDir()`.
- Fallback: `~/.config/nebo`.

### 8.5 Model Status Checking

#### `VoiceModelsReady() bool`

Returns true only if ALL required models are present on disk.

#### `modelPresent(path string, m ModelManifest) bool`

- If `SHA256` is set: checks `file.Size() == m.Size` (exact size match).
- Otherwise: checks file exists and `Size > 0`.

#### `ModelStatus() []map[string]any`

Returns status of each model: `{name, size, downloaded}`.

### 8.6 Download Pipeline

#### `DownloadModels(ctx context.Context, progress func(DownloadProgress)) error`

- Iterates `RequiredModels()`.
- Skips already-present models (reports them as done).
- Calls `downloadModel()` for missing models.

#### `DownloadProgress`

```go
type DownloadProgress struct {
    Model      string `json:"model"`
    Downloaded int64  `json:"downloaded"`
    Total      int64  `json:"total"`
    Done       bool   `json:"done"`
    Error      string `json:"error,omitempty"`
}
```

#### `downloadModel(ctx context.Context, m ModelManifest, dir string, progress func(DownloadProgress)) error`

Tries primary URL first, then fallback URL on failure.

#### `downloadFromURL(ctx context.Context, url string, m ModelManifest, dir string, progress func(DownloadProgress)) error`

1. HTTP GET with context.
2. Uses `Content-Length` for progress (falls back to manifest size estimate).
3. Writes to `.tmp` file atomically.
4. Progress: 64KB read chunks.
5. SHA256 verification (if `SHA256` is set in manifest).
6. Atomic rename from `.tmp` to final name.
7. Context cancellation support throughout.

---

## 9. WebSocket Transport Protocol

### 9.1 Transport Interface

**File:** `transport.go`

```go
type VoiceTransport interface {
    ReadPump(ctx context.Context, cancel context.CancelFunc, audioCh chan<- []byte, onControl func([]byte))
    WritePump(ctx context.Context, audioOutCh <-chan []byte)
    SendControl(msg ControlMessage) error
    Close() error
}
```

### 9.2 Direct WebSocket Transport (wsTransport)

**File:** `transport_ws.go`

```go
type wsTransport struct {
    conn    *websocket.Conn
    writeMu sync.Mutex
}
```

#### ReadPump

- Sets initial read deadline: `voicePongWait` (60s).
- Registers pong handler that extends read deadline.
- **Binary frames** -> pushed to `audioCh` (raw Int16LE PCM).
- **Text frames** -> passed to `onControl` callback (JSON).
- Drops audio frames if `audioCh` is full (non-blocking send).
- Calls `cancel()` on exit (error or context done).

#### WritePump

- Sends audio from `audioOutCh` as binary WebSocket frames.
- Sends periodic pings at `voicePingPeriod` (54s).
- Write deadline: `voiceWriteWait` (10s) per message.
- `writeMu` protects all writes.

#### SendControl

- Marshals `ControlMessage` to JSON.
- Sends as WebSocket text frame.
- Thread-safe via `writeMu`.

### 9.3 NeboLoop Comms Transport (CommsTransport)

**File:** `transport_comms.go`

```go
type CommsTransport struct {
    sendFunc CommsTransportSendFunc
    inbound  chan CommsVoiceMessage  // buffer: 100
    closeMu  sync.Mutex
    closed   bool
}
```

#### Wire Format

```go
type CommsVoiceMessage struct {
    Type       string `json:"type"`                 // "audio", "voice_start", "voice_end", "interrupt", "config"
    Data       string `json:"data,omitempty"`        // base64-encoded PCM
    SampleRate int    `json:"sample_rate,omitempty"`  // 16000
    Channels   int    `json:"channels,omitempty"`     // 1
    Encoding   string `json:"encoding,omitempty"`     // "pcm_s16le"
    Text       string `json:"text,omitempty"`
    Final      bool   `json:"final,omitempty"`
    Speaking   bool   `json:"speaking,omitempty"`
    State      string `json:"state,omitempty"`
    Voice      string `json:"voice,omitempty"`
    Error      string `json:"error,omitempty"`
}
```

#### Feed(msg CommsVoiceMessage)

Called by the comms plugin to inject inbound messages. Non-blocking: drops if channel full.

#### ReadPump

- Reads from `inbound` channel.
- `"audio"` type: base64-decodes `Data` field, pushes raw PCM to `audioCh`.
- Other types: converts to `ControlMessage` JSON, passes to `onControl`.

#### WritePump

- Reads from `audioOutCh`.
- Base64-encodes PCM.
- Sends via `sendFunc` as `CommsVoiceMessage{Type: "audio", Data: encoded, SampleRate: 16000, Channels: 1, Encoding: "pcm_s16le"}`.

#### SendControl

Converts `ControlMessage` to `CommsVoiceMessage` and sends via `sendFunc`.

### 9.4 Control Message Protocol

```go
type ControlMessage struct {
    Type       string `json:"type"`                  // "state", "transcript", "config", "vad_state", "error", "auth_ok", "wake", "auth"
    State      string `json:"state,omitempty"`        // "idle", "listening", "processing", "speaking", "interrupting"
    Text       string `json:"text,omitempty"`
    IsSpeech   bool   `json:"is_speech,omitempty"`
    SampleRate int    `json:"sample_rate,omitempty"`
    Voice      string `json:"voice,omitempty"`
}
```

**Inbound (client -> server):**
- `"config"` with `Voice` field: changes TTS voice.
- `"interrupt"`: triggers interrupt (drains TTS queue).
- `"auth"` with `token` field: post-connect JWT authentication.

**Outbound (server -> client):**
- `"state"`: voice state change notification.
- `"transcript"`: transcribed user speech.
- `"vad_state"`: VAD speech/silence status (`is_speech`).
- `"error"`: error message.
- `"auth_ok"`: authentication accepted.
- `"wake"`: wake word detected (from WakeWordHandler).

### 9.5 WebSocket Configuration

```go
const (
    voiceWriteWait  = 10 * time.Second
    voicePongWait   = 60 * time.Second
    voicePingPeriod = 54 * time.Second  // (60 * 9) / 10
)
```

```go
var upgrader = websocket.Upgrader{
    ReadBufferSize:  4096,
    WriteBufferSize: 4096,
    CheckOrigin:     // allows empty origin or localhost origins
}
```

### 9.6 Authentication

#### Pre-upgrade Auth

```go
func extractVoiceAuth(r *http.Request, secret string) bool
```

Checks:
1. `Authorization: Bearer <token>` header -- validates JWT.
2. `nebo_token` cookie -- validates JWT.

#### Post-connect Auth

```go
func handleVoicePostConnect(conn *websocket.Conn, secret string)
```

- Sets 5-second read deadline.
- Reads first text message.
- If `type == "auth"` with `token`: validates JWT but allows connection even on failure.
- Sends `auth_ok` in all cases (local connections are always allowed).
- Auth timeout: `voiceAuthDeadline = 5 * time.Second`.

---

## 10. Voice Handler HTTP Endpoints

**File:** `transcribe.go` and `duplex.go`

### 10.1 `TranscribeHandler` (POST /api/voice/transcribe)

- Accepts multipart form with `"audio"` file field.
- Max upload: 25MB.
- Determines file extension from Content-Type or filename:
  - `webm`, `ogg`, `wav`, `m4a` detected; default `webm`.
- Writes to temp file.
- **Transcription priority:**
  1. Local: convert to WAV (if needed) via ffmpeg, then `TranscribeFile()`.
  2. OpenAI API: `OPENAI_API_KEY` env var, `whisper-1` model.
- Filters `[BLANK_AUDIO]` and `(silence)` results.
- Response: `{"text": "..."}`.

### 10.2 `TTSHandler` (POST /api/voice/tts)

```go
type TTSRequest struct {
    Text  string  `json:"text"`
    Voice string  `json:"voice"`
    Speed float64 `json:"speed"`
}
```

- Priority: ElevenLabs (`ELEVENLABS_API_KEY`) -> macOS `say` (darwin).
- Response: raw audio bytes with appropriate Content-Type.
- `Cache-Control: no-cache`.

### 10.3 `VoicesHandler` (GET /api/voice/voices)

- Returns available ElevenLabs voice names and IDs.
- Response: `{"voices": [{"name": "rachel", "id": "21m00Tcm4TlvDq8ikWAM"}, ...]}`.

### 10.4 `DuplexHandler` (WebSocket /ws/voice)

```go
func DuplexHandler(deps DuplexDeps, accessSecret string) http.HandlerFunc
```

1. Tries pre-upgrade auth (Bearer header, cookie).
2. Upgrades to WebSocket.
3. If not pre-authenticated, runs post-connect handshake.
4. Creates `wsTransport` and `VoiceConn`.
5. Calls `vc.Serve(r.Context())`.

### 10.5 `WakeWordHandler` (WebSocket /ws/wake)

```go
func WakeWordHandler(accessSecret string) http.HandlerFunc
```

1. Pre-upgrade auth check + WebSocket upgrade.
2. Post-connect handshake if needed.
3. Creates `WakeWordDetector` with callback that sends `{"type":"wake"}`.
4. Reads binary frames, decodes PCM, feeds to detector.
5. Separate ping ticker goroutine (54s interval).
6. On wake word detection: sends wake control message; client disconnects and opens full duplex.

### 10.6 Model Status & Download

These are available via `ModelStatus()` and `DownloadModels()` functions (exposed through HTTP handlers defined elsewhere in the server).

---

## 11. ONNX Runtime Integration

**File:** `vad_silero.go` (build tag: `cgo`)

### Initialization

```go
var (
    onnxOnce sync.Once
    onnxErr  error
)

func initONNXRuntime() error
```

- Shared between Silero VAD and Kokoro TTS.
- Called exactly once via `sync.Once`.
- Sets shared library path via `ort.SetSharedLibraryPath()`.
- Calls `ort.InitializeEnvironment()`.

### Library: `github.com/yalue/onnxruntime_go`

Types used:
- `ort.DynamicAdvancedSession` -- dynamic input/output tensor sessions.
- `ort.Tensor[float32]` -- typed tensor.
- `ort.NewTensor(shape, data)` -- tensor constructor.
- `ort.NewShape(dims...)` -- shape constructor.
- `ort.Value` -- generic tensor value (for dynamic outputs like Kokoro).
- `session.Run(inputs, outputs)` -- inference.
- `tensor.GetData()` -- access underlying slice.
- `tensor.Destroy()` -- manual memory management.

### Session Configurations

**Silero VAD:**
- Inputs: `["input", "state", "sr"]`
  - `input`: `[1, chunk_size]` float32
  - `state`: `[2, 1, 128]` float32 (hidden state, recurrent)
  - `sr`: `[1]` int64 (sample rate = 16000)
- Outputs: `["output", "stateN"]`
  - `output`: `[1, 1]` float32 (speech probability)
  - `stateN`: `[2, 1, 128]` float32 (new hidden state)

**Kokoro TTS:**
- Inputs: `["tokens", "style", "speed"]`
  - `tokens`: `[1, N]` int64 (phoneme token IDs)
  - `style`: `[1, 256]` float32 (voice embedding frame)
  - `speed`: `[1]` float32 (speech rate, default 1.0)
- Outputs: `["audio"]`
  - `audio`: dynamically shaped float32 (24kHz mono PCM)
  - Output is `nil` in the inputs array -- ONNX runtime allocates dynamically.

---

## 12. Phonemization Pipeline

**File:** `phonemize.go` (build tag: `cgo`)

### Pipeline Overview

```
English Text -> normalizeText() -> textToPhonemes() -> phonemesToTokens() -> []int64
```

### 12.1 `Phonemize(text string) []int64`

Entry point. Returns Kokoro token IDs including start (0) and end (0) tokens.

### 12.2 Text Normalization

```go
func normalizeText(text string) string
```

- Expands abbreviations (14 entries):
  - `Mr.` -> `Mister`, `Mrs.` -> `Missus`, `Dr.` -> `Doctor`, `St.` -> `Saint`
  - `Jr.` -> `Junior`, `Sr.` -> `Senior`, `vs.` -> `versus`, `etc.` -> `etcetera`
  - `approx.` -> `approximately`, `dept.` -> `department`, `est.` -> `established`
  - `govt.` -> `government`, `e.g.` -> `for example`, `i.e.` -> `that is`
- Collapses whitespace.

### 12.3 Text to Phonemes

```go
func textToPhonemes(text string) string
```

**Tokenization** (`tokenizeText`):
- Splits on non-letter, non-apostrophe characters.
- Preserves punctuation as separate tokens (`.`, `,`, `!`, `?`, `;`, `:`, `-`).

**Per-word phonemization priority:**
1. **CMU Dictionary lookup** (`cmuDict`): ~180 common English words with hand-crafted IPA.
2. **Punctuation mapping**: `.!?` -> `"."`, `,;:` -> `","`, `-` -> `" "`.
3. **Rule-based G2P** (`rulesBasedG2P`): grapheme-to-phoneme rules.

### 12.4 Rule-Based G2P

```go
func rulesBasedG2P(word string) string
```

Longest-match-first strategy:
1. Try 4-character rules, then 3, then 2.
2. Fall back to single character rules.

**Multi-character rules (42 entries):**

| Grapheme | IPA | Grapheme | IPA |
|----------|-----|----------|-----|
| `tion` | `shan` | `sion` | `zhun` |
| `ough` | `uf` | `ight` | `aIt` |
| `eous` | `ias` | `ious` | `ias` |
| `ture` | `tcher` | `sure` | `sher` |
| `ould` | `Ud` | `ound` | `aUnd` |
| `ence` | `uns` | `ance` | `uns` |
| `ment` | `ment` | `ness` | `nes` |
| `able` | `ubul` | `ible` | `ubul` |
| `ally` | `uli` | `ful` | `ful` |
| `ing` | `ing` | `ght` | `t` |
| `tch` | `tch` | `dge` | `dzh` |
| `sch` | `sk` | `chr` | `kr` |
| `que` | `k` | `ph` | `f` |
| `th` | `th` | `sh` | `sh` |
| `ch` | `tch` | `wh` | `w` |
| `wr` | `r` | `kn` | `n` |
| `gn` | `n` | `ck` | `k` |
| `ng` | `ng` | `gh` | (silent) |
| `ee` | `i` | `ea` | `i` |
| `oo` | `u` | `ou` | `aU` |
| `ow` | `oU` | `ai` | `eI` |
| `ay` | `eI` | `oi` | `oI` |
| `oy` | `oI` | `au` | `o` |
| `aw` | `o` | `er` | `er` |
| `ir` | `er` | `ur` | `er` |
| `ar` | `ar` | `or` | `or` |
| `le` | `ul` | | |

**Single character rules (26 entries):**
All lowercase letters mapped to their most common IPA pronunciation (e.g., `a` -> `ae`, `b` -> `b`, `c` -> `k`).

### 12.5 Phonemes to Token IDs

```go
func phonemesToTokens(phonemes string) []int64
```

- Prepends start token (0).
- Tries 2-rune match first (diphthongs/affricates), then single-rune match.
- Appends end token (0).
- Unknown phonemes are silently skipped.

**Token ID Map (45 entries):**

| Phoneme | ID | Phoneme | ID | Phoneme | ID |
|---------|----|---------|----|---------|-----|
| ` ` (space) | 1 | `a` (open back) | 2 | `ae` | 3 |
| `uh` | 4 | `aw` | 5 | `aU` (diphthong) | 6 |
| `aI` (diphthong) | 7 | `b` | 8 | `tS` (affricate) | 9 |
| `d` | 10 | `dh` | 11 | `E` | 12 |
| `er` (r-colored) | 13 | `er` (schwa+r) | 14 | `eI` (diphthong) | 15 |
| `f` | 16 | `g` | 17 | `h` | 18 |
| `i` | 19 | `I` | 20 | `dZ` (affricate) | 21 |
| `k` | 22 | `l` | 23 | `m` | 24 |
| `n` | 25 | `ng` | 26 | `oU` (diphthong) | 27 |
| `oI` (diphthong) | 28 | `p` | 29 | `r` | 30 |
| `s` | 31 | `sh` | 32 | `t` | 33 |
| `th` | 34 | `u` | 35 | `U` | 36 |
| `v` | 37 | `w` | 38 | `j` | 39 |
| `z` | 40 | `zh` | 41 | `.` | 42 |
| `,` | 43 | `schwa` | 44 | | |

---

## 13. Error Handling and Fallback Behavior

### 13.1 ASR Fallback Chain

```
1. Embedded whisper.cpp (cgo) -- preferred
   |-- Fails if model not loaded -> error
   |-- Fails if whisper_full returns non-zero -> error

2. whisper-cli subprocess (!cgo) -- headless fallback
   |-- Requires whisper-cli in PATH
   |-- Requires ggml-base.en.bin on disk (or embedded)
   |-- Falls through on any failure

3. OpenAI Whisper API -- cloud fallback (TranscribeHandler only)
   |-- Requires OPENAI_API_KEY env var
   |-- 503 if no backend available
```

### 13.2 TTS Fallback Chain

**Duplex pipeline (SynthesizeSpeechForDuplex):**
```
cgo build:
  1. Kokoro ONNX (24kHz raw PCM) -- only option that works in duplex

!cgo build:
  1. ElevenLabs (MP3) -- WILL NOT WORK in duplex (audioToPCM rejects MP3)
  2. macOS say (AIFF) -- WILL NOT WORK in duplex (audioToPCM rejects AIFF)
  * Result: duplex TTS is effectively broken in !cgo builds
```

**HTTP handler (TTSHandler):**
```
1. ElevenLabs (ELEVENLABS_API_KEY) -> audio/mpeg
   |-- Returns nil,nil on any failure (silent fallback)
2. macOS say (darwin only) -> audio/aiff
3. 503 "No TTS provider configured"
```

### 13.3 VAD Fallback

```
cgo build:
  1. RMSVAD (Silero VAD is disabled with TODO comment)
  * If Silero were enabled:
    - SileroVAD with ONNX
    - On first inference error: sets broken=true, permanently falls back to rms(pcm) >= 0.006

!cgo build:
  1. RMSVAD (only option)
```

### 13.4 Pipeline Error Recovery

| Error | Behavior |
|-------|----------|
| Audio frame dropped (channel full) | Frame silently dropped, pipeline continues |
| Noise gate calibrating | All frames suppressed for first 20 frames (~160ms) |
| Transcription fails | Error control message sent to client, returns to `StateListening` |
| Transcription returns blank/silence | Silently returns to `StateListening` |
| Runner not configured | Error control message, returns to `StateListening` |
| Runner error | Error control message, returns to `StateListening` |
| TTS synthesis fails | Logs error, skips sentence, continues with next |
| TTS returns empty audio | Logs warning, skips sentence |
| audioToPCM rejects format | Logs warning, returns nil, ttsLoop skips |
| WebSocket read error | ReadPump exits, cancels context, entire pipeline shuts down |
| WebSocket write error | WritePump exits, pipeline continues until context cancelled |
| Context cancelled | All goroutines exit, channels closed, transport closed |
| Interrupt during TTS | CAS-based state transition, drains queues, returns to listening |
| Comms transport backed up | Inbound messages dropped (non-blocking send to channel) |
| Model download fails primary URL | Retries with fallback URL |
| Model download checksum mismatch | Returns error, model file deleted |
| Model download context cancelled | Download aborted, temp file cleaned up |

### 13.5 Test Coverage

**File:** `duplex_test.go`

| Test | What it verifies |
|------|-----------------|
| `TestDuplexHandler_PostConnectAuth` | WebSocket upgrade without pre-auth, then `{"type":"auth","token":"..."}` -> `auth_ok` |
| `TestDuplexHandler_BearerAuth` | WebSocket upgrade with `Authorization: Bearer` header -> 101 |
| `TestDuplexHandler_InvalidToken` | Invalid JWT in post-connect auth -> connection closed |
| `TestDuplexHandler_NoAuth` | No auth message sent -> connection closed after timeout (~5s) |
| `TestWakeWordHandler_NoAuth` | WakeWord handler with no auth -> connection closed after timeout |

---

## Appendix: Binary Assets in Voice Directory

The following binary files are present in the voice source directory (likely for development/testing):

| File | Purpose |
|------|---------|
| `ggml-base.en.bin` | Whisper base English model (~142MB) |
| `kokoro-v1.0.onnx` | Kokoro TTS ONNX model (~80MB) |
| `silero_vad.onnx` | Silero VAD ONNX model (~2MB) |
| `voices-v1.0.bin` | Kokoro voice embeddings ZIP (~50MB) |
| `libonnxruntime.arm64.dylib` | ONNX Runtime for Apple Silicon |
