package voice

import (
	"context"
	"encoding/binary"
	"fmt"
	"math"
	"strings"
	"time"
	"unicode"
)

// asrLoop reads audio frames, applies noise gate + VAD, accumulates speech,
// and transcribes via whisper-cli when silence is detected.
func (vc *VoiceConn) asrLoop(ctx context.Context) {
	var speechBuf []int16
	var inSpeech bool
	silenceFrames := 0
	const maxSilenceFrames = 25 // ~500ms at 20ms frames

	for {
		select {
		case <-ctx.Done():
			return
		case raw, ok := <-vc.audioCh:
			if !ok {
				return
			}

			pcm := decodePCM(raw)

			// Apply noise gate
			filtered := vc.gate.Filter(pcm)
			if filtered == nil {
				continue
			}

			// Run VAD
			speech := vc.vad.IsSpeech(filtered)

			// Notify client of VAD state
			vc.sendControl(ControlMessage{
				Type:     "vad_state",
				IsSpeech: speech,
			})

			// Handle interruption — if user speaks while we're outputting audio
			if speech && vc.getState() == StateSpeaking {
				vc.interrupt()
			}

			if speech {
				if !inSpeech {
					inSpeech = true
					speechBuf = speechBuf[:0]
					silenceFrames = 0
				}
				speechBuf = append(speechBuf, filtered...)
				silenceFrames = 0
			} else if inSpeech {
				// Still append a few silence frames for natural trailing
				speechBuf = append(speechBuf, filtered...)
				silenceFrames++

				if silenceFrames >= maxSilenceFrames {
					// End of utterance — transcribe
					inSpeech = false
					if len(speechBuf) > vc.deps.SampleRate/2 { // at least 0.5s
						vc.transcribe(ctx, speechBuf)
					}
					speechBuf = speechBuf[:0]
					silenceFrames = 0
				}
			}
		}
	}
}

// transcribe converts accumulated PCM to float32 and runs through TranscribePCM.
func (vc *VoiceConn) transcribe(ctx context.Context, pcm []int16) {
	vc.setState(StateProcessing)

	// Convert int16 → float32 for TranscribePCM
	float32Pcm := make([]float32, len(pcm))
	for i, s := range pcm {
		float32Pcm[i] = float32(s) / 32768.0
	}

	text, err := TranscribePCM(float32Pcm)
	if err != nil {
		fmt.Printf("[voice-duplex] Transcription failed: %v\n", err)
		vc.sendControl(ControlMessage{
			Type: "error",
			Text: "Transcription failed: " + err.Error(),
		})
		vc.setState(StateListening)
		return
	}

	text = strings.TrimSpace(text)
	if text == "" || text == "[BLANK_AUDIO]" || text == "(silence)" {
		vc.setState(StateListening)
		return
	}

	// Send transcript to client
	vc.sendControl(ControlMessage{
		Type: "transcript",
		Text: text,
	})

	// Forward to LLM pipeline
	select {
	case vc.textCh <- text:
	case <-ctx.Done():
	}
}

// llmLoop takes transcribed text, runs it through the agentic runner,
// and splits the response into sentences for the TTS pipeline.
func (vc *VoiceConn) llmLoop(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case text, ok := <-vc.textCh:
			if !ok {
				return
			}

			vc.setState(StateProcessing)

			// Broadcast the user message to the web UI
			if vc.deps.SendFrame != nil {
				vc.deps.SendFrame(map[string]any{
					"type":   "event",
					"method": "dm_user_message",
					"payload": map[string]any{
						"content": text,
						"source":  "voice_duplex",
					},
				})
			}

			// Run through the agentic loop
			if vc.deps.RunnerFunc == nil {
				vc.sendControl(ControlMessage{
					Type: "error",
					Text: "Runner not configured",
				})
				vc.setState(StateListening)
				continue
			}

			chunks, err := vc.deps.RunnerFunc(ctx, "companion-default", text, "voice")
			if err != nil {
				fmt.Printf("[voice-duplex] Runner error: %v\n", err)
				vc.sendControl(ControlMessage{
					Type: "error",
					Text: "Processing failed",
				})
				vc.setState(StateListening)
				continue
			}

			// Accumulate and split into sentences for TTS
			vc.streamToSentences(ctx, chunks)
		}
	}
}

