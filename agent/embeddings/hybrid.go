package embeddings

import (
	"context"
	"database/sql"
	"fmt"
	"sort"
	"strings"
)

// HybridSearcher provides hybrid search combining FTS5 and vector search
type HybridSearcher struct {
	db       *sql.DB
	embedder *Service
}

// HybridSearchConfig configures the hybrid searcher
type HybridSearchConfig struct {
	DB       *sql.DB
	Embedder *Service
}

// NewHybridSearcher creates a new hybrid searcher
func NewHybridSearcher(cfg HybridSearchConfig) *HybridSearcher {
	return &HybridSearcher{
		db:       cfg.DB,
		embedder: cfg.Embedder,
	}
}

// SearchResult represents a search result
type SearchResult struct {
	ID          int64   `json:"id"`
	Key         string  `json:"key"`
	Value       string  `json:"value"`
	Namespace   string  `json:"namespace"`
	Score       float64 `json:"score"`
	VectorScore float64 `json:"vector_score,omitempty"`
	TextScore   float64 `json:"text_score,omitempty"`
	Source      string  `json:"source,omitempty"`
}

// SearchOptions configures search behavior
type SearchOptions struct {
	Namespace    string
	Limit        int
	VectorWeight float64 // Weight for vector search (default: 0.7)
	TextWeight   float64 // Weight for FTS search (default: 0.3)
	UserID       string  // User ID for user-scoped queries
}

// DefaultSearchOptions returns default search options
func DefaultSearchOptions() SearchOptions {
	return SearchOptions{
		Namespace:    "default",
		Limit:        10,
		VectorWeight: 0.7,
		TextWeight:   0.3,
		UserID:       "",
	}
}

// Search performs hybrid search combining FTS5 and vector search
func (h *HybridSearcher) Search(ctx context.Context, query string, opts SearchOptions) ([]SearchResult, error) {
	if opts.Limit <= 0 {
		opts.Limit = 10
	}
	if opts.VectorWeight == 0 && opts.TextWeight == 0 {
		opts.VectorWeight = 0.7
		opts.TextWeight = 0.3
	}

	// Get FTS results (user-scoped)
	ftsResults, err := h.searchFTS(query, opts.Namespace, opts.UserID, opts.Limit*2)
	if err != nil {
		// FTS might fail, fall back to LIKE search
		ftsResults, err = h.searchLike(query, opts.Namespace, opts.UserID, opts.Limit*2)
		if err != nil {
			return nil, fmt.Errorf("text search failed: %w", err)
		}
	}

	// Get vector results if embedder is available (user-scoped)
	var vectorResults []SearchResult
	if h.embedder != nil && h.embedder.HasProvider() {
		vectorResults, err = h.searchVector(ctx, query, opts.Namespace, opts.UserID, opts.Limit*2)
		if err != nil {
			// Vector search failure is not fatal, continue with FTS only
			fmt.Printf("[HybridSearch] Vector search failed: %v\n", err)
		}
	}

	// Merge results
	merged := h.mergeResults(ftsResults, vectorResults, opts.VectorWeight, opts.TextWeight)

	// Limit results
	if len(merged) > opts.Limit {
		merged = merged[:opts.Limit]
	}

	return merged, nil
}

