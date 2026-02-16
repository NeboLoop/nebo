package ai

import (
	"context"
	"encoding/json"

	"github.com/neboloop/nebo/internal/agent/session"
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
	EventTypeMessage    StreamEventType = "message" // Full message from CLI provider's internal loop
)

// StreamEvent represents a streaming response event
type StreamEvent struct {
	Type     StreamEventType  `json:"type"`
	Text     string           `json:"text,omitempty"`
	ToolCall *ToolCall        `json:"tool_call,omitempty"`
	Error    error            `json:"error,omitempty"`
	Message  *session.Message `json:"message,omitempty"` // For CLI provider intermediate messages
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

	// User context for gateway apps — per-request identity
	UserToken string `json:"user_token,omitempty"` // NeboLoop JWT
	UserID    string `json:"user_id,omitempty"`    // User UUID
	UserPlan  string `json:"user_plan,omitempty"`  // Plan tier
}

// Provider interface for AI providers
type Provider interface {
	// ID returns the provider identifier (e.g., "anthropic", "openai")
	ID() string

	// ProfileID returns the auth profile ID if this provider has one
	// Returns empty string for providers without profile tracking
	ProfileID() string

	// HandlesTools returns true if this provider executes tools itself
	// (e.g., CLI providers via MCP). When true, the runner forwards
	// tool call events to the client for display but does not re-execute them.
	HandlesTools() bool

	// Stream sends a request and returns a channel of streaming events
	Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error)
}

// ProfiledProvider wraps a Provider with auth profile tracking
// This enables per-request profile ID tracking for cooldown and usage stats
type ProfiledProvider struct {
	Provider  Provider
	profileID string
}

// NewProfiledProvider creates a provider with auth profile tracking
func NewProfiledProvider(p Provider, profileID string) *ProfiledProvider {
	return &ProfiledProvider{
		Provider:  p,
		profileID: profileID,
	}
}

// ID delegates to the underlying provider
func (p *ProfiledProvider) ID() string {
	return p.Provider.ID()
}

// ProfileID returns the auth profile ID for this provider
func (p *ProfiledProvider) ProfileID() string {
	return p.profileID
}

// HandlesTools delegates to the underlying provider
func (p *ProfiledProvider) HandlesTools() bool {
	return p.Provider.HandlesTools()
}

// Stream delegates to the underlying provider
func (p *ProfiledProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	return p.Provider.Stream(ctx, req)
}

// ProfileTracker records usage and errors for auth profiles
// Implement this interface with AuthProfileManager for production use
type ProfileTracker interface {
	// RecordUsage marks a profile as successfully used
	RecordUsage(ctx context.Context, profileID string) error
	// RecordErrorWithCooldownString records an error and applies exponential backoff cooldown
	// Reason should be one of: "billing", "rate_limit", "auth", "timeout", "other"
	RecordErrorWithCooldownString(ctx context.Context, profileID string, reason string) error
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

// IsRoleOrderingError checks if an error is due to message role ordering issues
// These occur when messages don't alternate properly between user/assistant
// Auto-reset session on these errors
func IsRoleOrderingError(err error) bool {
	if err == nil {
		return false
	}
	msg := err.Error()
	keywords := []string{
		"roles must alternate",
		"incorrect role information",
		"function call turn comes immediately after",
		"expected alternating",
		"must be followed by",
	}
	for _, kw := range keywords {
		if containsIgnoreCase(msg, kw) {
			return true
		}
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

// ClassifyErrorReason determines the category of error for cooldown duration
// Returns: "billing", "rate_limit", "auth", "timeout", or "other"
func ClassifyErrorReason(err error) string {
	if err == nil {
		return "other"
	}

	msg := err.Error()
	lowerMsg := toLower(msg)

	// Check for ProviderError first
	if pe, ok := err.(*ProviderError); ok {
		// Check error code
		switch pe.Code {
		case "rate_limit_exceeded":
			return "rate_limit"
		case "authentication_error", "invalid_api_key", "unauthorized":
			return "auth"
		case "insufficient_quota", "billing_error", "payment_required":
			return "billing"
		}

		// Check error type
		switch pe.Type {
		case "rate_limit_error":
			return "rate_limit"
		case "authentication_error":
			return "auth"
		}
	}

	// Pattern matching on error message
	// Billing/quota errors
	billingPatterns := []string{
		"billing", "quota", "payment", "credit", "insufficient",
		"subscription", "exceeded your", "spending limit",
	}
	for _, p := range billingPatterns {
		if containsIgnoreCase(lowerMsg, p) {
			return "billing"
		}
	}

	// Rate limit errors
	rateLimitPatterns := []string{
		"rate limit", "rate_limit", "too many requests", "429",
		"throttle", "throttling", "slow down",
	}
	for _, p := range rateLimitPatterns {
		if containsIgnoreCase(lowerMsg, p) {
			return "rate_limit"
		}
	}

	// Auth errors
	authPatterns := []string{
		"authentication", "unauthorized", "invalid.*key", "api key",
		"401", "forbidden", "403", "invalid credentials",
	}
	for _, p := range authPatterns {
		if containsIgnoreCase(lowerMsg, p) {
			return "auth"
		}
	}

	// Timeout errors
	timeoutPatterns := []string{
		"timeout", "timed out", "deadline exceeded", "context deadline",
		"ETIMEDOUT", "ESOCKETTIMEDOUT", "context canceled",
	}
	for _, p := range timeoutPatterns {
		if containsIgnoreCase(lowerMsg, p) {
			return "timeout"
		}
	}

	return "other"
}

// toLower is a simple lowercase helper
func toLower(s string) string {
	result := make([]byte, len(s))
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' {
			result[i] = c + 32
		} else {
			result[i] = c
		}
	}
	return string(result)
}
