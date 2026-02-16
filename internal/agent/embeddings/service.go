package embeddings

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/db"
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
	queries  *db.Queries
	sqlDB    *sql.DB // Keep for raw queries (FTS)
	provider Provider
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

	svc := &Service{
		queries:  db.New(cfg.DB),
		sqlDB:    cfg.DB,
		provider: cfg.Provider,
	}

	// Evict stale cache entries (older than 30 days) on startup
	cutoff := sql.NullTime{Time: time.Now().AddDate(0, 0, -30), Valid: true}
	_ = svc.queries.CleanOldEmbeddingCache(context.Background(), cutoff)

	return svc, nil
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
		if embedding, ok := s.getCached(ctx, hash, model); ok {
			results[i] = embedding
		} else {
			uncachedIndices = append(uncachedIndices, i)
			uncachedTexts = append(uncachedTexts, text)
		}
	}

	// Generate embeddings for uncached texts (with retry on transient errors)
	if len(uncachedTexts) > 0 {
		var embeddings [][]float32
		var err error
		for attempt := 0; attempt < 3; attempt++ {
			embeddings, err = provider.Embed(ctx, uncachedTexts)
			if err == nil {
				break
			}
			// Don't retry on auth/client errors (4xx)
			errStr := err.Error()
			if containsAny(errStr, "401", "403", "400", "Unauthorized", "invalid_api_key", "Bad Request") {
				break
			}
			// Exponential backoff: 500ms, 2s, 8s
			backoff := time.Duration(1<<uint(attempt*2)) * 500 * time.Millisecond
			fmt.Printf("[Embeddings] Attempt %d failed: %v — retrying in %v\n", attempt+1, err, backoff)
			select {
			case <-time.After(backoff):
			case <-ctx.Done():
				return nil, ctx.Err()
			}
		}
		if err != nil {
			return nil, fmt.Errorf("failed to generate embeddings: %w", err)
		}

		// Store in cache and results
		for j, embedding := range embeddings {
			idx := uncachedIndices[j]
			results[idx] = embedding
			hash := hashText(uncachedTexts[j])
			s.setCached(ctx, hash, model, provider.Dimensions(), embedding)
		}
	}

	return results, nil
}

// getCached retrieves an embedding from the cache using sqlc
func (s *Service) getCached(ctx context.Context, contentHash, model string) ([]float32, bool) {
	cached, err := s.queries.GetEmbeddingCache(ctx, db.GetEmbeddingCacheParams{
		ContentHash: contentHash,
		Model:       model,
	})
	if err != nil {
		return nil, false
	}

	embedding, err := blobToFloats(cached.Embedding)
	if err != nil {
		return nil, false
	}

	return embedding, true
}

// setCached stores an embedding in the cache using sqlc
func (s *Service) setCached(ctx context.Context, contentHash, model string, dimensions int, embedding []float32) {
	blob := floatsToBlob(embedding)
	s.queries.UpsertEmbeddingCache(ctx, db.UpsertEmbeddingCacheParams{
		ContentHash: contentHash,
		Embedding:   blob,
		Model:       model,
		Dimensions:  int64(dimensions),
	})
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

// GetDB returns the underlying database connection for raw queries (FTS)
func (s *Service) GetDB() *sql.DB {
	return s.sqlDB
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

// floatsToBlob converts a float32 slice to a byte slice
func floatsToBlob(floats []float32) []byte {
	data, _ := json.Marshal(floats)
	return data
}

// containsAny returns true if s contains any of the substrings.
func containsAny(s string, subs ...string) bool {
	for _, sub := range subs {
		if strings.Contains(s, sub) {
			return true
		}
	}
	return false
}

// blobToFloats converts a byte slice to a float32 slice
func blobToFloats(blob []byte) ([]float32, error) {
	var floats []float32
	if err := json.Unmarshal(blob, &floats); err != nil {
		return nil, err
	}
	return floats, nil
}
