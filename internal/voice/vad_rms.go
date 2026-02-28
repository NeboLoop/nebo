//go:build !cgo

package voice

// RMSVAD is a pure-Go voice activity detector based on RMS energy levels.
// Uses hysteresis to avoid flickering between speech and silence states.
type RMSVAD struct {
	speechThreshold  float64 // RMS level to start speech
	silenceThreshold float64 // RMS level to end speech
	speechFrames     int     // consecutive speech frames needed to trigger
	silenceFrames    int     // consecutive silence frames needed to end
	inSpeech         bool
	speechCount      int
	silenceCount     int
}

// NewDefaultVAD returns an RMSVAD suitable for 16kHz 20ms frames.
func NewDefaultVAD() VAD {
	return &RMSVAD{
		speechThreshold:  0.015,
		silenceThreshold: 0.008,
		speechFrames:     3,     // 3 frames (~60ms) to start
		silenceFrames:    30,    // 30 frames (~600ms) to end
	}
}

// IsSpeech returns true if the PCM chunk is considered speech.
func (v *RMSVAD) IsSpeech(pcm []int16) bool {
	level := rms(pcm)

	if v.inSpeech {
		if level < v.silenceThreshold {
			v.silenceCount++
			v.speechCount = 0
			if v.silenceCount >= v.silenceFrames {
				v.inSpeech = false
				v.silenceCount = 0
			}
		} else {
			v.silenceCount = 0
		}
	} else {
		if level >= v.speechThreshold {
			v.speechCount++
			v.silenceCount = 0
			if v.speechCount >= v.speechFrames {
				v.inSpeech = true
				v.speechCount = 0
			}
		} else {
			v.speechCount = 0
		}
	}

	return v.inSpeech
}

// Reset clears internal state.
func (v *RMSVAD) Reset() {
	v.inSpeech = false
	v.speechCount = 0
	v.silenceCount = 0
}
