package voice

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"

	"github.com/neboloop/nebo/internal/defaults"
)

// ModelManifest describes a voice model that must be present for duplex to work.
type ModelManifest struct {
	Name   string `json:"name"`   // e.g. "ggml-base.en.bin"
	URL    string `json:"url"`    // CDN download URL
	Size   int64  `json:"size"`   // expected size in bytes
	SHA256 string `json:"sha256"` // hex-encoded checksum
}

// DownloadProgress is emitted during model downloads.
type DownloadProgress struct {
	Model      string `json:"model"`
	Downloaded int64  `json:"downloaded"`
	Total      int64  `json:"total"`
	Done       bool   `json:"done"`
	Error      string `json:"error,omitempty"`
}

const cdnBase = "https://cdn.neboloop.com/voice/"

// ModelsDir returns the voice models directory inside the Nebo data dir.
// Creates the directory if it does not exist.
func ModelsDir() string {
	dataDir, err := defaults.DataDir()
	if err != nil {
		home, _ := os.UserHomeDir()
		dataDir = filepath.Join(home, ".config", "nebo")
	}
	dir := filepath.Join(dataDir, "voice")
	os.MkdirAll(dir, 0755)
	return dir
}

// VoiceModelsReady returns true if all required models are downloaded and present.
func VoiceModelsReady() bool {
	dir := ModelsDir()
	for _, m := range RequiredModels() {
		if !modelPresent(filepath.Join(dir, m.Name), m) {
			return false
		}
	}
	return true
}

// modelPresent checks if a model file exists and looks valid.
// When SHA256 is set, requires exact size match. Otherwise just checks file exists and is non-empty.
func modelPresent(path string, m ModelManifest) bool {
	info, err := os.Stat(path)
	if err != nil {
		return false
	}
	if m.SHA256 != "" {
		return info.Size() == m.Size
	}
	return info.Size() > 0
}

// ModelStatus returns the download status of each required model.
func ModelStatus() []map[string]any {
	dir := ModelsDir()
	var result []map[string]any
	for _, m := range RequiredModels() {
		path := filepath.Join(dir, m.Name)
		result = append(result, map[string]any{
			"name":       m.Name,
			"size":       m.Size,
			"downloaded": modelPresent(path, m),
		})
	}
	return result
}

// DownloadModels downloads any missing required models.
// The progress callback is called for each chunk downloaded.
func DownloadModels(ctx context.Context, progress func(DownloadProgress)) error {
	dir := ModelsDir()

	for _, m := range RequiredModels() {
		path := filepath.Join(dir, m.Name)

		// Skip if already downloaded
		if modelPresent(path, m) {
			if progress != nil {
				progress(DownloadProgress{Model: m.Name, Downloaded: m.Size, Total: m.Size, Done: true})
			}
			continue
		}

		if err := downloadModel(ctx, m, dir, progress); err != nil {
			if progress != nil {
				progress(DownloadProgress{Model: m.Name, Error: err.Error()})
			}
			return fmt.Errorf("failed to download %s: %w", m.Name, err)
		}
	}
	return nil
}

// downloadModel downloads a single model file with progress reporting.
func downloadModel(ctx context.Context, m ModelManifest, dir string, progress func(DownloadProgress)) error {
	req, err := http.NewRequestWithContext(ctx, "GET", m.URL, nil)
	if err != nil {
		return err
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("HTTP %d from %s", resp.StatusCode, m.URL)
	}

	// Use Content-Length for accurate progress; fall back to manifest estimate
	total := m.Size
	if resp.ContentLength > 0 {
		total = resp.ContentLength
	}

	// Write to temp file first, then rename for atomicity
	tmpPath := filepath.Join(dir, m.Name+".tmp")
	f, err := os.Create(tmpPath)
	if err != nil {
		return err
	}
	defer func() {
		f.Close()
		os.Remove(tmpPath) // clean up on failure
	}()

	hasher := sha256.New()
	writer := io.MultiWriter(f, hasher)

	var downloaded int64
	buf := make([]byte, 64*1024) // 64KB chunks

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		n, readErr := resp.Body.Read(buf)
		if n > 0 {
			if _, err := writer.Write(buf[:n]); err != nil {
				return err
			}
			downloaded += int64(n)
			if progress != nil {
				progress(DownloadProgress{Model: m.Name, Downloaded: downloaded, Total: total})
			}
		}
		if readErr == io.EOF {
			break
		}
		if readErr != nil {
			return readErr
		}
	}

	f.Close()

	// Verify checksum if provided
	if m.SHA256 != "" {
		got := hex.EncodeToString(hasher.Sum(nil))
		if got != m.SHA256 {
			return fmt.Errorf("checksum mismatch for %s: expected %s, got %s", m.Name, m.SHA256, got)
		}
	}

	// Atomic rename
	finalPath := filepath.Join(dir, m.Name)
	if err := os.Rename(tmpPath, finalPath); err != nil {
		return err
	}

	if progress != nil {
		progress(DownloadProgress{Model: m.Name, Downloaded: total, Total: total, Done: true})
	}
	return nil
}

// RequiredModels returns all voice models needed for duplex voice.
// Same set on every platform — downloaded on first use.
func RequiredModels() []ModelManifest {
	models := []ModelManifest{
		{
			Name:   "ggml-base.en.bin",
			URL:    cdnBase + "ggml-base.en.bin",
			Size:   147951465, // ~142MB — Whisper base English
			SHA256: "",        // TODO: populate after CDN upload
		},
		{
			Name:   "silero_vad.onnx",
			URL:    cdnBase + "silero_vad.onnx",
			Size:   2167808, // ~2MB — Silero VAD
			SHA256: "",
		},
		{
			Name:   "kokoro-v1.0.onnx",
			URL:    cdnBase + "kokoro-v1.0.onnx",
			Size:   83886080, // ~80MB — Kokoro TTS
			SHA256: "",
		},
		{
			Name:   "voices-v1.0.bin",
			URL:    cdnBase + "voices-v1.0.bin",
			Size:   52428800, // ~50MB — Kokoro voice embeddings
			SHA256: "",
		},
	}

	// ONNX Runtime shared library — platform-specific
	if onnx := onnxRuntimeModel(); onnx.Name != "" {
		models = append(models, onnx)
	}

	return models
}

// onnxRuntimeModel returns the platform-specific ONNX Runtime shared library manifest.
// Required by Silero VAD and Kokoro TTS.
func onnxRuntimeModel() ModelManifest {
	var name string
	switch runtime.GOOS {
	case "darwin":
		name = fmt.Sprintf("libonnxruntime.%s.dylib", runtime.GOARCH)
	case "linux":
		name = fmt.Sprintf("libonnxruntime.%s.so", runtime.GOARCH)
	case "windows":
		name = fmt.Sprintf("onnxruntime.%s.dll", runtime.GOARCH)
	default:
		return ModelManifest{}
	}
	return ModelManifest{
		Name:   name,
		URL:    cdnBase + name,
		Size:   0, // determined by Content-Length at download time
		SHA256: "",
	}
}