// streamToSentences reads text chunks from the runner and splits them into
// sentence-sized pieces for the TTS pipeline. Uses aggressive first-chunk
// timing and clause-boundary detection for low-latency speech output.
func (vc *VoiceConn) streamToSentences(ctx context.Context, chunks <-chan string) {
	var buf strings.Builder
	sentSent := false
	firstChunkSent := false

	// Timer-based flush — 400ms for first segment, 800ms for subsequent
	flushDelay := 400 * time.Millisecond
	flushTimer := time.NewTimer(flushDelay)
	flushTimer.Stop() // don't fire until we receive data
	defer flushTimer.Stop()

	for {
		select {
		case <-ctx.Done():
			return

		case <-flushTimer.C:
			// Timer fired — flush whatever we have
			text := strings.TrimSpace(buf.String())
			if text != "" {
				vc.sendToTTS(ctx, text)
				sentSent = true
				firstChunkSent = true
				buf.Reset()
			}

		case chunk, ok := <-chunks:
			if !ok {
				// Stream ended — flush remaining text
				flushTimer.Stop()
				remaining := strings.TrimSpace(buf.String())
				if remaining != "" {
					vc.sendToTTS(ctx, remaining)
					sentSent = true
				}
				if !sentSent {
					// No text was produced — go back to listening
					vc.setState(StateListening)
				}
				return
			}

			// Stream to web UI
			if vc.deps.SendFrame != nil {
				vc.deps.SendFrame(map[string]any{
					"type":   "event",
					"method": "chat_stream",
					"payload": map[string]any{
						"content": chunk,
						"source":  "voice_duplex",
					},
				})
			}

			buf.WriteString(chunk)

			// Try to split on sentence boundaries (.!?)
			text := buf.String()
			flushed := false
			for {
				idx := findSentenceEnd(text)
				if idx < 0 {
					break
				}
				sentence := strings.TrimSpace(text[:idx+1])
				if sentence != "" {
					vc.sendToTTS(ctx, sentence)
					sentSent = true
					firstChunkSent = true
					flushed = true
				}
				text = text[idx+1:]
			}
			buf.Reset()
			buf.WriteString(text)

			// Try clause boundaries if buffer is long enough
			if !flushed {
				text = buf.String()
				if idx := findClauseEnd(text); idx >= 0 && len(strings.TrimSpace(text[:idx+1])) > 20 {
					clause := strings.TrimSpace(text[:idx+1])
					if clause != "" {
						vc.sendToTTS(ctx, clause)
						sentSent = true
						firstChunkSent = true
						flushed = true
						buf.Reset()
						buf.WriteString(text[idx+1:])
					}
				}
			}

			// Reset/start the flush timer if we didn't flush via punctuation
			if !flushed {
				flushTimer.Stop()
				if firstChunkSent {
					flushTimer.Reset(800 * time.Millisecond)
				} else {
					flushTimer.Reset(400 * time.Millisecond)
				}
			} else {
				flushTimer.Stop()
			}
		}
	}
}

// sendToTTS sends a sentence to the TTS pipeline.
func (vc *VoiceConn) sendToTTS(ctx context.Context, text string) {
	select {
	case vc.ttsCh <- text:
	case <-ctx.Done():
	}
}

// findSentenceEnd returns the index of the last character of the first sentence,
// or -1 if no sentence boundary is found.
func findSentenceEnd(text string) int {
	for i, r := range text {
		if (r == '.' || r == '!' || r == '?') && i > 0 {
			// Check it's not an abbreviation (e.g., "Dr.")
			if i+1 < len(text) {
				next := rune(text[i+1])
				if unicode.IsSpace(next) || unicode.IsUpper(next) {
					return i
				}
			} else {
				return i
			}
		}
	}
	return -1
}

// findClauseEnd returns the index of a clause boundary in the text, or -1.
// Clause boundaries: ", " / "; " / " — " / ": "
func findClauseEnd(text string) int {
	// Check for clause-ending punctuation followed by a space
	for i := 0; i < len(text)-1; i++ {
		ch := text[i]
		if (ch == ',' || ch == ';' || ch == ':') && i+1 < len(text) && text[i+1] == ' ' {
			return i
		}
	}
	// Check for em-dash " — "
	if idx := strings.Index(text, " — "); idx >= 0 {
		return idx + len(" —") - 1
	}
	// Check for double-hyphen " -- "
	if idx := strings.Index(text, " -- "); idx >= 0 {
		return idx + len(" --") - 1
	}
	return -1
}

