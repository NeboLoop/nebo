package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/mcp/mcpctx"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// memoryActions defines valid actions for the memory resource.
var memoryActions = map[string][]string{
	"memory": {"store", "recall", "search", "list", "delete", "clear"},
}

// MemoryInput defines input for the memory MCP tool.
type MemoryInput struct {
	Resource string `json:"resource" jsonschema:"required,Resource type: memory"`
	Action   string `json:"action" jsonschema:"required,Action: store, recall, search, list, delete, clear"`

	// Store/recall/delete
	Key   string `json:"key,omitempty" jsonschema:"Memory key (path-like: 'user/name', 'project/nebo'). Required for store, recall, delete."`
	Value string `json:"value,omitempty" jsonschema:"Value to store. Required for store."`

	// Organization
	Layer     string `json:"layer,omitempty" jsonschema:"Memory layer: tacit (long-term), daily (day-specific), entity (people/places/things). Prepended to namespace."`
	Namespace string `json:"namespace,omitempty" jsonschema:"Namespace for organization (default: 'default')."`

	// Categorization
	Tags     []string          `json:"tags,omitempty" jsonschema:"Tags for categorization."`
	Metadata map[string]string `json:"metadata,omitempty" jsonschema:"Additional metadata as key-value pairs."`

	// Search
	Query string `json:"query,omitempty" jsonschema:"Search query. Required for search."`
}

// RegisterMemoryTool registers the memory MCP tool.
func RegisterMemoryTool(server *mcp.Server, toolCtx *mcpctx.ToolContext) {
	mcp.AddTool(server, &mcp.Tool{
		Name:  "memory",
		Title: "Memory Management",
		Description: `Persistent fact storage across sessions using a three-layer memory system.

Layers:
- tacit: Long-term preferences and learned behaviors (e.g., code style, favorite tools)
- daily: Day-specific facts (e.g., today's standup notes, meeting decisions)
- entity: Information about people, places, and things (e.g., person/sarah, project/nebo)

Actions:
- memory.store: Store a fact (requires: key, value; optional: layer, namespace, tags, metadata)
- memory.recall: Retrieve a fact by key (requires: key; optional: layer, namespace)
- memory.search: Full-text search across memories (requires: query; optional: layer, namespace)
- memory.list: List stored memories (optional: layer, namespace)
- memory.delete: Delete a memory by key (requires: key; optional: layer, namespace)
- memory.clear: Clear all memories in a namespace (optional: layer, namespace)

Examples:
  memory(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit")
  memory(resource: memory, action: recall, key: "user/name", layer: "tacit")
  memory(resource: memory, action: search, query: "preferences")
  memory(resource: memory, action: list, layer: "tacit", namespace: "user")
  memory(resource: memory, action: delete, key: "user/name", layer: "tacit")
  memory(resource: memory, action: clear, layer: "daily")`,
	}, memoryHandler(toolCtx))
}

func memoryHandler(toolCtx *mcpctx.ToolContext) func(ctx context.Context, req *mcp.CallToolRequest, input MemoryInput) (*mcp.CallToolResult, any, error) {
	return func(ctx context.Context, req *mcp.CallToolRequest, input MemoryInput) (*mcp.CallToolResult, any, error) {
		fmt.Printf("[MCP memory] Handler called - Resource: %q, Action: %q\n", input.Resource, input.Action)

		// Default resource to "memory" if not specified
		if input.Resource == "" {
			input.Resource = "memory"
		}

		// Validate resource
		validActions, ok := memoryActions[input.Resource]
		if !ok {
			return nil, nil, mcpctx.NewValidationError(
				fmt.Sprintf("invalid resource '%s', must be: memory", input.Resource),
				"resource")
		}

		// Validate action
		if !slices.Contains(validActions, input.Action) {
			return nil, nil, mcpctx.NewValidationError(
				fmt.Sprintf("invalid action '%s' for resource '%s', must be: %s",
					input.Action, input.Resource, strings.Join(validActions, ", ")),
				"action")
		}

		// Resolve namespace with layer prefix
		namespace := input.Namespace
		if namespace == "" {
			namespace = "default"
		}
		if input.Layer != "" {
			namespace = input.Layer + "/" + namespace
		}

		userID := toolCtx.UserID()

		switch input.Action {
		case "store":
			return handleMemoryStore(ctx, toolCtx, input, namespace, userID)
		case "recall":
			return handleMemoryRecall(ctx, toolCtx, input, namespace, userID)
		case "search":
			return handleMemorySearch(ctx, toolCtx, input, namespace, userID)
		case "list":
			return handleMemoryList(ctx, toolCtx, input, namespace, userID)
		case "delete":
			return handleMemoryDelete(ctx, toolCtx, input, namespace, userID)
		case "clear":
			return handleMemoryClear(ctx, toolCtx, input, namespace, userID)
		}
		return nil, nil, nil
	}
}

// ============================================================================
// MEMORY HANDLERS
// ============================================================================

