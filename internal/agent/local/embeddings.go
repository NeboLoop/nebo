package local

import (
	"context"
	"fmt"
	"math"
	"sync"

	"github.com/hybridgroup/yzma/pkg/llama"
)

// EmbeddingProvider implements embeddings.Provider using yzma (llama.cpp via purego).
// It loads a GGUF embedding model and generates embeddings locally with no external
// dependencies (no Ollama, no API keys required).
type EmbeddingProvider struct {
	manager *Manager
	spec    ModelSpec

	model      llama.Model
	vocab      llama.Vocab
	dimensions int

	mu     sync.Mutex
	loaded bool
}

// NewEmbeddingProvider creates a local embedding provider.
// Call Init() before use, or let Embed() auto-initialize.
func NewEmbeddingProvider(manager *Manager, spec ModelSpec) *EmbeddingProvider {
	return &EmbeddingProvider{
		manager: manager,
		spec:    spec,
	}
}

// Init loads the embedding model. Safe to call multiple times.
func (p *EmbeddingProvider) Init() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.loaded {
		return nil
	}

	// Ensure yzma runtime is initialized
	if err := p.manager.Init(); err != nil {
		return fmt.Errorf("init yzma: %w", err)
	}

	// Download model if not present
	modelPath, err := p.manager.EnsureModel(p.spec)
	if err != nil {
		return fmt.Errorf("ensure model: %w", err)
	}

	// Load model with default params, all layers on GPU
	params := llama.ModelDefaultParams()
	params.NGpuLayers = 99 // Offload everything to GPU (Metal on macOS)

	model, err := llama.ModelLoadFromFile(modelPath, params)
	if err != nil {
		return fmt.Errorf("load model %s: %w", p.spec.Name, err)
	}

	p.model = model
	p.vocab = llama.ModelGetVocab(model)
	p.dimensions = int(llama.ModelNEmbd(model))
	p.loaded = true

	fmt.Printf("[Local] Embedding model loaded: %s (%d dimensions)\n", p.spec.Name, p.dimensions)
	return nil
}

// Embed generates embeddings for the given texts.
// Implements embeddings.Provider interface.
func (p *EmbeddingProvider) Embed(ctx context.Context, texts []string) ([][]float32, error) {
	if err := p.Init(); err != nil {
		return nil, err
	}

	results := make([][]float32, len(texts))
	for i, text := range texts {
		embedding, err := p.embedOne(text)
		if err != nil {
			return nil, fmt.Errorf("embed text %d: %w", i, err)
		}
		results[i] = embedding
	}

	return results, nil
}

// embedOne generates an embedding for a single text using a fresh context per call.
// This avoids KV cache conflicts and is safe for concurrent use (caller holds no lock).
func (p *EmbeddingProvider) embedOne(text string) ([]float32, error) {
	p.mu.Lock()
	model := p.model
	vocab := p.vocab
	nEmbd := p.dimensions
	p.mu.Unlock()

	// Create a fresh context for this embedding
	ctxParams := llama.ContextDefaultParams()
	ctxParams.Embeddings = 1 // Enable embedding mode
	ctxParams.NCtx = 2048   // Sufficient for most embedding inputs
	ctxParams.NBatch = 2048
	ctxParams.NUbatch = 2048
	ctxParams.NThreads = 4

	lctx, err := llama.InitFromModel(model, ctxParams)
	if err != nil {
		return nil, fmt.Errorf("create context: %w", err)
	}
	defer llama.Free(lctx)

	// Enable embedding extraction
	llama.SetEmbeddings(lctx, true)

	// Tokenize the input
	tokens := llama.Tokenize(vocab, text, true, false)
	if len(tokens) == 0 {
		return make([]float32, nEmbd), nil
	}

	// Truncate if too long for context
	maxTokens := int(ctxParams.NCtx) - 1
	if len(tokens) > maxTokens {
		tokens = tokens[:maxTokens]
	}

	// Create batch and decode
	batch := llama.BatchGetOne(tokens)
	batch.SetLogit(int32(len(tokens)-1), true)

	if _, err := llama.Decode(lctx, batch); err != nil {
		return nil, fmt.Errorf("decode: %w", err)
	}

	// Extract embeddings using sequence-level pooling
	raw, err := llama.GetEmbeddingsSeq(lctx, 0, int32(nEmbd))
	if err != nil {
		return nil, fmt.Errorf("get embeddings: %w", err)
	}

	// Copy and normalize
	embedding := make([]float32, len(raw))
	copy(embedding, raw)
	normalize(embedding)

	return embedding, nil
}

// Dimensions returns the embedding dimension size.
// Implements embeddings.Provider interface.
func (p *EmbeddingProvider) Dimensions() int {
	if !p.loaded {
		return 0
	}
	return p.dimensions
}

// Model returns the model identifier.
// Implements embeddings.Provider interface.
func (p *EmbeddingProvider) Model() string {
	return p.spec.Name
}

// Close releases the embedding model resources.
func (p *EmbeddingProvider) Close() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.loaded && p.model != 0 {
		llama.ModelFree(p.model)
		p.model = 0
		p.loaded = false
	}
}

// normalize L2-normalizes a vector in place.
func normalize(v []float32) {
	var sum float64
	for _, x := range v {
		sum += float64(x) * float64(x)
	}
	if sum == 0 {
		return
	}
	norm := float32(math.Sqrt(sum))
	for i := range v {
		v[i] /= norm
	}
}
