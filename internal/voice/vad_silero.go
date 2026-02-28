//go:build cgo

package voice

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"

	ort "github.com/yalue/onnxruntime_go"
)

// SileroVAD uses the Silero VAD ONNX model for high-accuracy speech detection.
// Expects 30ms chunks (480 samples at 16kHz). ~1ms inference per chunk on Apple Silicon.
type SileroVAD struct {
	session   *ort.DynamicAdvancedSession
	state     *ort.Tensor[float32] // hidden state [2, 1, 64]
	threshold float32
	inSpeech  bool
}

// NewDefaultVAD returns a SileroVAD if the model is available, falling back to RMSVAD.
func NewDefaultVAD() VAD {
	modelPath := sileroModelPath()
	if _, err := os.Stat(modelPath); err != nil {
		fmt.Printf("[voice] Silero VAD model not found at %s, using RMS VAD\n", modelPath)
		return newRMSVAD()
	}

	vad, err := newSileroVAD(modelPath)
	if err != nil {
		fmt.Printf("[voice] Failed to load Silero VAD: %v, using RMS VAD\n", err)
		return newRMSVAD()
	}
	return vad
}

func newSileroVAD(modelPath string) (*SileroVAD, error) {
	// Initialize ONNX Runtime shared library if not already done
	libPath := onnxRuntimeLibPath()
	if libPath != "" {
		ort.SetSharedLibraryPath(libPath)
	}
	if err := ort.InitializeEnvironment(); err != nil {
		// Already initialized is fine
		if err.Error() != "the ONNX runtime is already initialized" {
			return nil, fmt.Errorf("failed to initialize ONNX runtime: %w", err)
		}
	}

	// Create hidden state tensor [2, 1, 64] - zeros
	stateData := make([]float32, 2*1*64)
	state, err := ort.NewTensor(ort.NewShape(2, 1, 64), stateData)
	if err != nil {
		return nil, fmt.Errorf("failed to create state tensor: %w", err)
	}

	// Create session with dynamic inputs/outputs
	inputNames := []string{"input", "state", "sr"}
	outputNames := []string{"output", "stateN"}

	session, err := ort.NewDynamicAdvancedSession(
		modelPath,
		inputNames,
		outputNames,
		nil, // use default session options
	)
	if err != nil {
		state.Destroy()
		return nil, fmt.Errorf("failed to create session: %w", err)
	}

	return &SileroVAD{
		session:   session,
		state:     state,
		threshold: 0.5,
	}, nil
}

// IsSpeech runs Silero VAD inference on a PCM chunk and returns speech probability > threshold.
func (v *SileroVAD) IsSpeech(pcm []int16) bool {
	// Convert int16 → float32
	input := make([]float32, len(pcm))
	for i, s := range pcm {
		input[i] = float32(s) / 32768.0
	}

	// Create input tensor [1, chunk_size]
	inputTensor, err := ort.NewTensor(ort.NewShape(1, int64(len(input))), input)
	if err != nil {
		return v.inSpeech // keep previous state on error
	}
	defer inputTensor.Destroy()

	// Sample rate tensor [1] = 16000
	srData := []int64{16000}
	srTensor, err := ort.NewTensor(ort.NewShape(1), srData)
	if err != nil {
		return v.inSpeech
	}
	defer srTensor.Destroy()

	// Output tensors
	outputData := make([]float32, 1)
	outputTensor, err := ort.NewTensor(ort.NewShape(1, 1), outputData)
	if err != nil {
		return v.inSpeech
	}
	defer outputTensor.Destroy()

	newStateData := make([]float32, 2*1*64)
	newState, err := ort.NewTensor(ort.NewShape(2, 1, 64), newStateData)
	if err != nil {
		return v.inSpeech
	}
	defer newState.Destroy()

	// Run inference
	inputs := []ort.Value{inputTensor, v.state, srTensor}
	outputs := []ort.Value{outputTensor, newState}

	if err := v.session.Run(inputs, outputs); err != nil {
		return v.inSpeech
	}

	// Update state for next call
	copy(v.state.GetData(), newState.GetData())

	// Check speech probability
	prob := outputTensor.GetData()[0]
	v.inSpeech = prob >= v.threshold
	return v.inSpeech
}

// Reset clears the hidden state.
func (v *SileroVAD) Reset() {
	v.inSpeech = false
	for i := range v.state.GetData() {
		v.state.GetData()[i] = 0
	}
}

// newRMSVAD creates an RMSVAD (fallback when Silero model unavailable).
func newRMSVAD() VAD {
	return &RMSVAD{
		speechThreshold:  0.015,
		silenceThreshold: 0.008,
		speechFrames:     3,
		silenceFrames:    30,
	}
}

// RMSVAD is duplicated here for the desktop build tag.
// Both vad_rms.go (!desktop) and vad_silero.go (desktop) need it.
type RMSVAD struct {
	speechThreshold  float64
	silenceThreshold float64
	speechFrames     int
	silenceFrames    int
	inSpeech         bool
	speechCount      int
	silenceCount     int
}

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

func (v *RMSVAD) Reset() {
	v.inSpeech = false
	v.speechCount = 0
	v.silenceCount = 0
}

// sileroModelPath returns the expected location of silero_vad.onnx.
// Uses the centralized ModelsDir() so models are downloaded on demand.
func sileroModelPath() string {
	return filepath.Join(ModelsDir(), "silero_vad.onnx")
}

// onnxRuntimeLibPath returns the platform-specific path to the ONNX Runtime shared library.
// Checks ModelsDir() first (downloaded on demand), then bundled/system paths.
func onnxRuntimeLibPath() string {
	// Check downloaded runtime in ModelsDir() first
	onnx := onnxRuntimeModel()
	if onnx.Name != "" {
		downloaded := filepath.Join(ModelsDir(), onnx.Name)
		if _, err := os.Stat(downloaded); err == nil {
			return downloaded
		}
	}

	// Fall back to bundled/system paths
	switch runtime.GOOS {
	case "darwin":
		exe, _ := os.Executable()
		return filepath.Join(filepath.Dir(exe), "..", "Frameworks", "libonnxruntime.dylib")
	case "linux":
		return "/usr/lib/libonnxruntime.so"
	case "windows":
		return "onnxruntime.dll"
	}
	return ""
}
