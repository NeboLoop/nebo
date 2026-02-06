package ai

import (
	"context"
	"testing"
	"time"

	"github.com/nebolabs/nebo/internal/agent/session"
)

func TestNewCLIProvider(t *testing.T) {
	p := NewCLIProvider("test", "echo", []string{"hello"})
	if p == nil {
		t.Fatal("NewCLIProvider returned nil")
	}
	if p.name != "test" {
		t.Errorf("expected name 'test', got %s", p.name)
	}
	if p.command != "echo" {
		t.Errorf("expected command 'echo', got %s", p.command)
	}
}

func TestNewClaudeCodeProvider(t *testing.T) {
	p := NewClaudeCodeProvider(25)
	if p == nil {
		t.Fatal("NewClaudeCodeProvider returned nil")
	}
	if p.name != "claude-code" {
		t.Errorf("expected name 'claude-code', got %s", p.name)
	}
	if p.command != "claude" {
		t.Errorf("expected command 'claude', got %s", p.command)
	}
}

func TestCLIProviderID(t *testing.T) {
	p := NewCLIProvider("my-provider", "test", nil)
	if p.ID() != "my-provider" {
		t.Errorf("expected ID 'my-provider', got %s", p.ID())
	}
}

func TestCLIProviderStreamWithEcho(t *testing.T) {
	// Test with echo command which is always available
	p := NewCLIProvider("echo-test", "echo", []string{})

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	req := &ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: "test message"},
		},
	}

	events, err := p.Stream(ctx, req)
	if err != nil {
		t.Fatalf("Stream failed: %v", err)
	}

	var gotText bool
	var gotDone bool
	var receivedText string

	for event := range events {
		switch event.Type {
		case EventTypeText:
			gotText = true
			receivedText += event.Text
		case EventTypeDone:
			gotDone = true
		case EventTypeError:
			// Echo shouldn't error
			t.Errorf("unexpected error: %v", event.Error)
		}
	}

	if !gotText {
		t.Error("expected text event")
	}
	if !gotDone {
		t.Error("expected done event")
	}
	if receivedText == "" {
		t.Error("expected some text output")
	}
}

func TestCLIProviderStreamContextCancel(t *testing.T) {
	// Use sleep command to test cancellation
	if !CheckCLIAvailable("sleep") {
		t.Skip("sleep command not available")
	}

	p := NewCLIProvider("sleep-test", "sleep", []string{"10"})

	ctx, cancel := context.WithCancel(context.Background())

	req := &ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: ""},
		},
	}

	events, err := p.Stream(ctx, req)
	if err != nil {
		t.Fatalf("Stream failed: %v", err)
	}

	// Cancel immediately
	cancel()

	// Should receive events and terminate
	timeout := time.After(2 * time.Second)
	for {
		select {
		case _, ok := <-events:
			if !ok {
				return // Channel closed, test passed
			}
		case <-timeout:
			t.Fatal("timed out waiting for stream to close after cancel")
		}
	}
}

func TestCLIProviderStreamCommandNotFound(t *testing.T) {
	p := NewCLIProvider("nonexistent", "nonexistent-command-12345", nil)

	ctx := context.Background()
	req := &ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: "test"},
		},
	}

	events, err := p.Stream(ctx, req)
	if err != nil {
		t.Fatalf("Stream should not return error immediately: %v", err)
	}

	var gotError bool
	for event := range events {
		if event.Type == EventTypeError {
			gotError = true
		}
	}

	if !gotError {
		t.Error("expected error event for nonexistent command")
	}
}

func TestBuildPromptFromMessages(t *testing.T) {
	messages := []session.Message{
		{Role: "system", Content: "You are helpful"},
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there!"},
		{Role: "user", Content: "How are you?"},
	}

	prompt := buildPromptFromMessages(messages)

	if prompt == "" {
		t.Error("prompt should not be empty")
	}

	// Check all messages are included
	if !containsStr(prompt, "You are helpful") {
		t.Error("prompt should contain system message")
	}
	if !containsStr(prompt, "Hello") {
		t.Error("prompt should contain first user message")
	}
	if !containsStr(prompt, "Hi there!") {
		t.Error("prompt should contain assistant message")
	}
	if !containsStr(prompt, "How are you?") {
		t.Error("prompt should contain second user message")
	}
}

func TestCheckCLIAvailable(t *testing.T) {
	// echo should be available on all systems
	if !CheckCLIAvailable("echo") {
		t.Error("echo should be available")
	}

	// nonexistent command should not be available
	if CheckCLIAvailable("nonexistent-command-xyz-12345") {
		t.Error("nonexistent command should not be available")
	}
}

func TestGetAvailableCLIProviders(t *testing.T) {
	providers := GetAvailableCLIProviders()

	// Result should be a slice (may be empty if no CLIs installed)
	if providers == nil {
		t.Error("GetAvailableCLIProviders should return a slice, not nil")
	}
}

func TestParseLine(t *testing.T) {
	p := NewCLIProvider("test", "echo", nil)

	tests := []struct {
		name     string
		line     string
		expected StreamEventType
		text     string
	}{
		{
			name:     "plain text",
			line:     "Hello world",
			expected: EventTypeText,
			text:     "Hello world\n",
		},
		{
			name:     "empty line",
			line:     "",
			expected: EventTypeText,
			text:     "",
		},
		{
			name:     "content_block_delta text",
			line:     `{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}`,
			expected: EventTypeText,
			text:     "Hello",
		},
		{
			name:     "content_block_delta thinking",
			line:     `{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}`,
			expected: EventTypeThinking,
			text:     "Let me think...",
		},
		{
			name:     "content_block_start tool_use",
			line:     `{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_123","name":"Bash","input":{}}}`,
			expected: EventTypeText, // Tool use handling moved to Stream() — parseLine returns empty
		},
		{
			name:     "result",
			line:     `{"type":"result","result":"Final output here"}`,
			expected: EventTypeText,
			text:     "Final output here",
		},
		{
			name:     "error",
			line:     `{"type":"error","error":"Something went wrong"}`,
			expected: EventTypeError,
		},
		{
			name:     "message_stop",
			line:     `{"type":"message_stop"}`,
			expected: EventTypeText,
			text:     "",
		},
		{
			name:     "system message",
			line:     `{"type":"system","message":"Initializing..."}`,
			expected: EventTypeText,
			text:     "[system] Initializing...\n",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			event := p.parseLine(tt.line)
			if event.Type != tt.expected {
				t.Errorf("expected type %s, got %s", tt.expected, event.Type)
			}
			if tt.text != "" && event.Text != tt.text {
				t.Errorf("expected text %q, got %q", tt.text, event.Text)
			}
		})
	}
}

func containsStr(s, substr string) bool {
	return len(s) >= len(substr) && (s == substr || containsStr(s[1:], substr) || s[:len(substr)] == substr)
}
