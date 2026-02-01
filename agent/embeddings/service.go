package embeddings

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"sync"
)

// Provider interface for embedding providers
type Provider interface {
	// Embed generates embeddings for the given texts
	Embed(ctx context.Context, texts []string) ([][]float32, error)
	// Dimensions returns the embedding dimension size
	Dimensions() int
	// Model returns the model identifier
	Model() string
}

// Service provides embedding generation and caching
type Service struct {
	db       *sql.DB
	provider Provider
	cache    *Cache
	mu       sync.RWMutex
}

// Config configures the embedding service
type Config struct {
	DB       *sql.DB
	Provider Provider
}

// NewService creates a new embedding service
func NewService(cfg Config) (*Service, error) {
	if cfg.DB == nil {
		return nil, fmt.Errorf("database connection required")
	}

	return &Service{
		db:       cfg.DB,
		provider: cfg.Provider,
		cache:    NewCache(cfg.DB),
	}, nil
}

// SetProvider sets the embedding provider
func (s *Service) SetProvider(p Provider) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.provider = p
}

// HasProvider returns true if an embedding provider is configured
func (s *Service) HasProvider() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.provider != nil
}

// Embed generates embeddings for the given texts
func (s *Service) Embed(ctx context.Context, texts []string) ([][]float32, error) {
	s.mu.RLock()
	provider := s.provider
	s.mu.RUnlock()

	if provider == nil {
		return nil, fmt.Errorf("no embedding provider configured")
	}

	if len(texts) == 0 {
		return nil, nil
	}

	// Check cache first
	model := provider.Model()
	results := make([][]float32, len(texts))
	uncachedIndices := make([]int, 0)
	uncachedTexts := make([]string, 0)

	for i, text := range texts {
		hash := hashText(text)
		if embedding, ok := s.cache.Get(hash, model); ok {
			results[i] = embedding
		} else {
			uncachedIndices = append(uncachedIndices, i)
			uncachedTexts = append(uncachedTexts, text)
		}
	}

	// Generate embeddings for uncached texts
	if len(uncachedTexts) > 0 {
		embeddings, err := provider.Embed(ctx, uncachedTexts)
		if err != nil {
			return nil, fmt.Errorf("failed to generate embeddings: %w", err)
		}

		// Store in cache and results
		for j, embedding := range embeddings {
			idx := uncachedIndices[j]
			results[idx] = embedding
			hash := hashText(uncachedTexts[j])
			s.cache.Set(hash, model, provider.Dimensions(), embedding)
		}
	}

	return results, nil
}

// EmbedOne generates an embedding for a single text
func (s *Service) EmbedOne(ctx context.Context, text string) ([]float32, error) {
	results, err := s.Embed(ctx, []string{text})
	if err != nil {
		return nil, err
	}
	if len(results) == 0 {
		return nil, fmt.Errorf("no embedding generated")
	}
	return results[0], nil
}

// Dimensions returns the embedding dimensions
func (s *Service) Dimensions() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.provider == nil {
		return 0
	}
	return s.provider.Dimensions()
}

// Model returns the current model identifier
func (s *Service) Model() string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.provider == nil {
		return ""
	}
	return s.provider.Model()
}

// CosineSimilarity computes the cosine similarity between two vectors
func CosineSimilarity(a, b []float32) float64 {
	if len(a) != len(b) || len(a) == 0 {
		return 0
	}

	var dotProduct, normA, normB float64
	for i := range a {
		dotProduct += float64(a[i]) * float64(b[i])
		normA += float64(a[i]) * float64(a[i])
		normB += float64(b[i]) * float64(b[i])
	}

	if normA == 0 || normB == 0 {
		return 0
	}

	return dotProduct / (math.Sqrt(normA) * math.Sqrt(normB))
}

// hashText creates a SHA256 hash of the text
func hashText(text string) string {
	hash := sha256.Sum256([]byte(text))
	return hex.EncodeToString(hash[:])
}

// Cache provides embedding caching backed by SQLite
type Cache struct {
	db *sql.DB
}

// NewCache creates a new embedding cache
func NewCache(db *sql.DB) *Cache {
	return &Cache{db: db}
}

// Get retrieves an embedding from the cache
func (c *Cache) Get(contentHash, model string) ([]float32, bool) {
	if c.db == nil {
		return nil, false
	}

	var embeddingBlob []byte
	err := c.db.QueryRow(
		`SELECT embedding FROM embedding_cache WHERE content_hash = ? AND model = ?`,
		contentHash, model,
	).Scan(&embeddingBlob)

	if err != nil {
		return nil, false
	}

	embedding, err := blobToFloats(embeddingBlob)
	if err != nil {
		return nil, false
	}

	return embedding, true
}

// Set stores an embedding in the cache
func (c *Cache) Set(contentHash, model string, dimensions int, embedding []float32) {
	if c.db == nil {
		return
	}

	blob := floatsToBlob(embedding)
	_, _ = c.db.Exec(
		`INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, model, dimensions, created_at)
		 VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)`,
		contentHash, blob, model, dimensions,
	)
}

// floatsToBlob converts a float32 slice to a byte slice
func floatsToBlob(floats []float32) []byte {
	data, _ := json.Marshal(floats)
	return data
}

// blobToFloats converts a byte slice to a float32 slice
func blobToFloats(blob []byte) ([]float32, error) {
	var floats []float32
	if err := json.Unmarshal(blob, &floats); err != nil {
		return nil, err
	}
	return floats, nil
}
