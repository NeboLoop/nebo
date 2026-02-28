package voice

import (
	"encoding/binary"
	"math"
	"os"
)

// VAD detects whether a chunk of PCM audio contains speech.
type VAD interface {
	// IsSpeech returns true if the given 16-bit PCM chunk contains speech.
	IsSpeech(pcm []int16) bool
	// Reset clears internal state (call between utterances).
	Reset()
}

// NoiseGate filters out ambient noise using a calibrated RMS threshold.
// Frames below the gate are zeroed out.
type NoiseGate struct {
	threshold   float64
	calibrated  bool
	calibFrames int
	calibSum    float64
}

// NewNoiseGate creates a NoiseGate that auto-calibrates from the first few frames.
func NewNoiseGate() *NoiseGate {
	return &NoiseGate{}
}

// Filter returns the PCM data if it exceeds the noise floor, or nil if gated.
// The first ~20 frames are used for calibration (assumes initial silence).
func (g *NoiseGate) Filter(pcm []int16) []int16 {
	level := rms(pcm)

	if !g.calibrated {
		g.calibFrames++
		g.calibSum += level
		if g.calibFrames >= 20 {
			avg := g.calibSum / float64(g.calibFrames)
			g.threshold = avg * 2.5 // 2.5x the ambient floor
			if g.threshold < 0.005 {
				g.threshold = 0.005
			}
			g.calibrated = true
		}
		return nil // suppress during calibration
	}

	if level < g.threshold {
		return nil
	}
	return pcm
}

// rms computes the root-mean-square of 16-bit PCM samples, normalized to [0, 1].
func rms(pcm []int16) float64 {
	if len(pcm) == 0 {
		return 0
	}
	var sum float64
	for _, s := range pcm {
		v := float64(s) / 32768.0
		sum += v * v
	}
	return math.Sqrt(sum / float64(len(pcm)))
}

// writeWavFile writes 16kHz mono 16-bit PCM data to a WAV file.
func writeWavFile(path string, pcm []int16) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	defer f.Close()

	dataSize := uint32(len(pcm) * 2)
	fileSize := 36 + dataSize

	// RIFF header
	f.Write([]byte("RIFF"))
	binary.Write(f, binary.LittleEndian, fileSize)
	f.Write([]byte("WAVE"))

	// fmt chunk
	f.Write([]byte("fmt "))
	binary.Write(f, binary.LittleEndian, uint32(16)) // chunk size
	binary.Write(f, binary.LittleEndian, uint16(1))  // PCM format
	binary.Write(f, binary.LittleEndian, uint16(1))  // mono
	binary.Write(f, binary.LittleEndian, uint32(16000)) // sample rate
	binary.Write(f, binary.LittleEndian, uint32(32000)) // byte rate (16000 * 2)
	binary.Write(f, binary.LittleEndian, uint16(2))     // block align
	binary.Write(f, binary.LittleEndian, uint16(16))    // bits per sample

	// data chunk
	f.Write([]byte("data"))
	binary.Write(f, binary.LittleEndian, dataSize)
	return binary.Write(f, binary.LittleEndian, pcm)
}

// decodePCM converts raw Int16LE bytes to an int16 slice.
func decodePCM(raw []byte) []int16 {
	n := len(raw) / 2
	pcm := make([]int16, n)
	for i := 0; i < n; i++ {
		pcm[i] = int16(binary.LittleEndian.Uint16(raw[i*2:]))
	}
	return pcm
}
