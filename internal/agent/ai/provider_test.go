package ai

import (
	"fmt"
	"testing"
)

func TestStreamEventTypes(t *testing.T) {
	// Verify all event types are defined
	eventTypes := []StreamEventType{
		EventTypeText,
		EventTypeToolCall,
		EventTypeToolResult,
		EventTypeError,
		EventTypeDone,
		EventTypeThinking,
	}

	for _, et := range eventTypes {
		if et == "" {
			t.Error("event type is empty string")
		}
	}

	// Verify specific values
	if EventTypeText != "text" {
		t.Errorf("expected 'text', got %s", EventTypeText)
	}
	if EventTypeToolCall != "tool_call" {
		t.Errorf("expected 'tool_call', got %s", EventTypeToolCall)
	}
	if EventTypeDone != "done" {
		t.Errorf("expected 'done', got %s", EventTypeDone)
	}
}

func TestProviderError(t *testing.T) {
	err := &ProviderError{
		Code:    "test_error",
		Message: "Test error message",
		Type:    "test_type",
	}

	if err.Error() != "Test error message" {
		t.Errorf("expected 'Test error message', got %s", err.Error())
	}
}

func TestIsContextOverflow(t *testing.T) {
	tests := []struct {
		name     string
		err      error
		expected bool
	}{
		{
			name: "context length exceeded code",
			err: &ProviderError{
				Code:    "context_length_exceeded",
				Message: "Context too long",
			},
			expected: true,
		},
		{
			name: "invalid request with context error",
			err: &ProviderError{
				Type:    "invalid_request_error",
				Message: "The context is too long",
			},
			expected: true,
		},
		{
			name: "invalid request with token error",
			err: &ProviderError{
				Type:    "invalid_request_error",
				Message: "Maximum token length exceeded",
			},
			expected: true,
		},
		{
			name: "rate limit error",
			err: &ProviderError{
				Code:    "rate_limit_exceeded",
				Message: "Rate limited",
			},
			expected: false,
		},
		{
			name: "regular error",
			err: &ProviderError{
				Code:    "other_error",
				Message: "Something else went wrong",
			},
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsContextOverflow(tt.err)
			if result != tt.expected {
				t.Errorf("IsContextOverflow() = %v, expected %v", result, tt.expected)
			}
		})
	}
}

func TestIsRateLimitOrAuth(t *testing.T) {
	tests := []struct {
		name     string
		err      error
		expected bool
	}{
		{
			name: "rate limit exceeded code",
			err: &ProviderError{
				Code:    "rate_limit_exceeded",
				Message: "Rate limited",
			},
			expected: true,
		},
		{
			name: "authentication error code",
			err: &ProviderError{
				Code:    "authentication_error",
				Message: "Invalid API key",
			},
			expected: true,
		},
		{
			name: "rate limit error type",
			err: &ProviderError{
				Type:    "rate_limit_error",
				Message: "Too many requests",
			},
			expected: true,
		},
		{
			name: "authentication error type",
			err: &ProviderError{
				Type:    "authentication_error",
				Message: "Unauthorized",
			},
			expected: true,
		},
		{
			name: "context overflow (not rate limit)",
			err: &ProviderError{
				Code:    "context_length_exceeded",
				Message: "Context too long",
			},
			expected: false,
		},
		{
			name: "other error",
			err: &ProviderError{
				Code:    "server_error",
				Message: "Internal error",
			},
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsRateLimitOrAuth(tt.err)
			if result != tt.expected {
				t.Errorf("IsRateLimitOrAuth() = %v, expected %v", result, tt.expected)
			}
		})
	}
}

func TestContainsContextError(t *testing.T) {
	tests := []struct {
		msg      string
		expected bool
	}{
		{"The context is too long", true},
		{"Maximum token limit exceeded", true},
		{"Request length exceeded", true},
		{"Too many tokens", true},
		{"Something else", false},
		{"Error occurred", false},
		{"", false},
	}

	for _, tt := range tests {
		t.Run(tt.msg, func(t *testing.T) {
			result := containsContextError(tt.msg)
			if result != tt.expected {
				t.Errorf("containsContextError(%q) = %v, expected %v", tt.msg, result, tt.expected)
			}
		})
	}
}

