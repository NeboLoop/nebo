package tools

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/neboloop/nebo/internal/agent/ai"
)

// DesktopQueueFunc wraps a tool execution so it runs on a serialized desktop lane.
// The function receives a callback that performs the actual tool execution and must
// call it exactly once, returning its result.
type DesktopQueueFunc func(ctx context.Context, execute func(ctx context.Context) *ToolResult) *ToolResult

// desktopToolNames is the set of tool names that must be serialized through
// the desktop lane because they control the screen, mouse, or keyboard.
var desktopToolNames = map[string]bool{
	"desktop":       true,
	"accessibility": true,
	"screenshot":    true,
	"app":           true,
	"browser":       true,
	"window":        true,
	"menubar":       true,
	"dialog":        true,
	"shortcuts":     true,
}

// IsDesktopTool returns true if the tool name is a desktop-category tool
// that should be serialized through LaneDesktop.
func IsDesktopTool(name string) bool {
	return desktopToolNames[name]
}

// SetDesktopQueue sets the function used to serialize desktop tool execution.
// When set, Execute() wraps desktop-category tool calls through this function.
func (r *Registry) SetDesktopQueue(fn DesktopQueueFunc) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.desktopQueue = fn
}

// executeWithDesktopQueue wraps a tool execution through the desktop queue
// if the tool is a desktop-category tool and the queue is configured.
func (r *Registry) executeWithDesktopQueue(ctx context.Context, tool Tool, toolCall *ai.ToolCall) *ToolResult {
	r.mu.RLock()
	queueFn := r.desktopQueue
	r.mu.RUnlock()

	executeFn := func(execCtx context.Context) *ToolResult {
		return r.executeTool(execCtx, tool, toolCall)
	}

	if queueFn != nil && IsDesktopTool(toolCall.Name) {
		fmt.Printf("[Registry] Routing desktop tool %s through LaneDesktop\n", toolCall.Name)
		return queueFn(ctx, executeFn)
	}

	return executeFn(ctx)
}

// executeTool performs the actual tool execution (approval check + execute).
func (r *Registry) executeTool(ctx context.Context, tool Tool, toolCall *ai.ToolCall) *ToolResult {
	// Check origin-based restrictions
	origin := GetOrigin(ctx)
	if r.policy != nil && r.policy.IsDeniedForOrigin(origin, toolCall.Name) {
		return &ToolResult{
			Content: fmt.Sprintf("Tool %q is not permitted for %s-origin requests", toolCall.Name, origin),
			IsError: true,
		}
	}

	// Check approval
	if tool.RequiresApproval() && r.policy != nil {
		approved, err := r.policy.RequestApproval(ctx, toolCall.Name, toolCall.Input)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Approval request failed: %v", err),
				IsError: true,
			}
		}
		if !approved {
			return &ToolResult{
				Content: fmt.Sprintf("Tool '%s' was not approved by the user", toolCall.Name),
				IsError: true,
			}
		}
	}

	result, err := tool.Execute(ctx, toolCall.Input)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Tool execution error: %v", err),
			IsError: true,
		}
	}
	if result == nil {
		return &ToolResult{Content: "Tool returned no result"}
	}

	// Truncate extremely large results to prevent context window pollution
	const maxResultLen = 100000
	if len(result.Content) > maxResultLen {
		result.Content = result.Content[:maxResultLen] + "\n\n[Output truncated — exceeded 100KB]"
	}

	return result
}

// wrapToolResult creates a ToolResult with the given error from JSON unmarshal
func wrapToolResult(content string, isError bool) *ToolResult {
	return &ToolResult{Content: content, IsError: isError}
}

func init() {
	// Verify the interface is satisfied (compile-time check)
	var _ json.Marshaler = json.RawMessage{}
	_ = wrapToolResult // suppress unused warning during development
}
