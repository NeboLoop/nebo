package ai

import (
	"context"
	"encoding/json"

	"nebo/agent/session"
)

// StreamEventType defines the type of streaming event
type StreamEventType string

const (
	EventTypeText       StreamEventType = "text"
	EventTypeToolCall   StreamEventType = "tool_call"
	EventTypeToolResult StreamEventType = "tool_result"
	EventTypeError      StreamEventType = "error"
	EventTypeDone       StreamEventType = "done"
	EventTypeThinking   StreamEventType = "thinking"
)

// StreamEvent represents a streaming response event
type StreamEvent struct {
	Type     StreamEventType `json:"type"`
	Text     string          `json:"text,omitempty"`
	ToolCall *ToolCall       `json:"tool_call,omitempty"`
	Error    error           `json:"error,omitempty"`
}

// ToolCall represents a tool invocation from the AI
type ToolCall struct {
	ID    string          `json:"id"`
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
}

// ToolDefinition describes a tool available to the AI
type ToolDefinition struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	InputSchema json.RawMessage `json:"input_schema"`
}

// ChatRequest represents a request to the AI provider
type ChatRequest struct {
	Messages       []session.Message `json:"messages"`
	Tools          []ToolDefinition  `json:"tools,omitempty"`
	MaxTokens      int               `json:"max_tokens,omitempty"`
	Temperature    float64           `json:"temperature,omitempty"`
	System         string            `json:"system,omitempty"`
	Model          string            `json:"model,omitempty"`           // Model override (e.g., "haiku", "sonnet", "opus")
	EnableThinking bool              `json:"enable_thinking,omitempty"` // Enable extended thinking mode for reasoning
}

// Provider interface for AI providers
type Provider interface {
	// ID returns the provider identifier
	ID() string

	// Stream sends a request and returns a channel of streaming events
	Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error)
}

// ProviderError represents an error from a provider
type ProviderError struct {
	Code    string `json:"code,omitempty"`
	Message string `json:"message"`
	Type    string `json:"type,omitempty"`
}

func (e *ProviderError) Error() string {
	return e.Message
}

// IsContextOverflow checks if an error indicates context window overflow
func IsContextOverflow(err error) bool {
	if pe, ok := err.(*ProviderError); ok {
		return pe.Code == "context_length_exceeded" ||
			pe.Type == "invalid_request_error" && containsContextError(pe.Message)
	}
	return false
}

// IsRateLimitOrAuth checks if an error is due to rate limiting or auth issues
func IsRateLimitOrAuth(err error) bool {
	if pe, ok := err.(*ProviderError); ok {
		return pe.Code == "rate_limit_exceeded" ||
			pe.Code == "authentication_error" ||
			pe.Type == "rate_limit_error" ||
			pe.Type == "authentication_error"
	}
	return false
}

// containsContextError checks if error message indicates context overflow
func containsContextError(msg string) bool {
	keywords := []string{"context", "token", "length", "exceeded", "too long"}
	for _, kw := range keywords {
		if containsIgnoreCase(msg, kw) {
			return true
		}
	}
	return false
}

func containsIgnoreCase(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || containsIgnoreCase(s[1:], substr) || s[:len(substr)] == substr)
}
