//go:build !cgo

package voice

// SynthesizeSpeechForDuplex generates TTS audio for the duplex pipeline.
// On headless builds, delegates to SynthesizeSpeech (ElevenLabs → macOS say).
// Returns raw audio bytes (MP3 or AIFF depending on provider).
func SynthesizeSpeechForDuplex(text, voice string) ([]byte, error) {
	data, _, err := SynthesizeSpeech(text, voice, 1.0)
	return data, err
}
