//go:build !cgo

package voice

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// TranscribePCM transcribes float32 PCM audio by writing to a temp WAV and
// calling whisper-cli or falling back to OpenAI API.
func TranscribePCM(pcm []float32) (string, error) {
	// Convert float32 to int16
	int16Pcm := make([]int16, len(pcm))
	for i, s := range pcm {
		if s > 1.0 {
			s = 1.0
		} else if s < -1.0 {
			s = -1.0
		}
		int16Pcm[i] = int16(s * 32767)
	}

	// Write temp WAV
	tmpFile, err := os.CreateTemp("", "nebo-asr-*.wav")
	if err != nil {
		return "", fmt.Errorf("failed to create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()
	tmpFile.Close()
	defer os.Remove(tmpPath)

	if err := writeWavFile(tmpPath, int16Pcm); err != nil {
		return "", fmt.Errorf("failed to write WAV: %w", err)
	}

	return TranscribeFile(tmpPath)
}

// TranscribeFile transcribes a WAV file using whisper-cli subprocess.
func TranscribeFile(path string) (string, error) {
	modelPath := filepath.Join(ModelsDir(), "ggml-base.en.bin")

	// Try local whisper-cli
	if _, err := exec.LookPath("whisper-cli"); err == nil {
		if _, err := os.Stat(modelPath); err == nil {
			text, err := transcribeLocal(path, modelPath)
			if err == nil {
				return strings.TrimSpace(text), nil
			}
		}
	}

	return "", fmt.Errorf("no transcription backend available: install whisper-cli or set OPENAI_API_KEY")
}