// searchFTS performs full-text search using FTS5 (user-scoped)
func (h *HybridSearcher) searchFTS(query, namespace, userID string, limit int) ([]SearchResult, error) {
	// Build FTS query
	ftsQuery := buildFTSQuery(query)
	if ftsQuery == "" {
		return nil, nil
	}

	rows, err := h.db.Query(`
		SELECT m.id, m.key, m.value, m.namespace, bm25(memories_fts) as rank
		FROM memories m
		JOIN memories_fts f ON m.id = f.rowid
		WHERE memories_fts MATCH ? AND m.namespace = ? AND m.user_id = ?
		ORDER BY rank
		LIMIT ?
	`, ftsQuery, namespace, userID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var results []SearchResult
	for rows.Next() {
		var r SearchResult
		var rank float64
		if err := rows.Scan(&r.ID, &r.Key, &r.Value, &r.Namespace, &rank); err != nil {
			continue
		}
		r.TextScore = bm25RankToScore(rank)
		r.Source = "fts"
		results = append(results, r)
	}

	return results, nil
}

// searchLike performs fallback LIKE search (user-scoped)
func (h *HybridSearcher) searchLike(query, namespace, userID string, limit int) ([]SearchResult, error) {
	pattern := "%" + query + "%"

	rows, err := h.db.Query(`
		SELECT id, key, value, namespace
		FROM memories
		WHERE namespace = ? AND user_id = ? AND (key LIKE ? OR value LIKE ?)
		LIMIT ?
	`, namespace, userID, pattern, pattern, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var results []SearchResult
	for rows.Next() {
		var r SearchResult
		if err := rows.Scan(&r.ID, &r.Key, &r.Value, &r.Namespace); err != nil {
			continue
		}
		r.TextScore = 0.5 // Fixed score for LIKE matches
		r.Source = "like"
		results = append(results, r)
	}

	return results, nil
}

// searchVector performs vector similarity search (user-scoped)
func (h *HybridSearcher) searchVector(ctx context.Context, query, namespace, userID string, limit int) ([]SearchResult, error) {
	// Generate query embedding
	queryVec, err := h.embedder.EmbedOne(ctx, query)
	if err != nil {
		return nil, err
	}

	model := h.embedder.Model()

	// Get all embeddings for this namespace and user
	rows, err := h.db.Query(`
		SELECT c.id, m.key, c.text, m.namespace, e.embedding
		FROM memory_embeddings e
		JOIN memory_chunks c ON e.chunk_id = c.id
		JOIN memories m ON c.memory_id = m.id
		WHERE m.namespace = ? AND m.user_id = ? AND e.model = ?
	`, namespace, userID, model)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	type scoredResult struct {
		result SearchResult
		score  float64
	}

	var scored []scoredResult
	for rows.Next() {
		var r SearchResult
		var embeddingBlob []byte
		if err := rows.Scan(&r.ID, &r.Key, &r.Value, &r.Namespace, &embeddingBlob); err != nil {
			continue
		}

		embedding, err := blobToFloats(embeddingBlob)
		if err != nil {
			continue
		}

		similarity := CosineSimilarity(queryVec, embedding)
		r.VectorScore = similarity
		r.Source = "vector"
		scored = append(scored, scoredResult{result: r, score: similarity})
	}

	// Sort by score descending
	sort.Slice(scored, func(i, j int) bool {
		return scored[i].score > scored[j].score
	})

	// Limit results
	if len(scored) > limit {
		scored = scored[:limit]
	}

	results := make([]SearchResult, len(scored))
	for i, s := range scored {
		results[i] = s.result
	}

	return results, nil
}

// mergeResults combines FTS and vector results with weighted scoring
func (h *HybridSearcher) mergeResults(ftsResults, vectorResults []SearchResult, vectorWeight, textWeight float64) []SearchResult {
	// Create map by key for merging
	byKey := make(map[string]*SearchResult)

	// Process FTS results
	for _, r := range ftsResults {
		key := fmt.Sprintf("%s:%s", r.Namespace, r.Key)
		if existing, ok := byKey[key]; ok {
			existing.TextScore = r.TextScore
		} else {
			result := r
			byKey[key] = &result
		}
	}

	// Process vector results
	for _, r := range vectorResults {
		key := fmt.Sprintf("%s:%s", r.Namespace, r.Key)
		if existing, ok := byKey[key]; ok {
			existing.VectorScore = r.VectorScore
			if r.Value != "" {
				existing.Value = r.Value
			}
		} else {
			result := r
			byKey[key] = &result
		}
	}

	// Calculate combined scores
	var results []SearchResult
	for _, r := range byKey {
		r.Score = vectorWeight*r.VectorScore + textWeight*r.TextScore
		results = append(results, *r)
	}

	// Sort by combined score descending
	sort.Slice(results, func(i, j int) bool {
		return results[i].Score > results[j].Score
	})

	return results
}

// buildFTSQuery creates an FTS5 query from natural language
func buildFTSQuery(raw string) string {
	// Extract tokens
	tokens := strings.Fields(raw)
	if len(tokens) == 0 {
		return ""
	}

	// Build AND query with quoted tokens
	var quoted []string
	for _, t := range tokens {
		cleaned := strings.TrimFunc(t, func(r rune) bool {
			return !((r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') || r == '_')
		})
		if cleaned != "" {
			quoted = append(quoted, `"`+cleaned+`"`)
		}
	}

	if len(quoted) == 0 {
		return ""
	}

	return strings.Join(quoted, " AND ")
}

// bm25RankToScore converts BM25 rank to a 0-1 score
func bm25RankToScore(rank float64) float64 {
	// BM25 ranks are negative, with lower (more negative) being better
	// Convert to 0-1 scale where 1 is best
	if rank >= 0 {
		return 1 / (1 + rank)
	}
	return 1 / (1 - rank)
}
