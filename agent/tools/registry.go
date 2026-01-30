package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"sync"

	"nebo/agent/ai"
)

// ToolResult represents the result of a tool execution
type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error,omitempty"`
}

// Tool interface that all tools must implement
type Tool interface {
	// Name returns the tool's unique name
	Name() string

	// Description returns a description for the AI
	Description() string

	// Schema returns the JSON schema for the tool's input
	Schema() json.RawMessage

	// Execute runs the tool with the given input
	Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error)

	// RequiresApproval returns true if this tool needs user approval
	RequiresApproval() bool
}

// Registry manages available tools
type Registry struct {
	mu     sync.RWMutex
	tools  map[string]Tool
	policy *Policy
}

// NewRegistry creates a new tool registry
func NewRegistry(policy *Policy) *Registry {
	return &Registry{
		tools:  make(map[string]Tool),
		policy: policy,
	}
}

// Register adds a tool to the registry
func (r *Registry) Register(tool Tool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.tools[tool.Name()] = tool
}

// Get returns a tool by name
func (r *Registry) Get(name string) (Tool, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	tool, ok := r.tools[name]
	return tool, ok
}

// List returns all tools as AI tool definitions
func (r *Registry) List() []ai.ToolDefinition {
	r.mu.RLock()
	defer r.mu.RUnlock()

	defs := make([]ai.ToolDefinition, 0, len(r.tools))
	for _, tool := range r.tools {
		defs = append(defs, ai.ToolDefinition{
			Name:        tool.Name(),
			Description: tool.Description(),
			InputSchema: tool.Schema(),
		})
	}
	return defs
}

// Execute runs a tool and returns the result
func (r *Registry) Execute(ctx context.Context, toolCall *ai.ToolCall) *ToolResult {
	fmt.Printf("[Registry] Executing tool: %s\n", toolCall.Name)

	r.mu.RLock()
	tool, ok := r.tools[toolCall.Name]
	r.mu.RUnlock()

	if !ok {
		fmt.Printf("[Registry] Unknown tool: %s\n", toolCall.Name)
		return &ToolResult{
			Content: fmt.Sprintf("Unknown tool: %s", toolCall.Name),
			IsError: true,
		}
	}

	// Check if approval is required
	if tool.RequiresApproval() && r.policy != nil {
		fmt.Printf("[Registry] Tool requires approval, policy level=%s\n", r.policy.Level)
		approved, err := r.policy.RequestApproval(ctx, tool.Name(), toolCall.Input)
		fmt.Printf("[Registry] Approval result: approved=%v err=%v\n", approved, err)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Approval error: %v", err),
				IsError: true,
			}
		}
		if !approved {
			return &ToolResult{
				Content: "Tool execution denied by user",
				IsError: true,
			}
		}
	} else {
		fmt.Printf("[Registry] No approval needed (requiresApproval=%v, hasPolicy=%v)\n",
			tool.RequiresApproval(), r.policy != nil)
	}

	fmt.Printf("[Registry] Calling tool.Execute...\n")
	result, err := tool.Execute(ctx, toolCall.Input)
	fmt.Printf("[Registry] tool.Execute returned: err=%v\n", err)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Tool error: %v", err),
			IsError: true,
		}
	}

	fmt.Printf("[Registry] Tool completed, content_len=%d, isError=%v\n", len(result.Content), result.IsError)
	return result
}

// SetPolicy updates the registry's policy
func (r *Registry) SetPolicy(policy *Policy) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.policy = policy
}

// RegisterDefaults registers the default set of tools
func (r *Registry) RegisterDefaults() {
	// Core file tools
	r.Register(NewBashTool(r.policy))
	r.Register(NewReadTool())
	r.Register(NewWriteTool())
	r.Register(NewEditTool())
	r.Register(NewGlobTool())
	r.Register(NewGrepTool())

	// Web tools
	r.Register(NewWebTool())
	r.Register(NewSearchTool())

	// Browser automation (headless by default)
	r.Register(NewBrowserTool(BrowserConfig{Headless: true}))

	// Screenshot (desktop capture)
	r.Register(NewScreenshotTool())

	// Vision (image analysis) - requires ANTHROPIC_API_KEY
	if apiKey := os.Getenv("ANTHROPIC_API_KEY"); apiKey != "" {
		r.Register(NewVisionTool(VisionConfig{APIKey: apiKey}))
	}

	// Memory (persistent facts) - initialize with defaults
	if memTool, err := NewMemoryTool(MemoryConfig{}); err == nil {
		r.Register(memTool)
	}

	// Process management
	r.Register(NewProcessTool())

	// Task/sub-agent spawning
	r.Register(NewTaskTool())
}
