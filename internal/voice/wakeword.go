package voice

import (
	"strings"
	"unicode"
)

// WakeWordDetector listens for "Hey Nebo" in short audio utterances.
// Feed audio frames; when a wake word is detected, the onDetect callback fires.
type WakeWordDetector struct {
	vad      VAD
	gate     *NoiseGate
	onDetect func()

	// Accumulate short speech for wake word check
	speechBuf     []int16
	inSpeech      bool
	silenceFrames int
}

const (
	// Wake word utterances are short — 0.3s to 2s
	wakeMinSamples = 16000 * 3 / 10  // 0.3s at 16kHz
	wakeMaxSamples = 16000 * 2        // 2.0s at 16kHz
	wakeSilenceMax = 15               // ~300ms at 20ms frames
)

// NewWakeWordDetector creates a detector that calls onDetect when "Hey Nebo" is heard.
func NewWakeWordDetector(onDetect func()) *WakeWordDetector {
	return &WakeWordDetector{
		vad:      NewDefaultVAD(),
		gate:     NewNoiseGate(),
		onDetect: onDetect,
	}
}

// Feed processes a single audio frame (20ms of int16 PCM at 16kHz).
// Returns true if the wake word was detected in this frame.
func (w *WakeWordDetector) Feed(pcm []int16) bool {
	filtered := w.gate.Filter(pcm)
	if filtered == nil {
		return false
	}

	speech := w.vad.IsSpeech(filtered)

	if speech {
		if !w.inSpeech {
			w.inSpeech = true
			w.speechBuf = w.speechBuf[:0]
			w.silenceFrames = 0
		}
		w.speechBuf = append(w.speechBuf, filtered...)
		w.silenceFrames = 0

		// Abort if utterance is too long — not a wake word
		if len(w.speechBuf) > wakeMaxSamples {
			w.inSpeech = false
			w.speechBuf = w.speechBuf[:0]
			w.vad.Reset()
		}
	} else if w.inSpeech {
		w.speechBuf = append(w.speechBuf, filtered...)
		w.silenceFrames++

		if w.silenceFrames >= wakeSilenceMax {
			w.inSpeech = false
			defer func() {
				w.speechBuf = w.speechBuf[:0]
				w.silenceFrames = 0
				w.vad.Reset()
			}()

			// Check if the short utterance is a wake word
			if len(w.speechBuf) >= wakeMinSamples {
				float32Pcm := make([]float32, len(w.speechBuf))
				for i, s := range w.speechBuf {
					float32Pcm[i] = float32(s) / 32768.0
				}

				text, err := TranscribePCM(float32Pcm)
				if err != nil {
					return false
				}

				if isWakeWord(text) {
					if w.onDetect != nil {
						w.onDetect()
					}
					return true
				}
			}
		}
	}

	return false
}

// isWakeWord checks if the transcribed text matches "Hey Nebo" or close variants.
func isWakeWord(text string) bool {
	// Normalize: lowercase, strip punctuation, collapse whitespace
	text = strings.ToLower(strings.TrimSpace(text))
	var cleaned strings.Builder
	for _, r := range text {
		if unicode.IsLetter(r) || unicode.IsSpace(r) {
			cleaned.WriteRune(r)
		}
	}
	text = strings.Join(strings.Fields(cleaned.String()), " ")

	// Exact and common misheard variants
	triggers := []string{
		"hey nebo",
		"hey nemo",
		"hey neighbor",
		"a nebo",
		"hey nebbo",
		"hey nebow",
		"he nebo",
	}

	for _, trigger := range triggers {
		if text == trigger || strings.HasPrefix(text, trigger+" ") {
			return true
		}
	}

	// Fuzzy: edit distance ≤ 3 from "hey nebo"
	if levenshtein(text, "hey nebo") <= 3 {
		return true
	}

	return false
}

// levenshtein computes the edit distance between two strings.
func levenshtein(a, b string) int {
	la, lb := len(a), len(b)
	if la == 0 {
		return lb
	}
	if lb == 0 {
		return la
	}

	// Use single-row DP
	prev := make([]int, lb+1)
	for j := 0; j <= lb; j++ {
		prev[j] = j
	}

	for i := 1; i <= la; i++ {
		curr := make([]int, lb+1)
		curr[0] = i
		for j := 1; j <= lb; j++ {
			cost := 1
			if a[i-1] == b[j-1] {
				cost = 0
			}
			del := prev[j] + 1
			ins := curr[j-1] + 1
			sub := prev[j-1] + cost
			curr[j] = min(del, min(ins, sub))
		}
		prev = curr
	}

	return prev[lb]
}
