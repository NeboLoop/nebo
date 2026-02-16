package runner

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/neboloop/nebo/internal/agent/session"
)

func TestCollectToolFailures(t *testing.T) {
	// Create messages with tool calls and results
	toolCalls := []session.ToolCall{
		{ID: "call-1", Name: "bash"},
		{ID: "call-2", Name: "read"},
		{ID: "call-3", Name: "grep"},
	}
	toolCallsJSON, _ := json.Marshal(toolCalls)

	toolResults := []session.ToolResult{
		{ToolCallID: "call-1", Content: "Command exited with code 1\nENOENT: missing file", IsError: true},
		{ToolCallID: "call-2", Content: "File contents here", IsError: false}, // Not an error
		{ToolCallID: "call-3", Content: "Permission denied", IsError: true},
	}
	toolResultsJSON, _ := json.Marshal(toolResults)

	messages := []session.Message{
		{Role: "user", Content: "Run some commands"},
		{Role: "assistant", Content: "I'll run those", ToolCalls: toolCallsJSON},
		{Role: "tool", ToolResults: toolResultsJSON},
	}

	failures := CollectToolFailures(messages)

	// Should have 2 failures (call-1 and call-3, not call-2 since it's not an error)
	if len(failures) != 2 {
		t.Errorf("expected 2 failures, got %d", len(failures))
	}

	// Check first failure
	if failures[0].ToolCallID != "call-1" {
		t.Errorf("expected call-1, got %s", failures[0].ToolCallID)
	}
	if failures[0].ToolName != "bash" {
		t.Errorf("expected bash, got %s", failures[0].ToolName)
	}
	if !strings.Contains(failures[0].Summary, "ENOENT") {
		t.Errorf("expected summary to contain ENOENT, got %s", failures[0].Summary)
	}

	// Check second failure
	if failures[1].ToolCallID != "call-3" {
		t.Errorf("expected call-3, got %s", failures[1].ToolCallID)
	}
	if failures[1].ToolName != "grep" {
		t.Errorf("expected grep, got %s", failures[1].ToolName)
	}
}

func TestCollectToolFailures_Dedupe(t *testing.T) {
	toolCalls := []session.ToolCall{{ID: "call-1", Name: "exec"}}
	toolCallsJSON, _ := json.Marshal(toolCalls)

	// Same tool call ID appears twice (shouldn't happen normally, but test dedupe)
	toolResults1 := []session.ToolResult{
		{ToolCallID: "call-1", Content: "First error", IsError: true},
	}
	toolResultsJSON1, _ := json.Marshal(toolResults1)

	toolResults2 := []session.ToolResult{
		{ToolCallID: "call-1", Content: "Duplicate error", IsError: true},
	}
	toolResultsJSON2, _ := json.Marshal(toolResults2)

	messages := []session.Message{
		{Role: "assistant", ToolCalls: toolCallsJSON},
		{Role: "tool", ToolResults: toolResultsJSON1},
		{Role: "tool", ToolResults: toolResultsJSON2},
	}

	failures := CollectToolFailures(messages)

	// Should only have 1 failure (deduped by tool_call_id)
	if len(failures) != 1 {
		t.Errorf("expected 1 failure (deduped), got %d", len(failures))
	}
}

func TestFormatToolFailuresSection(t *testing.T) {
	failures := []ToolFailure{
		{ToolCallID: "call-1", ToolName: "exec", Summary: "ENOENT: missing file", Meta: "exitCode=1"},
		{ToolCallID: "call-2", ToolName: "read", Summary: "Permission denied"},
	}

	section := FormatToolFailuresSection(failures)

	if section == "" {
		t.Error("expected non-empty section")
	}

	if !strings.Contains(section, "## Tool Failures") {
		t.Error("expected section header")
	}

	if !strings.Contains(section, "exec (exitCode=1): ENOENT: missing file") {
		t.Errorf("expected first failure format, got %s", section)
	}

	if !strings.Contains(section, "read: Permission denied") {
		t.Errorf("expected second failure (no meta), got %s", section)
	}
}

func TestFormatToolFailuresSection_Empty(t *testing.T) {
	section := FormatToolFailuresSection(nil)
	if section != "" {
		t.Error("expected empty section for nil failures")
	}

	section = FormatToolFailuresSection([]ToolFailure{})
	if section != "" {
		t.Error("expected empty section for empty failures")
	}
}

