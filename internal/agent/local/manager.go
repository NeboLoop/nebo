package local

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"

	"github.com/hybridgroup/yzma/pkg/download"
	"github.com/hybridgroup/yzma/pkg/llama"
)

// ModelSpec describes a GGUF model to download and use.
type ModelSpec struct {
	Name string // Human-readable name (e.g., "qwen3-embedding")
	URL  string // HuggingFace download URL
	File string // Local filename (e.g., "qwen3-embedding-0.6b-q8_0.gguf")
}

// DefaultEmbeddingModel is the default embedding model for local inference.
var DefaultEmbeddingModel = ModelSpec{
	Name: "qwen3-embedding-0.6b",
	URL:  "https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF/resolve/main/qwen3-embedding-0.6b-q8_0.gguf",
	File: "qwen3-embedding-0.6b-q8_0.gguf",
}

// DefaultChatModel is the default chat model for local inference.
var DefaultChatModel = ModelSpec{
	Name: "qwen3-4b-instruct",
	URL:  "https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/qwen3-4b-q4_k_m.gguf",
	File: "qwen3-4b-q4_k_m.gguf",
}

// Manager handles llama.cpp library installation, GGUF model downloads,
// and yzma lifecycle management. It ensures everything is ready for
// local inference without requiring Ollama or any external process.
type Manager struct {
	dataDir string // Nebo data directory (e.g., ~/Library/Application Support/Nebo)
	libDir  string // llama.cpp shared libraries
	modelDir string // GGUF model files

	initialized bool
	mu          sync.Mutex
}

// NewManager creates a new local inference manager.
func NewManager(dataDir string) *Manager {
	return &Manager{
		dataDir:  dataDir,
		libDir:   filepath.Join(dataDir, "lib"),
		modelDir: filepath.Join(dataDir, "models"),
	}
}

// Init ensures llama.cpp libraries are installed and initializes yzma.
// This must be called before using any model. Safe to call multiple times.
func (m *Manager) Init() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.initialized {
		return nil
	}

	// Ensure directories exist
	if err := os.MkdirAll(m.libDir, 0755); err != nil {
		return fmt.Errorf("create lib dir: %w", err)
	}
	if err := os.MkdirAll(m.modelDir, 0755); err != nil {
		return fmt.Errorf("create model dir: %w", err)
	}

	// Check if llama.cpp library is installed
	libName := download.LibraryName(runtime.GOOS)
	libPath := filepath.Join(m.libDir, libName)

	if _, err := os.Stat(libPath); os.IsNotExist(err) {
		fmt.Println("[Local] llama.cpp library not found, downloading...")
		if err := m.installLibrary(); err != nil {
			return fmt.Errorf("install llama.cpp: %w", err)
		}
		fmt.Println("[Local] llama.cpp library installed")
	}

	// Load and initialize yzma
	llama.Load(m.libDir)
	llama.LogSet(llama.LogSilent())
	llama.Init()

	m.initialized = true
	return nil
}

// Close shuts down yzma. Call when the application exits.
func (m *Manager) Close() {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.initialized {
		llama.Close()
		m.initialized = false
	}
}

// EnsureModel downloads a GGUF model if it doesn't exist locally.
// Returns the full path to the model file.
func (m *Manager) EnsureModel(spec ModelSpec) (string, error) {
	modelPath := filepath.Join(m.modelDir, spec.File)

	if _, err := os.Stat(modelPath); err == nil {
		return modelPath, nil // Already exists
	}

	fmt.Printf("[Local] Downloading model %s...\n", spec.Name)
	if err := download.GetModel(spec.URL, modelPath); err != nil {
		// Clean up partial download
		os.Remove(modelPath)
		return "", fmt.Errorf("download model %s: %w", spec.Name, err)
	}
	fmt.Printf("[Local] Model %s ready\n", spec.Name)

	return modelPath, nil
}

// ModelPath returns the local path for a model spec (without downloading).
func (m *Manager) ModelPath(spec ModelSpec) string {
	return filepath.Join(m.modelDir, spec.File)
}

// LibDir returns the library directory path.
func (m *Manager) LibDir() string {
	return m.libDir
}

// installLibrary downloads the prebuilt llama.cpp libraries for the current platform.
func (m *Manager) installLibrary() error {
	version, err := download.LlamaLatestVersion()
	if err != nil {
		return fmt.Errorf("get llama.cpp version: %w", err)
	}

	// Use Metal on macOS (Apple Silicon), CPU elsewhere as fallback
	processor := "cpu"
	if runtime.GOOS == "darwin" {
		processor = "metal"
	}

	fmt.Printf("[Local] Installing llama.cpp %s (%s/%s/%s)\n", version, runtime.GOOS, runtime.GOARCH, processor)

	if err := download.Get(runtime.GOARCH, runtime.GOOS, processor, version, m.libDir); err != nil {
		// Fallback to CPU if GPU-specific download fails
		if processor != "cpu" {
			fmt.Printf("[Local] %s download failed, falling back to CPU\n", processor)
			if err := download.Get(runtime.GOARCH, runtime.GOOS, "cpu", version, m.libDir); err != nil {
				return fmt.Errorf("download llama.cpp (cpu fallback): %w", err)
			}
			return nil
		}
		return fmt.Errorf("download llama.cpp: %w", err)
	}

	return nil
}