// ttsLoop reads sentences, synthesizes speech, and sends PCM audio to the client.
func (vc *VoiceConn) ttsLoop(ctx context.Context) {
	for {
		select {
		case <-ctx.Done():
			return
		case sentence, ok := <-vc.ttsCh:
			if !ok {
				return
			}

			vc.setState(StateSpeaking)

			// Synthesize speech
			audioData, err := SynthesizeSpeechForDuplex(sentence, vc.voice)
			if err != nil {
				fmt.Printf("[voice-duplex] TTS failed: %v\n", err)
				continue
			}

			if len(audioData) == 0 {
				continue
			}

			// Convert to PCM if needed and chunk into 20ms frames.
			// SynthesizeSpeechForDuplex returns raw PCM for Kokoro (desktop)
			// or MP3/AIFF for the fallback (headless).
			// For the fallback formats, we send the raw audio as a single chunk
			// and let the browser decode it.
			pcmData := audioToPCM(audioData, vc.deps.SampleRate)

			// Chunk into 20ms frames (sample_rate * 2 bytes * 0.020s)
			frameSize := vc.deps.SampleRate * 2 * 20 / 1000 // bytes per 20ms frame
			for i := 0; i < len(pcmData); i += frameSize {
				end := i + frameSize
				if end > len(pcmData) {
					end = len(pcmData)
				}

				// Check for interruption
				if vc.getState() != StateSpeaking {
					return
				}

				select {
				case vc.audioOutCh <- pcmData[i:end]:
				case <-ctx.Done():
					return
				}

				// Pace output to match real-time playback (~20ms per frame)
				time.Sleep(18 * time.Millisecond)
			}

			// After all TTS frames sent for this batch, check if more sentences pending
			if len(vc.ttsCh) == 0 {
				vc.setState(StateListening)
			}
		}
	}
}

// audioToPCM converts audio data to 16-bit PCM at the target sample rate.
// If the data is already PCM (from Kokoro), it's returned as-is.
// For encoded formats (MP3, AIFF), we return the raw bytes and let the
// browser handle decoding via the playback processor.
func audioToPCM(data []byte, _ int) []byte {
	// Check for AIFF header
	if len(data) > 4 && string(data[:4]) == "FORM" {
		return data // Return raw AIFF for browser to decode
	}

	// Check for MP3 header (ID3 tag or sync word)
	if len(data) > 3 && (string(data[:3]) == "ID3" || (data[0] == 0xFF && (data[1]&0xE0) == 0xE0)) {
		return data // Return raw MP3 for browser to decode
	}

	// Check for WAV/RIFF header
	if len(data) > 4 && string(data[:4]) == "RIFF" {
		return data // Return raw WAV for browser to decode
	}

	// Assume raw PCM (from Kokoro)
	return data
}

// pcmToFloat32 converts int16 PCM to float32 samples.
func pcmToFloat32(pcm []int16) []float32 {
	out := make([]float32, len(pcm))
	for i, s := range pcm {
		out[i] = float32(s) / 32768.0
	}
	return out
}

// float32ToPCM converts float32 samples to int16 PCM bytes (little-endian).
func float32ToPCM(samples []float32) []byte {
	out := make([]byte, len(samples)*2)
	for i, s := range samples {
		// Clamp
		if s > 1.0 {
			s = 1.0
		} else if s < -1.0 {
			s = -1.0
		}
		val := int16(s * 32767)
		binary.LittleEndian.PutUint16(out[i*2:], uint16(val))
	}
	return out
}

// resample resamples audio from srcRate to dstRate using linear interpolation.
func resample(samples []float32, srcRate, dstRate int) []float32 {
	if srcRate == dstRate {
		return samples
	}
	ratio := float64(srcRate) / float64(dstRate)
	outLen := int(math.Ceil(float64(len(samples)) / ratio))
	out := make([]float32, outLen)
	for i := range out {
		srcIdx := float64(i) * ratio
		idx := int(srcIdx)
		frac := float32(srcIdx - float64(idx))
		if idx+1 < len(samples) {
			out[i] = samples[idx]*(1-frac) + samples[idx+1]*frac
		} else if idx < len(samples) {
			out[i] = samples[idx]
		}
	}
	return out
}
