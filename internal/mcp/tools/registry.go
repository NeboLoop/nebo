package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"gobot/internal/mcp/mcpctx"
)

// ToolFunc is a function that executes a tool with JSON args.
// This allows tools to be called directly without going through the MCP protocol.
type ToolFunc func(ctx context.Context, args json.RawMessage) (interface{}, error)

// ToolRegistry stores tool handlers for direct invocation.
type ToolRegistry struct {
	mu    sync.RWMutex
	tools map[string]ToolFunc
}

// NewToolRegistry creates a new tool registry.
func NewToolRegistry() *ToolRegistry {
	return &ToolRegistry{
		tools: make(map[string]ToolFunc),
	}
}

// Register adds a tool to the registry.
func (r *ToolRegistry) Register(name string, fn ToolFunc) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.tools[name] = fn
}

// Call invokes a tool by name with the given arguments.
func (r *ToolRegistry) Call(ctx context.Context, name string, args map[string]interface{}) (interface{}, error) {
	r.mu.RLock()
	fn, ok := r.tools[name]
	r.mu.RUnlock()

	if !ok {
		return nil, fmt.Errorf("tool not found: %s", name)
	}

	argsJSON, err := json.Marshal(args)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal args: %w", err)
	}

	return fn(ctx, argsJSON)
}

// Has returns true if the registry has a tool with the given name.
func (r *ToolRegistry) Has(name string) bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	_, ok := r.tools[name]
	return ok
}

// List returns all registered tool names.
func (r *ToolRegistry) List() []string {
	r.mu.RLock()
	defer r.mu.RUnlock()

	names := make([]string, 0, len(r.tools))
	for name := range r.tools {
		names = append(names, name)
	}
	return names
}

// NewRegistryWithTools creates a registry with all tools registered.
// This is used by the agent executor to call tools directly.
func NewRegistryWithTools(toolCtx *mcpctx.ToolContext) *ToolRegistry {
	registry := NewToolRegistry()

	// Register all tool handlers (unified resource/action pattern)
	registerUserToolToRegistry(registry, toolCtx)
	registerNotificationToolToRegistry(registry, toolCtx)

	return registry
}
