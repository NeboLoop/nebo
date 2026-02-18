package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"time"
	"unicode"

	"github.com/neboloop/nebo/internal/agent/embeddings"
	"github.com/neboloop/nebo/internal/db"
)

// Memory content limits
const (
	MaxMemoryKeyLength   = 128
	MaxMemoryValueLength = 2048
)

// instructionPatterns matches strings that look like prompt injection attempts.
// These are checked case-insensitively against memory content.
var instructionPatterns = regexp.MustCompile(`(?i)` +
	`(ignore\s+(all\s+)?previous\s+instructions)` +
	`|(ignore\s+(all\s+)?above)` +
	`|(disregard\s+(all\s+)?previous)` +
	`|(you\s+are\s+now\s+)` +
	`|(new\s+instructions?\s*:)` +
	`|(system\s*:\s)` +
	`|(<\s*system\s*>)` +
	`|(<\s*/?\s*system-?(prompt|message|instruction)\s*>)` +
	`|(IMPORTANT\s*:\s*you\s+must)` +
	`|(override\s+(all\s+)?previous)` +
	`|(forget\s+(all\s+)?previous)` +
	`|(act\s+as\s+(if|though)\s+you)` +
	`|(pretend\s+you\s+are)` +
	`|(from\s+now\s+on\s*,?\s*you)`,
)

// sanitizeMemoryKey validates and cleans a memory key.
// Returns the sanitized key and an error if the key is invalid.
func sanitizeMemoryKey(key string) (string, error) {
	key = strings.TrimSpace(key)
	if key == "" {
		return "", fmt.Errorf("key is required")
	}
	key = stripControlChars(key)
	if len(key) > MaxMemoryKeyLength {
		key = key[:MaxMemoryKeyLength]
	}
	return key, nil
}

// sanitizeMemoryValue validates and cleans a memory value.
// Returns the sanitized value and an error if the value is invalid.
func sanitizeMemoryValue(value string) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return "", fmt.Errorf("value is required")
	}
	value = stripControlChars(value)
	if len(value) > MaxMemoryValueLength {
		value = value[:MaxMemoryValueLength]
	}
	// Block instruction-like content
	if instructionPatterns.MatchString(value) {
		return "", fmt.Errorf("value contains instruction-like content that cannot be stored in memory")
	}
	return value, nil
}

// stripControlChars removes control characters except newlines and tabs.
func stripControlChars(s string) string {
	return strings.Map(func(r rune) rune {
		if r == '\n' || r == '\t' || r == '\r' {
			return r
		}
		if unicode.IsControl(r) {
			return -1 // Drop
		}
		return r
	}, s)
}

// MemoryTool provides persistent fact storage across sessions
type MemoryTool struct {
	sqlDB         *sql.DB                // Raw DB connection (for embeddings, FTS)
	queries       *db.Queries            // sqlc queries for memory operations
	embedder      *embeddings.Service    // Embedding service for vector storage
	searcher      *embeddings.HybridSearcher
	currentUserID string                 // Set per-request for user-scoped operations
	sanitize      bool                   // Enable injection-pattern filtering on content
}

type memoryInput struct {
	Action    string            `json:"action"`    // store, recall, search, list, delete, clear
	Key       string            `json:"key"`       // Fact key/identifier
	Value     string            `json:"value"`     // Fact content (for store)
	Tags      []string          `json:"tags"`      // Tags for categorization
	Query     string            `json:"query"`     // Search query (for search action)
	Namespace string            `json:"namespace"` // Namespace for organization (default: "default")
	Layer     string            `json:"layer"`     // Memory layer: tacit, daily, entity (optional, prepended to namespace)
	Metadata  map[string]string `json:"metadata"`  // Additional metadata
}

// Memory layers for three-tier memory system
const (
	LayerTacit  = "tacit"  // Long-term preferences, learned behaviors
	LayerDaily  = "daily"  // Day-specific facts (keyed by date)
	LayerEntity = "entity" // People, places, things
)

// MemoryConfig configures the memory tool
type MemoryConfig struct {
	DB              *sql.DB             // Shared database connection (required)
	Embedder        *embeddings.Service // Optional embedding service for hybrid search
	SanitizeContent bool                // Enable injection-pattern filtering on stored content
}

// NewMemoryTool creates a new memory tool using the shared database connection.
// The database must already have the memories table and FTS index (via migrations).
// Uses sqlc queries for all memory operations per architectural requirements.
func NewMemoryTool(cfg MemoryConfig) (*MemoryTool, error) {
	if cfg.DB == nil {
		return nil, fmt.Errorf("database connection required")
	}

	tool := &MemoryTool{
		sqlDB:    cfg.DB,
		queries:  db.New(cfg.DB), // Create sqlc queries from DB connection
		sanitize: cfg.SanitizeContent,
	}

	// Set up embedding service and hybrid search if available
	if cfg.Embedder != nil {
		tool.embedder = cfg.Embedder
		tool.searcher = embeddings.NewHybridSearcher(embeddings.HybridSearchConfig{
			DB:       cfg.DB,
			Embedder: cfg.Embedder,
		})
	}

	return tool, nil
}

// SetEmbedder configures the embedding service for hybrid search and on-write embedding
func (t *MemoryTool) SetEmbedder(embedder *embeddings.Service) {
	t.embedder = embedder
	if embedder != nil {
		t.searcher = embeddings.NewHybridSearcher(embeddings.HybridSearchConfig{
			DB:       t.sqlDB,
			Embedder: embedder,
		})
	} else {
		t.searcher = nil
	}
}

// Close is a no-op since the database is shared and managed elsewhere
func (t *MemoryTool) Close() error {
	return nil
}