// MemoryStoreOutput defines output for memory.store.
type MemoryStoreOutput struct {
	Key       string `json:"key"`
	Namespace string `json:"namespace"`
	Stored    bool   `json:"stored"`
}

func handleMemoryStore(ctx context.Context, toolCtx *mcpctx.ToolContext, input MemoryInput, namespace, userID string) (*mcp.CallToolResult, any, error) {
	if input.Key == "" {
		return nil, nil, mcpctx.NewValidationError("key is required for store action", "key")
	}
	if input.Value == "" {
		return nil, nil, mcpctx.NewValidationError("value is required for store action", "value")
	}

	tagsJSON, _ := json.Marshal(input.Tags)
	metadataJSON, _ := json.Marshal(input.Metadata)

	err := toolCtx.DB().UpsertMemory(ctx, db.UpsertMemoryParams{
		Namespace: namespace,
		Key:       input.Key,
		Value:     input.Value,
		Tags:      sql.NullString{String: string(tagsJSON), Valid: len(tagsJSON) > 0},
		Metadata:  sql.NullString{String: string(metadataJSON), Valid: len(metadataJSON) > 0},
		UserID:    userID,
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to store memory: %w", err)
	}

	return nil, MemoryStoreOutput{
		Key:       input.Key,
		Namespace: namespace,
		Stored:    true,
	}, nil
}

// MemoryRecallOutput defines output for memory.recall.
type MemoryRecallOutput struct {
	Key         string `json:"key"`
	Value       string `json:"value"`
	Namespace   string `json:"namespace"`
	Tags        string `json:"tags,omitempty"`
	Metadata    string `json:"metadata,omitempty"`
	CreatedAt   string `json:"created_at,omitempty"`
	AccessCount int64  `json:"access_count"`
}

func handleMemoryRecall(ctx context.Context, toolCtx *mcpctx.ToolContext, input MemoryInput, namespace, userID string) (*mcp.CallToolResult, any, error) {
	if input.Key == "" {
		return nil, nil, mcpctx.NewValidationError("key is required for recall action", "key")
	}

	mem, err := toolCtx.DB().GetMemoryByKeyAndUser(ctx, db.GetMemoryByKeyAndUserParams{
		Namespace: namespace,
		Key:       input.Key,
		UserID:    userID,
	})
	if err == sql.ErrNoRows {
		return nil, nil, mcpctx.NewNotFoundError(
			fmt.Sprintf("no memory found with key '%s' in namespace '%s'", input.Key, namespace))
	}
	if err != nil {
		return nil, nil, fmt.Errorf("failed to recall memory: %w", err)
	}

	// Update access stats
	toolCtx.DB().IncrementMemoryAccessByKey(ctx, db.IncrementMemoryAccessByKeyParams{
		Namespace: namespace,
		Key:       input.Key,
		UserID:    userID,
	})

	out := MemoryRecallOutput{
		Key:       mem.Key,
		Value:     mem.Value,
		Namespace: mem.Namespace,
	}
	if mem.Tags.Valid && mem.Tags.String != "null" {
		out.Tags = mem.Tags.String
	}
	if mem.Metadata.Valid && mem.Metadata.String != "null" && mem.Metadata.String != "{}" {
		out.Metadata = mem.Metadata.String
	}
	if mem.CreatedAt.Valid {
		out.CreatedAt = mem.CreatedAt.Time.Format(time.RFC3339)
	}
	if mem.AccessCount.Valid {
		out.AccessCount = mem.AccessCount.Int64 + 1
	}

	return nil, out, nil
}

// MemorySearchResult defines a single search result.
type MemorySearchResult struct {
	Key       string `json:"key"`
	Value     string `json:"value"`
	Namespace string `json:"namespace"`
}

// MemorySearchOutput defines output for memory.search.
type MemorySearchOutput struct {
	Query   string               `json:"query"`
	Count   int                  `json:"count"`
	Results []MemorySearchResult `json:"results"`
}

func handleMemorySearch(ctx context.Context, toolCtx *mcpctx.ToolContext, input MemoryInput, namespace, userID string) (*mcp.CallToolResult, any, error) {
	if input.Query == "" {
		return nil, nil, mcpctx.NewValidationError("query is required for search action", "query")
	}

	var results []MemorySearchResult

	if namespace != "" && namespace != "default" {
		// Search within specific namespace
		rows, err := toolCtx.DB().SearchMemoriesByUserAndNamespace(ctx, db.SearchMemoriesByUserAndNamespaceParams{
			UserID:          userID,
			NamespacePrefix: sql.NullString{String: namespace, Valid: true},
			Query:           sql.NullString{String: input.Query, Valid: true},
			Limit:           20,
			Offset:          0,
		})
		if err != nil {
			return nil, nil, fmt.Errorf("failed to search memories: %w", err)
		}
		for _, r := range rows {
			value := r.Value
			if len(value) > 200 {
				value = value[:200] + "..."
			}
			results = append(results, MemorySearchResult{
				Key:       r.Key,
				Value:     value,
				Namespace: r.Namespace,
			})
		}
	} else {
		// Search across all namespaces
		rows, err := toolCtx.DB().SearchMemoriesByUser(ctx, db.SearchMemoriesByUserParams{
			UserID: userID,
			Query:  sql.NullString{String: input.Query, Valid: true},
			Limit:  20,
			Offset: 0,
		})
		if err != nil {
			return nil, nil, fmt.Errorf("failed to search memories: %w", err)
		}
		for _, r := range rows {
			value := r.Value
			if len(value) > 200 {
				value = value[:200] + "..."
			}
			results = append(results, MemorySearchResult{
				Key:       r.Key,
				Value:     value,
				Namespace: r.Namespace,
			})
		}
	}

	return nil, MemorySearchOutput{
		Query:   input.Query,
		Count:   len(results),
		Results: results,
	}, nil
}

// MemoryListItem defines a single list item.
type MemoryListItem struct {
	Key         string `json:"key"`
	Value       string `json:"value"`
	Namespace   string `json:"namespace"`
	AccessCount int64  `json:"access_count"`
}

// MemoryListOutput defines output for memory.list.
type MemoryListOutput struct {
	Namespace string           `json:"namespace"`
	Count     int              `json:"count"`
	Items     []MemoryListItem `json:"items"`
}

func handleMemoryList(ctx context.Context, toolCtx *mcpctx.ToolContext, input MemoryInput, namespace, userID string) (*mcp.CallToolResult, any, error) {
	rows, err := toolCtx.DB().ListMemoriesByUserAndNamespace(ctx, db.ListMemoriesByUserAndNamespaceParams{
		UserID:          userID,
		NamespacePrefix: sql.NullString{String: namespace, Valid: true},
		Limit:           50,
		Offset:          0,
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to list memories: %w", err)
	}

	var items []MemoryListItem
	for _, r := range rows {
		value := r.Value
		if len(value) > 100 {
			value = value[:100] + "..."
		}
		accessCount := int64(0)
		if r.AccessCount.Valid {
			accessCount = r.AccessCount.Int64
		}
		items = append(items, MemoryListItem{
			Key:         r.Key,
			Value:       value,
			Namespace:   r.Namespace,
			AccessCount: accessCount,
		})
	}

	return nil, MemoryListOutput{
		Namespace: namespace,
		Count:     len(items),
		Items:     items,
	}, nil
}

// MemoryDeleteOutput defines output for memory.delete.
type MemoryDeleteOutput struct {
	Key       string `json:"key"`
	Namespace string `json:"namespace"`
	Deleted   bool   `json:"deleted"`
}

func handleMemoryDelete(ctx context.Context, toolCtx *mcpctx.ToolContext, input MemoryInput, namespace, userID string) (*mcp.CallToolResult, any, error) {
	if input.Key == "" {
		return nil, nil, mcpctx.NewValidationError("key is required for delete action", "key")
	}

	result, err := toolCtx.DB().DeleteMemoryByKeyAndUser(ctx, db.DeleteMemoryByKeyAndUserParams{
		Namespace: namespace,
		Key:       input.Key,
		UserID:    userID,
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to delete memory: %w", err)
	}

	rows, _ := result.RowsAffected()
	if rows == 0 {
		return nil, nil, mcpctx.NewNotFoundError(
			fmt.Sprintf("no memory found with key '%s' in namespace '%s'", input.Key, namespace))
	}

	return nil, MemoryDeleteOutput{
		Key:       input.Key,
		Namespace: namespace,
		Deleted:   true,
	}, nil
}

// MemoryClearOutput defines output for memory.clear.
type MemoryClearOutput struct {
	Namespace string `json:"namespace"`
	Cleared   int64  `json:"cleared"`
}

func handleMemoryClear(ctx context.Context, toolCtx *mcpctx.ToolContext, input MemoryInput, namespace, userID string) (*mcp.CallToolResult, any, error) {
	result, err := toolCtx.DB().DeleteMemoriesByNamespaceAndUser(ctx, db.DeleteMemoriesByNamespaceAndUserParams{
		NamespacePrefix: sql.NullString{String: namespace, Valid: true},
		UserID:          userID,
	})
	if err != nil {
		return nil, nil, fmt.Errorf("failed to clear memories: %w", err)
	}

	rows, _ := result.RowsAffected()

	return nil, MemoryClearOutput{
		Namespace: namespace,
		Cleared:   rows,
	}, nil
}

// registerMemoryToolToRegistry registers memory tool to the direct-call registry.
func registerMemoryToolToRegistry(registry *ToolRegistry, toolCtx *mcpctx.ToolContext) {
	registry.Register("memory", func(ctx context.Context, args json.RawMessage) (interface{}, error) {
		var input MemoryInput
		if err := json.Unmarshal(args, &input); err != nil {
			return nil, fmt.Errorf("invalid input: %w", err)
		}
		handler := memoryHandler(toolCtx)
		_, output, err := handler(ctx, nil, input)
		return output, err
	})
}
