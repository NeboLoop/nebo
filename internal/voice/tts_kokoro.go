//go:build cgo

package voice

import (
	"fmt"
	"math"
	"os"
	"path/filepath"

	ort "github.com/yalue/onnxruntime_go"
)

// kokoroTTS holds the loaded Kokoro ONNX model for text-to-speech synthesis.
type kokoroTTS struct {
	session *ort.DynamicAdvancedSession
	voices  map[string][]float32 // voice name → embedding vector
}

var globalKokoro *kokoroTTS

// SynthesizeSpeechForDuplex generates TTS audio for the duplex pipeline.
// On desktop builds, uses Kokoro ONNX if available, falls back to SynthesizeSpeech.
func SynthesizeSpeechForDuplex(text, voice string) ([]byte, error) {
	k, err := getKokoro()
	if err != nil || k == nil {
		// Kokoro not available — fall back to ElevenLabs/say
		data, _, synthErr := SynthesizeSpeech(text, voice, 1.0)
		return data, synthErr
	}
	return k.synthesize(text, voice)
}

func getKokoro() (*kokoroTTS, error) {
	if globalKokoro != nil {
		return globalKokoro, nil
	}

	modelPath := kokoroModelPath()
	if _, err := os.Stat(modelPath); err != nil {
		return nil, nil // model not bundled — not an error
	}

	voicesPath := kokoroVoicesPath()
	if _, err := os.Stat(voicesPath); err != nil {
		return nil, nil
	}

	k, err := newKokoroTTS(modelPath, voicesPath)
	if err != nil {
		return nil, err
	}
	globalKokoro = k
	return k, nil
}

func newKokoroTTS(modelPath, voicesPath string) (*kokoroTTS, error) {
	// Initialize ONNX Runtime (may already be initialized by Silero VAD)
	libPath := onnxRuntimeLibPath()
	if libPath != "" {
		ort.SetSharedLibraryPath(libPath)
	}
	if err := ort.InitializeEnvironment(); err != nil {
		if err.Error() != "the ONNX runtime is already initialized" {
			return nil, fmt.Errorf("failed to initialize ONNX runtime: %w", err)
		}
	}

	inputNames := []string{"tokens", "style", "speed"}
	outputNames := []string{"audio"}

	session, err := ort.NewDynamicAdvancedSession(
		modelPath,
		inputNames,
		outputNames,
		nil,
	)
	if err != nil {
		return nil, fmt.Errorf("failed to create Kokoro session: %w", err)
	}

	// Load voice embeddings from binary file
	voices, err := loadVoices(voicesPath)
	if err != nil {
		session.Destroy()
		return nil, fmt.Errorf("failed to load voices: %w", err)
	}

	return &kokoroTTS{
		session: session,
		voices:  voices,
	}, nil
}

// synthesize converts text to PCM audio using Kokoro ONNX.
// Returns 24kHz mono 16-bit PCM as raw bytes.
func (k *kokoroTTS) synthesize(text, voice string) ([]byte, error) {
	if voice == "" {
		voice = "af_heart" // default Kokoro voice
	}

	// Get voice embedding
	style, ok := k.voices[voice]
	if !ok {
		// Try first available voice
		for _, v := range k.voices {
			style = v
			break
		}
		if style == nil {
			return nil, fmt.Errorf("no voices loaded")
		}
	}

	// Phonemize text to token IDs
	tokens := Phonemize(text)
	if len(tokens) == 0 {
		return nil, fmt.Errorf("phonemization produced no tokens")
	}

	// Create input tensors
	tokenTensor, err := ort.NewTensor(ort.NewShape(1, int64(len(tokens))), tokens)
	if err != nil {
		return nil, fmt.Errorf("failed to create token tensor: %w", err)
	}
	defer tokenTensor.Destroy()

	styleTensor, err := ort.NewTensor(ort.NewShape(1, int64(len(style))), style)
	if err != nil {
		return nil, fmt.Errorf("failed to create style tensor: %w", err)
	}
	defer styleTensor.Destroy()

	speedData := []float32{1.0}
	speedTensor, err := ort.NewTensor(ort.NewShape(1), speedData)
	if err != nil {
		return nil, fmt.Errorf("failed to create speed tensor: %w", err)
	}
	defer speedTensor.Destroy()

	// Output tensor — Kokoro outputs variable-length audio
	// We allocate a generous buffer; actual length comes from the model
	maxSamples := int64(len(tokens)) * 1024 // rough estimate
	if maxSamples < 24000 {
		maxSamples = 24000 // at least 1 second
	}
	outputData := make([]float32, maxSamples)
	outputTensor, err := ort.NewTensor(ort.NewShape(1, maxSamples), outputData)
	if err != nil {
		return nil, fmt.Errorf("failed to create output tensor: %w", err)
	}
	defer outputTensor.Destroy()

	// Run inference
	inputs := []ort.Value{tokenTensor, styleTensor, speedTensor}
	outputs := []ort.Value{outputTensor}
	if err := k.session.Run(inputs, outputs); err != nil {
		return nil, fmt.Errorf("Kokoro inference failed: %w", err)
	}

	// Convert float32 audio to int16 PCM bytes
	audio := outputTensor.GetData()
	pcm := make([]byte, len(audio)*2)
	for i, sample := range audio {
		// Clamp to [-1, 1]
		if sample > 1.0 {
			sample = 1.0
		} else if sample < -1.0 {
			sample = -1.0
		}
		val := int16(sample * 32767)
		pcm[i*2] = byte(val)
		pcm[i*2+1] = byte(val >> 8)
	}

	return pcm, nil
}

// loadVoices loads voice embeddings from the binary voices file.
// Format: 4-byte name length, name bytes, 4-byte embedding length, float32 values.
func loadVoices(path string) (map[string][]float32, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	voices := make(map[string][]float32)
	offset := 0

	for offset < len(data)-8 {
		// Read name length (4 bytes, little-endian)
		if offset+4 > len(data) {
			break
		}
		nameLen := int(data[offset]) | int(data[offset+1])<<8 | int(data[offset+2])<<16 | int(data[offset+3])<<24
		offset += 4

		if offset+nameLen > len(data) {
			break
		}
		name := string(data[offset : offset+nameLen])
		offset += nameLen

		// Read embedding length (4 bytes = number of float32s)
		if offset+4 > len(data) {
			break
		}
		embLen := int(data[offset]) | int(data[offset+1])<<8 | int(data[offset+2])<<16 | int(data[offset+3])<<24
		offset += 4

		if offset+embLen*4 > len(data) {
			break
		}

		embedding := make([]float32, embLen)
		for i := 0; i < embLen; i++ {
			bits := uint32(data[offset]) | uint32(data[offset+1])<<8 | uint32(data[offset+2])<<16 | uint32(data[offset+3])<<24
			embedding[i] = float32frombits(bits)
			offset += 4
		}
		voices[name] = embedding
	}

	if len(voices) == 0 {
		return nil, fmt.Errorf("no voices found in %s", path)
	}
	return voices, nil
}

func float32frombits(b uint32) float32 {
	return math.Float32frombits(b)
}

// kokoroModelPath returns the Kokoro TTS ONNX model path.
// Uses the centralized ModelsDir() so models are downloaded on demand.
func kokoroModelPath() string {
	return filepath.Join(ModelsDir(), "kokoro-v1.0.onnx")
}

// kokoroVoicesPath returns the Kokoro voice embeddings path.
func kokoroVoicesPath() string {
	return filepath.Join(ModelsDir(), "voices-v1.0.bin")
}