func TestIsRoleOrderingError(t *testing.T) {
	tests := []struct {
		name     string
		err      error
		expected bool
	}{
		{
			name:     "roles must alternate",
			err:      fmt.Errorf("roles must alternate between user and assistant"),
			expected: true,
		},
		{
			name:     "incorrect role information",
			err:      fmt.Errorf("incorrect role information in message"),
			expected: true,
		},
		{
			name:     "function call turn comes immediately after",
			err:      fmt.Errorf("function call turn comes immediately after another function call"),
			expected: true,
		},
		{
			name:     "expected alternating",
			err:      fmt.Errorf("expected alternating user/assistant messages"),
			expected: true,
		},
		{
			name:     "nil error",
			err:      nil,
			expected: false,
		},
		{
			name:     "context overflow (not role ordering)",
			err:      fmt.Errorf("context length exceeded"),
			expected: false,
		},
		{
			name:     "rate limit (not role ordering)",
			err:      fmt.Errorf("rate limit exceeded"),
			expected: false,
		},
		{
			name:     "generic error",
			err:      fmt.Errorf("something went wrong"),
			expected: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsRoleOrderingError(tt.err)
			if result != tt.expected {
				t.Errorf("IsRoleOrderingError() = %v, expected %v", result, tt.expected)
			}
		})
	}
}

func TestStreamEvent(t *testing.T) {
	// Test text event
	textEvent := StreamEvent{
		Type: EventTypeText,
		Text: "Hello world",
	}
	if textEvent.Type != EventTypeText {
		t.Error("text event type mismatch")
	}
	if textEvent.Text != "Hello world" {
		t.Error("text content mismatch")
	}

	// Test tool call event
	toolEvent := StreamEvent{
		Type: EventTypeToolCall,
		ToolCall: &ToolCall{
			ID:    "call_123",
			Name:  "bash",
			Input: []byte(`{"command": "ls"}`),
		},
	}
	if toolEvent.ToolCall == nil {
		t.Error("tool call should not be nil")
	}
	if toolEvent.ToolCall.Name != "bash" {
		t.Error("tool call name mismatch")
	}

	// Test error event
	testErr := &ProviderError{Message: "test error"}
	errEvent := StreamEvent{
		Type:  EventTypeError,
		Error: testErr,
	}
	if errEvent.Error == nil {
		t.Error("error should not be nil")
	}
}

func TestClassifyErrorReason(t *testing.T) {
	tests := []struct {
		name     string
		err      error
		expected string
	}{
		{"nil error", nil, "other"},
		{
			"rate limit code",
			&ProviderError{Code: "rate_limit_exceeded", Message: "Rate limited"},
			"rate_limit",
		},
		{
			"auth error code",
			&ProviderError{Code: "authentication_error", Message: "Invalid key"},
			"auth",
		},
		{
			"billing code",
			&ProviderError{Code: "insufficient_quota", Message: "Quota exceeded"},
			"billing",
		},
		{
			"rate limit message",
			fmt.Errorf("Error: too many requests, please slow down"),
			"rate_limit",
		},
		{
			"billing message",
			fmt.Errorf("Your billing quota has been exceeded"),
			"billing",
		},
		{
			"auth message",
			fmt.Errorf("401 Unauthorized: invalid api key"),
			"auth",
		},
		{
			"timeout message",
			fmt.Errorf("request timed out after 30s"),
			"timeout",
		},
		{
			"context deadline exceeded",
			fmt.Errorf("context deadline exceeded"),
			"timeout",
		},
		{
			"generic error",
			fmt.Errorf("something went wrong"),
			"other",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ClassifyErrorReason(tt.err)
			if result != tt.expected {
				t.Errorf("ClassifyErrorReason() = %s, expected %s", result, tt.expected)
			}
		})
	}
}

func TestToolDefinition(t *testing.T) {
	td := ToolDefinition{
		Name:        "test_tool",
		Description: "A test tool",
		InputSchema: []byte(`{"type": "object", "properties": {}}`),
	}

	if td.Name != "test_tool" {
		t.Errorf("expected name 'test_tool', got %s", td.Name)
	}
	if td.Description != "A test tool" {
		t.Errorf("expected description 'A test tool', got %s", td.Description)
	}
	if len(td.InputSchema) == 0 {
		t.Error("input schema should not be empty")
	}
}

func TestChatRequest(t *testing.T) {
	req := &ChatRequest{
		MaxTokens:   1000,
		Temperature: 0.7,
		System:      "You are a helpful assistant",
	}

	if req.MaxTokens != 1000 {
		t.Errorf("expected MaxTokens 1000, got %d", req.MaxTokens)
	}
	if req.Temperature != 0.7 {
		t.Errorf("expected Temperature 0.7, got %f", req.Temperature)
	}
	if req.System != "You are a helpful assistant" {
		t.Errorf("unexpected system prompt: %s", req.System)
	}
}
