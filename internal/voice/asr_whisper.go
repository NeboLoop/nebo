//go:build cgo

package voice

/*
#cgo CFLAGS: -I${SRCDIR}/../../third_party/whisper/include
#cgo LDFLAGS: -L${SRCDIR}/../../third_party/whisper/lib -lwhisper -lggml -lggml-base -lggml-cpu -lm -lstdc++
#cgo darwin LDFLAGS: -lggml-metal -lggml-blas -framework Accelerate -framework Foundation -framework Metal -framework MetalKit

#include <whisper.h>
#include <stdlib.h>

// whisper_full_default_params returns a copy of the default params struct.
// This is a helper because Go can't call C functions that return structs by value easily.
static struct whisper_full_params nebo_default_params() {
    return whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
}
*/
import "C"

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"unsafe"
)

var (
	whisperCtx  *C.struct_whisper_context
	whisperOnce sync.Once
	whisperErr  error
	whisperMu   sync.Mutex // serialize inference calls
)

// loadWhisperModel loads the whisper model from ModelsDir() once.
func loadWhisperModel() error {
	whisperOnce.Do(func() {
		modelPath := filepath.Join(ModelsDir(), "ggml-base.en.bin")
		if _, err := os.Stat(modelPath); err != nil {
			whisperErr = fmt.Errorf("whisper model not found at %s: %w", modelPath, err)
			return
		}

		cPath := C.CString(modelPath)
		defer C.free(unsafe.Pointer(cPath))

		params := C.whisper_context_default_params()
		whisperCtx = C.whisper_init_from_file_with_params(cPath, params)
		if whisperCtx == nil {
			whisperErr = fmt.Errorf("failed to load whisper model from %s", modelPath)
		}
	})
	return whisperErr
}

// TranscribePCM transcribes raw float32 PCM audio (16kHz mono) using embedded whisper.cpp.
func TranscribePCM(pcm []float32) (string, error) {
	if err := loadWhisperModel(); err != nil {
		return "", err
	}

	if len(pcm) == 0 {
		return "", nil
	}

	whisperMu.Lock()
	defer whisperMu.Unlock()

	params := C.nebo_default_params()
	params.language = C.CString("en")
	defer C.free(unsafe.Pointer(params.language))
	params.n_threads = 4
	params.no_timestamps = true
	params.print_progress = false
	params.print_realtime = false
	params.print_special = false
	params.print_timestamps = false

	rc := C.whisper_full(whisperCtx, params, (*C.float)(unsafe.Pointer(&pcm[0])), C.int(len(pcm)))
	if rc != 0 {
		return "", fmt.Errorf("whisper_full failed with code %d", rc)
	}

	nSegments := int(C.whisper_full_n_segments(whisperCtx))
	var segments []string
	for i := 0; i < nSegments; i++ {
		text := C.GoString(C.whisper_full_get_segment_text(whisperCtx, C.int(i)))
		text = strings.TrimSpace(text)
		if text != "" {
			segments = append(segments, text)
		}
	}

	return strings.Join(segments, " "), nil
}

// TranscribeFile transcribes a WAV file using embedded whisper.cpp.
// Used by TranscribeHandler for half-duplex voice.
func TranscribeFile(path string) (string, error) {
	if err := loadWhisperModel(); err != nil {
		return "", err
	}

	data, err := os.ReadFile(path)
	if err != nil {
		return "", fmt.Errorf("failed to read file: %w", err)
	}

	pcm, err := wavToFloat32(data)
	if err != nil {
		return "", fmt.Errorf("failed to decode WAV: %w", err)
	}

	return TranscribePCM(pcm)
}

// wavToFloat32 extracts float32 samples from a WAV file's raw bytes.
func wavToFloat32(data []byte) ([]float32, error) {
	// Find "data" chunk
	dataOffset := -1
	for i := 0; i < len(data)-8; i++ {
		if string(data[i:i+4]) == "data" {
			dataOffset = i + 8 // skip "data" + 4-byte size
			break
		}
	}
	if dataOffset < 0 {
		return nil, fmt.Errorf("no data chunk found in WAV")
	}

	raw := data[dataOffset:]
	nSamples := len(raw) / 2
	samples := make([]float32, nSamples)
	for i := 0; i < nSamples; i++ {
		val := int16(raw[i*2]) | int16(raw[i*2+1])<<8
		samples[i] = float32(val) / 32768.0
	}
	return samples, nil
}