// SetCurrentUser sets the user ID for user-scoped memory operations
// This should be called before each request to ensure proper isolation
func (t *MemoryTool) SetCurrentUser(userID string) {
	t.currentUserID = userID
}

// GetCurrentUser returns the current user ID, defaulting to "anonymous" if unset.
// This ensures MCP and other paths that don't call SetCurrentUser still work correctly.
func (t *MemoryTool) GetCurrentUser() string {
	if t.currentUserID == "" {
		return "anonymous"
	}
	return t.currentUserID
}

func (t *MemoryTool) Name() string {
	return "memory"
}


func (t *MemoryTool) Description() string {
	return `Store and recall facts persistently across sessions using a three-layer memory system:
- tacit: Long-term preferences and learned behaviors (e.g., code style, favorite tools)
- daily: Day-specific facts (e.g., today's standup notes, meeting decisions)
- entity: Information about people, places, and things (e.g., person/sarah, project/nebo)
Use for remembering user preferences, project context, learned information, and important notes.`
}

func (t *MemoryTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["store", "recall", "search", "list", "delete", "clear"],
				"description": "Memory action: store (save fact), recall (get by key), search (full-text search), list (list keys), delete (remove fact), clear (remove all in namespace)"
			},
			"key": {
				"type": "string",
				"description": "Unique identifier for the fact (required for store, recall, delete). Use path-like keys for organization (e.g., 'preferences/code_style', 'person/sarah')"
			},
			"value": {
				"type": "string",
				"description": "The fact content to store (required for store action)"
			},
			"tags": {
				"type": "array",
				"items": {"type": "string"},
				"description": "Tags for categorization (e.g., ['preference', 'user'])"
			},
			"query": {
				"type": "string",
				"description": "Search query for full-text search (required for search action)"
			},
			"layer": {
				"type": "string",
				"enum": ["tacit", "daily", "entity"],
				"description": "Memory layer for three-tier organization. tacit=long-term preferences, daily=day-specific facts, entity=people/places/things. Gets prepended to namespace."
			},
			"namespace": {
				"type": "string",
				"description": "Namespace for organization (default: 'default'). Use different namespaces for different projects/contexts.",
				"default": "default"
			},
			"metadata": {
				"type": "object",
				"additionalProperties": {"type": "string"},
				"description": "Additional metadata as key-value pairs"
			}
		},
		"required": ["action"]
	}`)
}

func (t *MemoryTool) RequiresApproval() bool {
	return false
}

func (t *MemoryTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params memoryInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to parse input: %v", err),
			IsError: true,
		}, nil
	}

	// Build effective namespace:
	// - No layer, no namespace → "" (search all)
	// - Layer only → "tacit" (search all in layer via prefix match)
	// - Namespace only → "default" (backwards compat)
	// - Both → "tacit/user" (exact prefix)
	if params.Layer != "" {
		if params.Namespace == "" {
			params.Namespace = params.Layer // e.g., "tacit" matches "tacit/user", "tacit/default", etc.
		} else {
			params.Namespace = params.Layer + "/" + params.Namespace
		}
	}
	// Only store needs a concrete namespace — default to "default" for writes
	if params.Namespace == "" && params.Action == "store" {
		params.Namespace = "default"
	}
	// For recall/delete/list/search/clear with no namespace, use "" to search all

	var result string
	var err error

	switch params.Action {
	case "store":
		result, err = t.store(params)
	case "recall":
		result, err = t.recall(ctx, params)
	case "search":
		result, err = t.searchWithContext(ctx, params)
	case "list":
		result, err = t.list(params)
	case "delete":
		result, err = t.delete(params)
	case "clear":
		result, err = t.clear(params)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s", params.Action),
			IsError: true,
		}, nil
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Memory action failed: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: result,
		IsError: false,
	}, nil
}

func (t *MemoryTool) store(params memoryInput) (string, error) {
	// Sanitize key and value before storing (when enabled)
	if t.sanitize {
		key, err := sanitizeMemoryKey(params.Key)
		if err != nil {
			return "", fmt.Errorf("key is required for store action")
		}
		params.Key = key

		value, valErr := sanitizeMemoryValue(params.Value)
		if valErr != nil {
			return "", valErr
		}
		params.Value = value
	}

	tagsJSON, _ := json.Marshal(params.Tags)
	metadataJSON, _ := json.Marshal(params.Metadata)

	// Use current user ID for user-scoped memories
	userID := t.GetCurrentUser()

	// Upsert into memories table using sqlc (user-scoped)
	upsertErr := t.queries.UpsertMemory(context.Background(), db.UpsertMemoryParams{
		Namespace: params.Namespace,
		Key:       params.Key,
		Value:     params.Value,
		Tags:      sql.NullString{String: string(tagsJSON), Valid: len(tagsJSON) > 0},
		Metadata:  sql.NullString{String: string(metadataJSON), Valid: len(metadataJSON) > 0},
		UserID:    userID,
	})
	if upsertErr != nil {
		return "", upsertErr
	}

	// Generate vector embedding for this memory (async-safe, non-blocking on failure)
	t.embedMemory(params.Namespace, params.Key, params.Value, userID)

	// Sync user-related memories to user_profiles table.
	// Handles multiple naming conventions used by skills and auto-extraction:
	//   - namespace="tacit/user", key="name"       (onboarding skill)
	//   - namespace="tacit", key="user/name"        (auto-extraction)
	//   - namespace="tacit.user", key="name"        (dot notation)
	if params.Namespace == "tacit/user" || params.Namespace == "tacit.user" {
		t.syncToUserProfile(params.Key, params.Value, userID)
	} else if params.Namespace == "tacit" && strings.HasPrefix(params.Key, "user/") {
		profileKey := strings.TrimPrefix(params.Key, "user/")
		t.syncToUserProfile(profileKey, params.Value, userID)
	}

	return fmt.Sprintf("Stored memory: %s (namespace: %s, user: %s)", params.Key, params.Namespace, userID), nil
}

// embedMemory creates a chunk and vector embedding for a memory.
// This is the write-path that populates memory_chunks + memory_embeddings tables.
// Non-fatal: logs errors but never fails the store operation.
// Runs asynchronously so it doesn't block the store path — embedding API calls
// with retries can take 10-30s per fact, which would serialize and block memory flushes.
func (t *MemoryTool) embedMemory(namespace, key, value, userID string) {
	if t.embedder == nil || !t.embedder.HasProvider() {
		return // No embedding provider configured, skip silently
	}

	go t.embedMemorySync(namespace, key, value, userID)
}

// embedMemorySync is the synchronous implementation called from the goroutine.
func (t *MemoryTool) embedMemorySync(namespace, key, value, userID string) {
	defer func() {
		if v := recover(); v != nil {
			fmt.Printf("[Memory] Panic in embedMemory: %v\n", v)
		}
	}()

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Get the memory ID (just upserted, so it must exist)
	mem, err := t.queries.GetMemoryByKeyAndUser(ctx, db.GetMemoryByKeyAndUserParams{
		Namespace: namespace,
		Key:       key,
		UserID:    userID,
	})
	if err != nil {
		fmt.Printf("[Memory] Failed to get memory ID for embedding (ns=%s key=%s): %v\n", namespace, key, err)
		return
	}

	memoryID := sql.NullInt64{Int64: mem.ID, Valid: true}

	// Delete old chunks (cascade deletes embeddings too)
	if err := t.queries.DeleteMemoryChunks(ctx, memoryID); err != nil {
		fmt.Printf("[Memory] Failed to delete old chunks for memory %d: %v\n", mem.ID, err)
		// Continue anyway — we'll create new ones
	}

	// Build chunk text: "key: value" gives better semantic context for short memories
	fullText := key + ": " + value
	model := t.embedder.Model()

	// Split into overlapping chunks
	chunks := embeddings.SplitText(fullText)

	// Collect texts for batch embedding
	texts := make([]string, len(chunks))
	for i, c := range chunks {
		texts[i] = c.Text
	}

	// Batch embed all chunks
	vectors, err := t.embedder.Embed(ctx, texts)
	if err != nil {
		fmt.Printf("[Memory] Failed to generate embeddings for memory %d: %v\n", mem.ID, err)
		return
	}

	// Create chunk + embedding rows
	for i, c := range chunks {
		if i >= len(vectors) {
			break
		}

		chunk, err := t.queries.CreateMemoryChunk(ctx, db.CreateMemoryChunkParams{
			MemoryID:   memoryID,
			ChunkIndex: int64(c.Index),
			Text:       c.Text,
			Source:     sql.NullString{String: "memory", Valid: true},
			Path:       sql.NullString{},
			StartChar:  sql.NullInt64{Int64: int64(c.StartChar), Valid: true},
			EndChar:    sql.NullInt64{Int64: int64(c.EndChar), Valid: true},
			Model:      sql.NullString{String: model, Valid: true},
			UserID:     userID,
		})
		if err != nil {
			fmt.Printf("[Memory] Failed to create chunk %d for memory %d: %v\n", i, mem.ID, err)
			continue
		}

		blob, _ := json.Marshal(vectors[i])
		_, err = t.queries.CreateMemoryEmbedding(ctx, db.CreateMemoryEmbeddingParams{
			ChunkID:    sql.NullInt64{Int64: chunk.ID, Valid: true},
			Model:      model,
			Dimensions: int64(len(vectors[i])),
			Embedding:  blob,
		})
		if err != nil {
			fmt.Printf("[Memory] Failed to store embedding for memory %d chunk %d: %v\n", mem.ID, chunk.ID, err)
		}
	}
}

// MigrateEmbeddings detects stale embeddings from a previous model and clears them
// so that BackfillEmbeddings can regenerate them with the current model.
// Returns (staleCount, deletedCount, error).
func (t *MemoryTool) MigrateEmbeddings(ctx context.Context) (int64, int64, error) {
	if t.embedder == nil || !t.embedder.HasProvider() {
		return 0, 0, nil
	}

	currentModel := t.embedder.Model()
	if currentModel == "" {
		return 0, 0, nil
	}

	// Count embeddings NOT matching the current model
	// These are stale (e.g., old nomic-embed-text 768-dim vectors)
	var staleCount int64
	err := t.sqlDB.QueryRowContext(ctx, `
		SELECT COUNT(*) FROM memory_embeddings WHERE model != ?
	`, currentModel).Scan(&staleCount)
	if err != nil {
		return 0, 0, fmt.Errorf("failed to count stale embeddings: %w", err)
	}

	if staleCount == 0 {
		return 0, 0, nil
	}

	// Find distinct stale models for logging
	rows, err := t.sqlDB.QueryContext(ctx, `
		SELECT DISTINCT model, COUNT(*), dimensions
		FROM memory_embeddings
		WHERE model != ?
		GROUP BY model, dimensions
	`, currentModel)
	if err == nil {
		defer rows.Close()
		for rows.Next() {
			var model string
			var count, dims int64
			if rows.Scan(&model, &count, &dims) == nil {
				fmt.Printf("[Memory] Stale embeddings: %d vectors from model %q (%d dims) → will re-embed with %q\n",
					count, model, dims, currentModel)
			}
		}
	}

	// Delete stale embeddings (chunks remain — BackfillEmbeddings uses LEFT JOIN on chunks)
	// We also delete the orphaned chunks so backfill picks up those memories
	result, err := t.sqlDB.ExecContext(ctx, `DELETE FROM memory_embeddings WHERE model != ?`, currentModel)
	if err != nil {
		return staleCount, 0, fmt.Errorf("failed to delete stale embeddings: %w", err)
	}
	deletedEmbeddings, _ := result.RowsAffected()

	// Delete chunks that no longer have embeddings — this makes BackfillEmbeddings
	// see those memories as "without chunks" and re-process them
	result2, err := t.sqlDB.ExecContext(ctx, `
		DELETE FROM memory_chunks WHERE id NOT IN (
			SELECT DISTINCT chunk_id FROM memory_embeddings WHERE chunk_id IS NOT NULL
		)
	`)
	if err != nil {
		fmt.Printf("[Memory] Warning: failed to clean orphaned chunks: %v\n", err)
	} else {
		deletedChunks, _ := result2.RowsAffected()
		if deletedChunks > 0 {
			fmt.Printf("[Memory] Cleaned %d orphaned chunks\n", deletedChunks)
		}
	}

	// Also clear the embedding cache for the old model(s) so re-embeds are fresh
	_, err = t.sqlDB.ExecContext(ctx, `DELETE FROM embedding_cache WHERE model != ?`, currentModel)
	if err != nil {
		fmt.Printf("[Memory] Warning: failed to clean old embedding cache: %v\n", err)
	}

	fmt.Printf("[Memory] Migration complete: deleted %d stale embeddings. BackfillEmbeddings will regenerate.\n", deletedEmbeddings)
	return staleCount, deletedEmbeddings, nil
}

// BackfillEmbeddings generates embeddings for all memories that don't have them yet.
// Returns the number of memories embedded and any error.
func (t *MemoryTool) BackfillEmbeddings(ctx context.Context) (int, error) {
	if t.embedder == nil || !t.embedder.HasProvider() {
		return 0, fmt.Errorf("no embedding provider configured")
	}

	// Find all memories without chunks
	rows, err := t.sqlDB.QueryContext(ctx, `
		SELECT m.id, m.namespace, m.key, m.value, m.user_id
		FROM memories m
		LEFT JOIN memory_chunks mc ON mc.memory_id = m.id
		WHERE mc.id IS NULL
		ORDER BY m.id
	`)
	if err != nil {
		return 0, fmt.Errorf("failed to query memories without embeddings: %w", err)
	}
	defer rows.Close()

	type memRow struct {
		id        int64
		namespace string
		key       string
		value     string
		userID    string
	}

	var memories []memRow
	for rows.Next() {
		var m memRow
		if err := rows.Scan(&m.id, &m.namespace, &m.key, &m.value, &m.userID); err != nil {
			continue
		}
		memories = append(memories, m)
	}

	if len(memories) == 0 {
		return 0, nil
	}

	model := t.embedder.Model()
	embedded := 0

	// Process each memory: chunk, then batch-embed all chunks
	batchSize := 20
	for i := 0; i < len(memories); i += batchSize {
		end := i + batchSize
		if end > len(memories) {
			end = len(memories)
		}
		batch := memories[i:end]

		// Build chunks for all memories in batch
		type chunkEntry struct {
			memIdx int
			chunk  embeddings.Chunk
		}
		var allChunks []chunkEntry
		for j, m := range batch {
			fullText := m.key + ": " + m.value
			chunks := embeddings.SplitText(fullText)
			for _, c := range chunks {
				allChunks = append(allChunks, chunkEntry{memIdx: j, chunk: c})
			}
		}

		if len(allChunks) == 0 {
			continue
		}

		// Batch embed all chunk texts
		texts := make([]string, len(allChunks))
		for j, ce := range allChunks {
			texts[j] = ce.chunk.Text
		}

		embeddingVecs, err := t.embedder.Embed(ctx, texts)
		if err != nil {
			errStr := err.Error()
			// Abort entirely on auth/config errors — every subsequent batch would fail too
			if strings.Contains(errStr, "401") || strings.Contains(errStr, "Unauthorized") ||
				strings.Contains(errStr, "invalid_api_key") || strings.Contains(errStr, "403") {
				return embedded, fmt.Errorf("embedding provider auth failed, aborting backfill: %w", err)
			}
			fmt.Printf("[Memory] Backfill batch %d-%d failed: %v\n", i, end, err)
			continue
		}

		// Store chunks + embeddings
		lastMemIdx := -1
		for j, ce := range allChunks {
			if j >= len(embeddingVecs) {
				break
			}

			m := batch[ce.memIdx]
			memoryID := sql.NullInt64{Int64: m.id, Valid: true}

			chunk, err := t.queries.CreateMemoryChunk(ctx, db.CreateMemoryChunkParams{
				MemoryID:   memoryID,
				ChunkIndex: int64(ce.chunk.Index),
				Text:       ce.chunk.Text,
				Source:     sql.NullString{String: "memory", Valid: true},
				Path:       sql.NullString{},
				StartChar:  sql.NullInt64{Int64: int64(ce.chunk.StartChar), Valid: true},
				EndChar:    sql.NullInt64{Int64: int64(ce.chunk.EndChar), Valid: true},
				Model:      sql.NullString{String: model, Valid: true},
				UserID:     m.userID,
			})
			if err != nil {
				fmt.Printf("[Memory] Backfill chunk failed for memory %d: %v\n", m.id, err)
				continue
			}

			blob, _ := json.Marshal(embeddingVecs[j])
			_, err = t.queries.CreateMemoryEmbedding(ctx, db.CreateMemoryEmbeddingParams{
				ChunkID:    sql.NullInt64{Int64: chunk.ID, Valid: true},
				Model:      model,
				Dimensions: int64(len(embeddingVecs[j])),
				Embedding:  blob,
			})
			if err != nil {
				fmt.Printf("[Memory] Backfill embedding failed for memory %d: %v\n", m.id, err)
				continue
			}

			// Count each memory once (not each chunk)
			if ce.memIdx != lastMemIdx {
				embedded++
				lastMemIdx = ce.memIdx
			}
		}
	}

	return embedded, nil
}

// syncToUserProfile updates the user_profiles table when tacit.user.* memories are stored.
// This bridges the memory system with structured user profiles for onboarding.
// Uses raw SQL for dynamic column updates (sqlc doesn't support dynamic columns).
func (t *MemoryTool) syncToUserProfile(key, value, userID string) {
	// Map memory keys to user_profile columns
	columnMap := map[string]string{
		"name":                "display_name",
		"display_name":        "display_name",
		"location":            "location",
		"timezone":            "timezone",
		"occupation":          "occupation",
		"goals":               "goals",
		"context":             "context",
		"communication_style": "communication_style",
		"interests":           "interests",
	}

	column, ok := columnMap[key]
	if !ok {
		return // Unknown key, skip
	}

	// If no userID provided, try to get the first user (backwards compatibility)
	if userID == "" {
		err := t.sqlDB.QueryRow(`SELECT id FROM users LIMIT 1`).Scan(&userID)
		if err != nil {
			return // No user found
		}
	}

	now := time.Now().Unix()

	// Upsert user_profiles (raw SQL for dynamic column - unavoidable)
	query := fmt.Sprintf(`
		INSERT INTO user_profiles (user_id, %s, created_at, updated_at)
		VALUES (?, ?, ?, ?)
		ON CONFLICT(user_id) DO UPDATE SET
			%s = excluded.%s,
			updated_at = excluded.updated_at
	`, column, column, column)

	t.sqlDB.Exec(query, userID, value, now, now)

	// Check if we should mark onboarding complete (name is required minimum)
	if key == "name" || key == "display_name" {
		t.sqlDB.Exec(`
			UPDATE user_profiles
			SET onboarding_completed = 1, updated_at = ?
			WHERE user_id = ? AND display_name IS NOT NULL AND display_name != ''
		`, now, userID)
	}
}

func (t *MemoryTool) recall(ctx context.Context, params memoryInput) (string, error) {
	if params.Key == "" {
		return "", fmt.Errorf("key is required for recall action")
	}

	// Use current user ID for user-scoped queries
	userID := t.GetCurrentUser()

	var mem db.GetMemoryByKeyAndUserRow
	var err error

	if params.Namespace != "" {
		// Exact namespace match
		mem, err = t.queries.GetMemoryByKeyAndUser(context.Background(), db.GetMemoryByKeyAndUserParams{
			Namespace: params.Namespace,
			Key:       params.Key,
			UserID:    userID,
		})
	} else {
		// No namespace specified — search across all namespaces
		anyMem, anyErr := t.queries.GetMemoryByKeyAndUserAnyNamespace(context.Background(), db.GetMemoryByKeyAndUserAnyNamespaceParams{
			Key:    params.Key,
			UserID: userID,
		})
		if anyErr == nil {
			// Map to the same row type
			mem = db.GetMemoryByKeyAndUserRow{
				ID:          anyMem.ID,
				Namespace:   anyMem.Namespace,
				Key:         anyMem.Key,
				Value:       anyMem.Value,
				Tags:        anyMem.Tags,
				Metadata:    anyMem.Metadata,
				CreatedAt:   anyMem.CreatedAt,
				UpdatedAt:   anyMem.UpdatedAt,
				AccessedAt:  anyMem.AccessedAt,
				AccessCount: anyMem.AccessCount,
			}
			params.Namespace = anyMem.Namespace // use the found namespace for access stats
		}
		err = anyErr
	}

	if err == sql.ErrNoRows {
		// Fall back to search using the key as a query — the LLM may not
		// remember the exact key format, but a fuzzy search often finds it.
		searchParams := memoryInput{
			Action:    "search",
			Query:     params.Key,
			Namespace: params.Namespace,
		}
		if result, searchErr := t.searchWithContext(ctx, searchParams); searchErr == nil && result != "" {
			return result, nil
		}
		ns := params.Namespace
		if ns == "" {
			ns = "(all)"
		}
		return fmt.Sprintf("No memory found with key '%s' in namespace '%s'", params.Key, ns), nil
	}
	if err != nil {
		return "", err
	}

	// Update access stats using sqlc
	t.queries.IncrementMemoryAccessByKey(context.Background(), db.IncrementMemoryAccessByKeyParams{
		Namespace: params.Namespace,
		Key:       params.Key,
		UserID:    userID,
	})

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Key: %s\n", params.Key))
	result.WriteString(fmt.Sprintf("Value: %s\n", mem.Value))
	if mem.Tags.Valid && mem.Tags.String != "" && mem.Tags.String != "null" {
		result.WriteString(fmt.Sprintf("Tags: %s\n", mem.Tags.String))
	}
	if mem.Metadata.Valid && mem.Metadata.String != "" && mem.Metadata.String != "null" && mem.Metadata.String != "{}" {
		result.WriteString(fmt.Sprintf("Metadata: %s\n", mem.Metadata.String))
	}
	if mem.CreatedAt.Valid {
		result.WriteString(fmt.Sprintf("Created: %s\n", mem.CreatedAt.Time.Format(time.RFC3339)))
	}
	accessCount := int64(0)
	if mem.AccessCount.Valid {
		accessCount = mem.AccessCount.Int64
	}
	result.WriteString(fmt.Sprintf("Accessed: %d times", accessCount+1))

	return result.String(), nil
}

func (t *MemoryTool) search(params memoryInput) (string, error) {
	return t.searchWithContext(context.Background(), params)
}

func (t *MemoryTool) searchWithContext(ctx context.Context, params memoryInput) (string, error) {
	if params.Query == "" {
		return "", fmt.Errorf("query is required for search action")
	}

	// Use current user ID for user-scoped queries
	userID := t.GetCurrentUser()

	// Use hybrid search if available
	if t.searcher != nil {
		results, err := t.searcher.Search(ctx, params.Query, embeddings.SearchOptions{
			Namespace: params.Namespace,
			Limit:     10,
			UserID:    userID,
		})
		if err == nil && len(results) > 0 {
			var formatted []string
			for _, r := range results {
				value := r.Value
				if len(value) > 200 {
					value = value[:200] + "..."
				}
				formatted = append(formatted, fmt.Sprintf("- %s: %s (score: %.2f)", r.Key, value, r.Score))
			}
			return fmt.Sprintf("Found %d memories (hybrid search):\n%s", len(formatted), strings.Join(formatted, "\n")), nil
		}
		// Fall through to sqlc search if hybrid fails
	}

	// Use sqlc for LIKE-based search (FTS is too complex for sqlc)
	var memResults []db.SearchMemoriesByUserRow
	var err error

	if params.Namespace != "" && params.Namespace != "default" {
		// Search within specific namespace
		memResults2, err2 := t.queries.SearchMemoriesByUserAndNamespace(ctx, db.SearchMemoriesByUserAndNamespaceParams{
			UserID:          userID,
			NamespacePrefix: sql.NullString{String: params.Namespace, Valid: true},
			Query:           sql.NullString{String: params.Query, Valid: true},
			Limit:           10,
			Offset:          0,
		})
		if err2 != nil {
			err = err2
		} else {
			// Convert to SearchMemoriesByUserRow format
			for _, m := range memResults2 {
				memResults = append(memResults, db.SearchMemoriesByUserRow{
					ID:          m.ID,
					Namespace:   m.Namespace,
					Key:         m.Key,
					Value:       m.Value,
					Tags:        m.Tags,
					Metadata:    m.Metadata,
					CreatedAt:   m.CreatedAt,
					UpdatedAt:   m.UpdatedAt,
					AccessedAt:  m.AccessedAt,
					AccessCount: m.AccessCount,
				})
			}
		}
	} else {
		// Search across all namespaces
		memResults, err = t.queries.SearchMemoriesByUser(ctx, db.SearchMemoriesByUserParams{
			UserID: userID,
			Query:  sql.NullString{String: params.Query, Valid: true},
			Limit:  10,
			Offset: 0,
		})
	}

	if err != nil {
		return "", err
	}

	var results []string
	for _, m := range memResults {
		value := m.Value
		if len(value) > 200 {
			value = value[:200] + "..."
		}
		results = append(results, fmt.Sprintf("- %s: %s", m.Key, value))
	}

	if len(results) == 0 {
		return fmt.Sprintf("No memories found matching '%s' in namespace '%s'", params.Query, params.Namespace), nil
	}

	return fmt.Sprintf("Found %d memories:\n%s", len(results), strings.Join(results, "\n")), nil
}

func (t *MemoryTool) list(params memoryInput) (string, error) {
	// Use current user ID for user-scoped queries
	userID := t.GetCurrentUser()

	// Use sqlc for listing
	mems, err := t.queries.ListMemoriesByUserAndNamespace(context.Background(), db.ListMemoriesByUserAndNamespaceParams{
		UserID:          userID,
		NamespacePrefix: sql.NullString{String: params.Namespace, Valid: true},
		Limit:           50,
		Offset:          0,
	})
	if err != nil {
		return "", err
	}

	var results []string
	for _, m := range mems {
		preview := m.Value
		if len(preview) > 80 {
			preview = preview[:80] + "..."
		}
		accessCount := int64(0)
		if m.AccessCount.Valid {
			accessCount = m.AccessCount.Int64
		}
		results = append(results, fmt.Sprintf("- %s: %s (accessed %d times)", m.Key, preview, accessCount))
	}

	if len(results) == 0 {
		return fmt.Sprintf("No memories in namespace '%s'", params.Namespace), nil
	}

	return fmt.Sprintf("Memories in namespace '%s' (%d items):\n%s", params.Namespace, len(results), strings.Join(results, "\n")), nil
}

func (t *MemoryTool) delete(params memoryInput) (string, error) {
	if params.Key == "" {
		return "", fmt.Errorf("key is required for delete action")
	}

	// Use current user ID for user-scoped operations
	userID := t.GetCurrentUser()

	var rows int64

	if params.Namespace != "" {
		// Delete from specific namespace
		result, err := t.queries.DeleteMemoryByKeyAndUser(context.Background(), db.DeleteMemoryByKeyAndUserParams{
			Namespace: params.Namespace,
			Key:       params.Key,
			UserID:    userID,
		})
		if err != nil {
			return "", err
		}
		rows, _ = result.RowsAffected()
	} else {
		// No namespace specified — delete across all namespaces
		result, err := t.queries.DeleteMemoryByKeyAndUserAnyNamespace(context.Background(), db.DeleteMemoryByKeyAndUserAnyNamespaceParams{
			Key:    params.Key,
			UserID: userID,
		})
		if err != nil {
			return "", err
		}
		rows, _ = result.RowsAffected()
	}

	if rows == 0 {
		ns := params.Namespace
		if ns == "" {
			ns = "(all)"
		}
		return fmt.Sprintf("No memory found with key '%s' in namespace '%s'", params.Key, ns), nil
	}

	ns := params.Namespace
	if ns == "" {
		ns = "(all)"
	}
	return fmt.Sprintf("Deleted memory: %s (namespace: %s)", params.Key, ns), nil
}

func (t *MemoryTool) clear(params memoryInput) (string, error) {
	// Use current user ID for user-scoped operations
	userID := t.GetCurrentUser()

	// Use sqlc for clearing (namespace prefix match)
	result, err := t.queries.DeleteMemoriesByNamespaceAndUser(context.Background(), db.DeleteMemoriesByNamespaceAndUserParams{
		NamespacePrefix: sql.NullString{String: params.Namespace, Valid: true},
		UserID:          userID,
	})
	if err != nil {
		return "", err
	}

	rows, _ := result.RowsAffected()
	return fmt.Sprintf("Cleared %d memories from namespace '%s'", rows, params.Namespace), nil
}

// StoreEntry stores a memory entry directly (for programmatic use, e.g., auto-extraction)
// Uses the current user ID for user-scoped storage
func (t *MemoryTool) StoreEntry(layer, namespace, key, value string, tags []string) error {
	return t.StoreEntryForUser(layer, namespace, key, value, tags, t.GetCurrentUser())
}

// StoreStyleEntryForUser stores a style observation with reinforcement tracking.
// If the style already exists, increments the reinforcement count in metadata
// instead of overwriting the value. This lets frequently-observed traits become stronger signals.
func (t *MemoryTool) StoreStyleEntryForUser(layer, namespace, key, value string, tags []string, userID string) error {
	// Build the full namespace the same way StoreEntryForUser does
	fullNamespace := namespace
	if layer != "" {
		fullNamespace = layer + "/" + namespace
	}
	if fullNamespace == "" {
		fullNamespace = "default"
	}

	// Check if this style observation already exists
	existing, err := t.queries.GetMemoryByKeyAndUser(context.Background(), db.GetMemoryByKeyAndUserParams{
		Namespace: fullNamespace,
		Key:       key,
		UserID:    userID,
	})

	if err == nil {
		// Style exists — reinforce it
		var meta map[string]interface{}
		if existing.Metadata.Valid && existing.Metadata.String != "" {
			json.Unmarshal([]byte(existing.Metadata.String), &meta)
		}
		if meta == nil {
			meta = map[string]interface{}{}
		}

		count, _ := meta["reinforced_count"].(float64)
		meta["reinforced_count"] = count + 1
		meta["last_reinforced"] = time.Now().Format(time.RFC3339)

		metaJSON, _ := json.Marshal(meta)
		metaStr := string(metaJSON)

		// Update metadata and bump updated_at — don't overwrite value (keep the original observation)
		return t.queries.UpdateMemory(context.Background(), db.UpdateMemoryParams{
			ID:       existing.ID,
			Metadata: sql.NullString{String: metaStr, Valid: true},
		})
	}

	// New style observation — store with initial reinforcement metadata
	meta := map[string]interface{}{
		"reinforced_count": float64(1),
		"first_observed":   time.Now().Format(time.RFC3339),
		"last_reinforced":  time.Now().Format(time.RFC3339),
	}
	metaJSON, _ := json.Marshal(meta)

	tagsJSON, _ := json.Marshal(tags)

	err = t.queries.UpsertMemory(context.Background(), db.UpsertMemoryParams{
		Namespace: fullNamespace,
		Key:       key,
		Value:     value,
		Tags:      sql.NullString{String: string(tagsJSON), Valid: len(tagsJSON) > 0},
		Metadata:  sql.NullString{String: string(metaJSON), Valid: true},
		UserID:    userID,
	})
	if err != nil {
		return err
	}

	// Generate vector embedding for this memory
	t.embedMemory(fullNamespace, key, value, userID)

	return nil
}

// IsDuplicate checks if a memory with the same namespace:key:user_id already exists
// with an identical value, OR if any memory in the same namespace already has the
// same content under a different key. This prevents the LLM extraction from creating
// duplicates like "preferences/code_style" and "preference/code-style" with identical values.
func (t *MemoryTool) IsDuplicate(layer, namespace, key, value, userID string) bool {
	if t.queries == nil {
		return false
	}
	fullNamespace := namespace
	if layer != "" {
		fullNamespace = layer + "/" + namespace
	}
	if fullNamespace == "" {
		fullNamespace = "default"
	}

	// Check 1: exact key match (existing behavior)
	existing, err := t.queries.GetMemoryByKeyAndUser(context.Background(), db.GetMemoryByKeyAndUserParams{
		Namespace: fullNamespace,
		Key:       key,
		UserID:    userID,
	})
	if err == nil && existing.Value == value {
		return true
	}

	// Check 2: same content under any key in the same namespace
	// This catches LLM extraction generating different keys for identical facts
	var count int64
	err = t.sqlDB.QueryRow(`
		SELECT COUNT(*) FROM memories
		WHERE namespace = ? AND user_id = ? AND value = ?
	`, fullNamespace, userID, value).Scan(&count)
	if err == nil && count > 0 {
		return true
	}

	return false
}

// IndexSessionTranscript creates searchable chunks from session messages
// that haven't been embedded yet. Called after compaction to make compacted
// messages discoverable via semantic search.
func (t *MemoryTool) IndexSessionTranscript(ctx context.Context, sessionID, userID string) (int, error) {
	if t.embedder == nil || !t.embedder.HasProvider() {
		return 0, nil
	}

	// Get the high-water mark for this session
	lastID, err := t.queries.GetSessionLastEmbeddedMessageID(ctx, sessionID)
	if err != nil {
		return 0, fmt.Errorf("failed to get last embedded message ID: %w", err)
	}

	// Fetch new messages since last embedding
	msgs, err := t.queries.GetMessagesAfterID(ctx, db.GetMessagesAfterIDParams{
		SessionID: sessionID,
		ID:        lastID,
	})
	if err != nil {
		return 0, fmt.Errorf("failed to get messages: %w", err)
	}
	if len(msgs) == 0 {
		return 0, nil
	}

	// Group messages into blocks (~5 messages per block with role prefixes)
	const blockSize = 5
	model := t.embedder.Model()
	chunksCreated := 0
	var maxMsgID int64

	for i := 0; i < len(msgs); i += blockSize {
		end := i + blockSize
		if end > len(msgs) {
			end = len(msgs)
		}
		block := msgs[i:end]

		// Build block text with role prefixes
		var buf strings.Builder
		for _, m := range block {
			content := ""
			if m.Content.Valid {
				content = m.Content.String
			}
			buf.WriteString(m.Role)
			buf.WriteString(": ")
			buf.WriteString(content)
			buf.WriteString("\n\n")
			if m.ID > maxMsgID {
				maxMsgID = m.ID
			}
		}
		blockText := strings.TrimSpace(buf.String())
		if blockText == "" {
			continue
		}

		// Chunk the block
		chunks := embeddings.SplitText(blockText)

		// Batch embed
		texts := make([]string, len(chunks))
		for j, c := range chunks {
			texts[j] = c.Text
		}

		vectors, embErr := t.embedder.Embed(ctx, texts)
		if embErr != nil {
			errStr := embErr.Error()
			if strings.Contains(errStr, "401") || strings.Contains(errStr, "Unauthorized") ||
				strings.Contains(errStr, "invalid_api_key") || strings.Contains(errStr, "403") {
				return chunksCreated, fmt.Errorf("embedding provider auth failed: %w", embErr)
			}
			fmt.Printf("[Memory] Session indexing embed failed for session %s: %v\n", sessionID, embErr)
			continue
		}

		for j, c := range chunks {
			if j >= len(vectors) {
				break
			}

			chunk, chunkErr := t.queries.CreateMemoryChunk(ctx, db.CreateMemoryChunkParams{
				MemoryID:   sql.NullInt64{}, // NULL — session chunk, not tied to a memory
				ChunkIndex: int64(c.Index),
				Text:       c.Text,
				Source:     sql.NullString{String: "session", Valid: true},
				Path:       sql.NullString{String: sessionID, Valid: true},
				StartChar:  sql.NullInt64{Int64: int64(c.StartChar), Valid: true},
				EndChar:    sql.NullInt64{Int64: int64(c.EndChar), Valid: true},
				Model:      sql.NullString{String: model, Valid: true},
				UserID:     userID,
			})
			if chunkErr != nil {
				fmt.Printf("[Memory] Session chunk create failed: %v\n", chunkErr)
				continue
			}

			blob, _ := json.Marshal(vectors[j])
			_, embStoreErr := t.queries.CreateMemoryEmbedding(ctx, db.CreateMemoryEmbeddingParams{
				ChunkID:    sql.NullInt64{Int64: chunk.ID, Valid: true},
				Model:      model,
				Dimensions: int64(len(vectors[j])),
				Embedding:  blob,
			})
			if embStoreErr != nil {
				fmt.Printf("[Memory] Session embedding create failed: %v\n", embStoreErr)
				continue
			}
			chunksCreated++
		}
	}

	// Update the high-water mark
	if maxMsgID > 0 {
		_ = t.queries.UpdateSessionLastEmbeddedMessageID(ctx, db.UpdateSessionLastEmbeddedMessageIDParams{
			LastEmbeddedMessageID: sql.NullInt64{Int64: maxMsgID, Valid: true},
			ID:                    sessionID,
		})
	}

	if chunksCreated > 0 {
		fmt.Printf("[Memory] Indexed %d chunks from session %s\n", chunksCreated, sessionID)
	}

	return chunksCreated, nil
}

// StoreEntryForUser stores a memory entry for a specific user (thread-safe for background operations)
func (t *MemoryTool) StoreEntryForUser(layer, namespace, key, value string, tags []string, userID string) error {
	// Sanitize key and value (when enabled)
	if t.sanitize {
		sanitizedKey, keyErr := sanitizeMemoryKey(key)
		if keyErr != nil {
			return fmt.Errorf("key and value are required")
		}
		key = sanitizedKey
		sanitizedValue, valErr := sanitizeMemoryValue(value)
		if valErr != nil {
			return valErr
		}
		value = sanitizedValue
	}

	// Apply layer prefix to namespace
	fullNamespace := namespace
	if layer != "" {
		fullNamespace = layer + "/" + namespace
	}
	if fullNamespace == "" {
		fullNamespace = "default"
	}

	tagsJSON, _ := json.Marshal(tags)

	// Use sqlc for user-scoped storage
	err := t.queries.UpsertMemory(context.Background(), db.UpsertMemoryParams{
		Namespace: fullNamespace,
		Key:       key,
		Value:     value,
		Tags:      sql.NullString{String: string(tagsJSON), Valid: len(tagsJSON) > 0},
		Metadata:  sql.NullString{}, // No metadata for this method
		UserID:    userID,
	})
	if err != nil {
		return err
	}

	// Generate vector embedding for this memory
	t.embedMemory(fullNamespace, key, value, userID)

	// Sync user-related memories to user_profiles (same logic as store())
	if fullNamespace == "tacit/user" || fullNamespace == "tacit.user" {
		t.syncToUserProfile(key, value, userID)
	} else if fullNamespace == "tacit" && strings.HasPrefix(key, "user/") {
		t.syncToUserProfile(strings.TrimPrefix(key, "user/"), value, userID)
	}

	return nil
}
