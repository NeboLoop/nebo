package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"nebo/agent/embeddings"
)

// MemoryTool provides persistent fact storage across sessions
type MemoryTool struct {
	db            *sql.DB
	searcher      *embeddings.HybridSearcher
	currentUserID string // Set per-request for user-scoped operations
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
	DB       *sql.DB              // Shared database connection (required)
	Embedder *embeddings.Service  // Optional embedding service for hybrid search
}

// NewMemoryTool creates a new memory tool using the shared database connection.
// The database must already have the memories table and FTS index (via migrations).
func NewMemoryTool(cfg MemoryConfig) (*MemoryTool, error) {
	if cfg.DB == nil {
		return nil, fmt.Errorf("database connection required")
	}

	tool := &MemoryTool{db: cfg.DB}

	// Set up hybrid search if embeddings are available
	if cfg.Embedder != nil {
		tool.searcher = embeddings.NewHybridSearcher(embeddings.HybridSearchConfig{
			DB:       cfg.DB,
			Embedder: cfg.Embedder,
		})
	}

	return tool, nil
}

// SetEmbedder configures the embedding service for hybrid search
func (t *MemoryTool) SetEmbedder(embedder *embeddings.Service) {
	if embedder != nil {
		t.searcher = embeddings.NewHybridSearcher(embeddings.HybridSearchConfig{
			DB:       t.db,
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

// GetCurrentUser returns the current user ID
func (t *MemoryTool) GetCurrentUser() string {
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

	if params.Namespace == "" {
		params.Namespace = "default"
	}

	// Apply layer prefix to namespace if specified
	if params.Layer != "" {
		params.Namespace = params.Layer + "/" + params.Namespace
	}

	var result string
	var err error

	switch params.Action {
	case "store":
		result, err = t.store(params)
	case "recall":
		result, err = t.recall(params)
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
	if params.Key == "" {
		return "", fmt.Errorf("key is required for store action")
	}
	if params.Value == "" {
		return "", fmt.Errorf("value is required for store action")
	}

	tagsJSON, _ := json.Marshal(params.Tags)
	metadataJSON, _ := json.Marshal(params.Metadata)

	// Use current user ID for user-scoped memories
	userID := t.currentUserID

	// Upsert into memories table (user-scoped)
	query := `
		INSERT INTO memories (namespace, key, value, tags, metadata, user_id, updated_at)
		VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
		ON CONFLICT(namespace, key, user_id) DO UPDATE SET
			value = excluded.value,
			tags = excluded.tags,
			metadata = excluded.metadata,
			updated_at = CURRENT_TIMESTAMP
	`
	_, err := t.db.Exec(query, params.Namespace, params.Key, params.Value, string(tagsJSON), string(metadataJSON), userID)
	if err != nil {
		return "", err
	}

	// Sync tacit.user memories to user_profiles table for onboarding
	if params.Namespace == "tacit/user" || params.Namespace == "tacit.user" {
		t.syncToUserProfile(params.Key, params.Value, userID)
	}

	return fmt.Sprintf("Stored memory: %s (namespace: %s, user: %s)", params.Key, params.Namespace, userID), nil
}

// syncToUserProfile updates the user_profiles table when tacit.user.* memories are stored.
// This bridges the memory system with structured user profiles for onboarding.
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
		err := t.db.QueryRow(`SELECT id FROM users LIMIT 1`).Scan(&userID)
		if err != nil {
			return // No user found
		}
	}

	now := time.Now().Unix()

	// Upsert user_profiles
	query := fmt.Sprintf(`
		INSERT INTO user_profiles (user_id, %s, created_at, updated_at)
		VALUES (?, ?, ?, ?)
		ON CONFLICT(user_id) DO UPDATE SET
			%s = excluded.%s,
			updated_at = excluded.updated_at
	`, column, column, column)

	t.db.Exec(query, userID, value, now, now)

	// Check if we should mark onboarding complete (name is required minimum)
	if key == "name" || key == "display_name" {
		t.db.Exec(`
			UPDATE user_profiles
			SET onboarding_completed = 1, updated_at = ?
			WHERE user_id = ? AND display_name IS NOT NULL AND display_name != ''
		`, now, userID)
	}
}

func (t *MemoryTool) recall(params memoryInput) (string, error) {
	if params.Key == "" {
		return "", fmt.Errorf("key is required for recall action")
	}

	var value, tags, metadata string
	var createdAt, updatedAt, accessedAt time.Time
	var accessCount int

	// Use current user ID for user-scoped queries
	userID := t.currentUserID

	query := `
		SELECT value, tags, metadata, created_at, updated_at, accessed_at, access_count
		FROM memories
		WHERE namespace = ? AND key = ? AND user_id = ?
	`
	err := t.db.QueryRow(query, params.Namespace, params.Key, userID).Scan(
		&value, &tags, &metadata, &createdAt, &updatedAt, &accessedAt, &accessCount,
	)
	if err == sql.ErrNoRows {
		return fmt.Sprintf("No memory found with key '%s' in namespace '%s'", params.Key, params.Namespace), nil
	}
	if err != nil {
		return "", err
	}

	// Update access stats
	t.db.Exec(`
		UPDATE memories SET accessed_at = CURRENT_TIMESTAMP, access_count = access_count + 1
		WHERE namespace = ? AND key = ? AND user_id = ?
	`, params.Namespace, params.Key, userID)

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Key: %s\n", params.Key))
	result.WriteString(fmt.Sprintf("Value: %s\n", value))
	if tags != "" && tags != "null" {
		result.WriteString(fmt.Sprintf("Tags: %s\n", tags))
	}
	if metadata != "" && metadata != "null" && metadata != "{}" {
		result.WriteString(fmt.Sprintf("Metadata: %s\n", metadata))
	}
	result.WriteString(fmt.Sprintf("Created: %s\n", createdAt.Format(time.RFC3339)))
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
	userID := t.currentUserID

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
		// Fall through to FTS search if hybrid fails
	}

	// FTS search (user-scoped)
	query := `
		SELECT m.key, m.value, m.tags
		FROM memories m
		JOIN memories_fts f ON m.id = f.rowid
		WHERE memories_fts MATCH ? AND m.namespace = ? AND m.user_id = ?
		ORDER BY rank
		LIMIT 10
	`
	rows, err := t.db.Query(query, params.Query, params.Namespace, userID)
	if err != nil {
		// Try simple LIKE search as fallback (user-scoped)
		query = `
			SELECT key, value, tags
			FROM memories
			WHERE namespace = ? AND user_id = ? AND (key LIKE ? OR value LIKE ?)
			LIMIT 10
		`
		likePattern := "%" + params.Query + "%"
		rows, err = t.db.Query(query, params.Namespace, userID, likePattern, likePattern)
		if err != nil {
			return "", err
		}
	}
	defer rows.Close()

	var results []string
	for rows.Next() {
		var key, value, tags string
		if err := rows.Scan(&key, &value, &tags); err != nil {
			continue
		}
		// Truncate value if too long
		if len(value) > 200 {
			value = value[:200] + "..."
		}
		results = append(results, fmt.Sprintf("- %s: %s", key, value))
	}

	if len(results) == 0 {
		return fmt.Sprintf("No memories found matching '%s' in namespace '%s'", params.Query, params.Namespace), nil
	}

	return fmt.Sprintf("Found %d memories:\n%s", len(results), strings.Join(results, "\n")), nil
}

func (t *MemoryTool) list(params memoryInput) (string, error) {
	// Use current user ID for user-scoped queries
	userID := t.currentUserID

	query := `
		SELECT key, substr(value, 1, 100) as preview, tags, access_count
		FROM memories
		WHERE namespace = ? AND user_id = ?
		ORDER BY access_count DESC, updated_at DESC
		LIMIT 50
	`
	rows, err := t.db.Query(query, params.Namespace, userID)
	if err != nil {
		return "", err
	}
	defer rows.Close()

	var results []string
	for rows.Next() {
		var key, preview, tags string
		var accessCount int
		if err := rows.Scan(&key, &preview, &tags, &accessCount); err != nil {
			continue
		}
		if len(preview) > 80 {
			preview = preview[:80] + "..."
		}
		results = append(results, fmt.Sprintf("- %s: %s (accessed %d times)", key, preview, accessCount))
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
	userID := t.currentUserID

	result, err := t.db.Exec(`DELETE FROM memories WHERE namespace = ? AND key = ? AND user_id = ?`, params.Namespace, params.Key, userID)
	if err != nil {
		return "", err
	}

	rows, _ := result.RowsAffected()
	if rows == 0 {
		return fmt.Sprintf("No memory found with key '%s' in namespace '%s'", params.Key, params.Namespace), nil
	}

	return fmt.Sprintf("Deleted memory: %s (namespace: %s)", params.Key, params.Namespace), nil
}

func (t *MemoryTool) clear(params memoryInput) (string, error) {
	// Use current user ID for user-scoped operations
	userID := t.currentUserID

	result, err := t.db.Exec(`DELETE FROM memories WHERE namespace = ? AND user_id = ?`, params.Namespace, userID)
	if err != nil {
		return "", err
	}

	rows, _ := result.RowsAffected()
	return fmt.Sprintf("Cleared %d memories from namespace '%s'", rows, params.Namespace), nil
}

// StoreEntry stores a memory entry directly (for programmatic use, e.g., auto-extraction)
// Uses the current user ID for user-scoped storage
func (t *MemoryTool) StoreEntry(layer, namespace, key, value string, tags []string) error {
	if key == "" || value == "" {
		return fmt.Errorf("key and value are required")
	}

	// Apply layer prefix to namespace
	fullNamespace := namespace
	if layer != "" {
		fullNamespace = layer + "/" + namespace
	}
	if fullNamespace == "" {
		fullNamespace = "default"
	}

	// Use current user ID for user-scoped storage
	userID := t.currentUserID

	tagsJSON, _ := json.Marshal(tags)

	query := `
		INSERT INTO memories (namespace, key, value, tags, user_id, updated_at)
		VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
		ON CONFLICT(namespace, key, user_id) DO UPDATE SET
			value = excluded.value,
			tags = excluded.tags,
			updated_at = CURRENT_TIMESTAMP
	`
	_, err := t.db.Exec(query, fullNamespace, key, value, string(tagsJSON), userID)
	return err
}