func TestFormatToolFailuresSection_Overflow(t *testing.T) {
	// Create more than MaxToolFailures
	failures := make([]ToolFailure, 10)
	for i := 0; i < 10; i++ {
		failures[i] = ToolFailure{
			ToolCallID: "call-" + string(rune('0'+i)),
			ToolName:   "exec",
			Summary:    "error " + string(rune('0'+i)),
		}
	}

	section := FormatToolFailuresSection(failures)

	// Should include overflow message
	if !strings.Contains(section, "...and 2 more") {
		t.Errorf("expected overflow indicator for 2 extra failures, got %s", section)
	}
}

func TestNormalizeFailureText(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"hello world", "hello world"},
		{"hello\nworld", "hello world"},
		{"hello  \t  world", "hello world"},
		{"  trimmed  ", "trimmed"},
		{"multiple\n\n\nlines", "multiple lines"},
	}

	for _, tt := range tests {
		result := normalizeFailureText(tt.input)
		if result != tt.expected {
			t.Errorf("normalizeFailureText(%q) = %q, want %q", tt.input, result, tt.expected)
		}
	}
}

func TestTruncateText(t *testing.T) {
	tests := []struct {
		input    string
		maxChars int
		expected string
	}{
		{"short", 10, "short"},
		{"exactly10!", 10, "exactly10!"},
		{"this is too long", 10, "this is..."},
		{"abc", 3, "abc"},
		{"abcd", 3, "abc"},
	}

	for _, tt := range tests {
		result := truncateText(tt.input, tt.maxChars)
		if result != tt.expected {
			t.Errorf("truncateText(%q, %d) = %q, want %q", tt.input, tt.maxChars, result, tt.expected)
		}
	}
}

func TestExtractFailureMeta(t *testing.T) {
	tests := []struct {
		input    string
		contains []string
	}{
		{"Command exited with code 1", []string{"exitCode=1"}},
		{"exit code 127", []string{"exitCode=127"}},
		{"Command timed out after 30s", []string{"status=timeout"}},
		{"Error: permission denied", []string{"status=permission_denied"}},
		{"ENOENT: file not found", []string{"status=not_found"}},
		{"Regular error message", []string{}},
	}

	for _, tt := range tests {
		result := extractFailureMeta(tt.input)
		for _, want := range tt.contains {
			if !strings.Contains(result, want) {
				t.Errorf("extractFailureMeta(%q) = %q, should contain %q", tt.input, result, want)
			}
		}
	}
}

func TestEnhancedSummary(t *testing.T) {
	toolCalls := []session.ToolCall{{ID: "call-1", Name: "bash"}}
	toolCallsJSON, _ := json.Marshal(toolCalls)

	toolResults := []session.ToolResult{
		{ToolCallID: "call-1", Content: "Error occurred", IsError: true},
	}
	toolResultsJSON, _ := json.Marshal(toolResults)

	messages := []session.Message{
		{Role: "user", Content: "Test request"},
		{Role: "assistant", ToolCalls: toolCallsJSON},
		{Role: "tool", ToolResults: toolResultsJSON},
	}

	baseSummary := "[Previous conversation summary]\n- User request: Test request"
	enhanced := EnhancedSummary(messages, baseSummary)

	if !strings.Contains(enhanced, baseSummary) {
		t.Error("enhanced summary should contain base summary")
	}

	if !strings.Contains(enhanced, "## Tool Failures") {
		t.Error("enhanced summary should contain tool failures section")
	}

	if !strings.Contains(enhanced, "bash") {
		t.Error("enhanced summary should mention the tool name")
	}
}

func TestSanitizeForSummary(t *testing.T) {
	tests := []struct {
		name  string
		input string
		want  string
	}{
		{"normal text", "User asked about Go", "User asked about Go"},
		{"strips null bytes", "hello\x00world", "helloworld"},
		{"strips bell", "test\x07text", "testtext"},
		{"strips escape sequences", "test\x1b[31mred\x1b[0m", "test[31mred[0m"},
		{"preserves newlines", "line1\nline2", "line1\nline2"},
		{"preserves tabs", "col1\tcol2", "col1\tcol2"},
		{"empty string", "", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := sanitizeForSummary(tt.input)
			if got != tt.want {
				t.Errorf("sanitizeForSummary(%q) = %q, want %q", tt.input, got, tt.want)
			}
		})
	}
}

func TestEnhancedSummary_NoFailures(t *testing.T) {
	messages := []session.Message{
		{Role: "user", Content: "Hello"},
		{Role: "assistant", Content: "Hi there!"},
	}

	baseSummary := "[Previous conversation summary]\n- User: Hello"
	enhanced := EnhancedSummary(messages, baseSummary)

	// Should be unchanged when no failures
	if enhanced != baseSummary {
		t.Errorf("expected unchanged summary, got %s", enhanced)
	}
}
