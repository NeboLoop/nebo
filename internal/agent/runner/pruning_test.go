package runner

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/session"
)

func defaultPruningConfig() config.ContextPruningConfig {
	return config.ContextPruningConfig{
		ContextTokens:        200000,
		SoftTrimRatio:        0.3,
		HardClearRatio:       0.5,
		KeepLastAssistant:    3,
		SoftTrimMaxChars:     4000,
		SoftTrimHead:         1500,
		SoftTrimTail:         1500,
		HardClearPlaceholder: "[Old tool result cleared]",
	}
}

func makeToolCalls(calls ...session.ToolCall) json.RawMessage {
	data, _ := json.Marshal(calls)
	return data
}

func makeToolResults(results ...session.ToolResult) json.RawMessage {
	data, _ := json.Marshal(results)
	return data
}

func TestPruneContext_NoOpUnderThreshold(t *testing.T) {
	cfg := defaultPruningConfig()
	messages := []session.Message{
		{Role: "user", Content: "hello"},
		{Role: "assistant", Content: "hi there"},
	}
	result := pruneContext(messages, cfg)
	if len(result) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(result))
	}
	if result[0].Content != "hello" || result[1].Content != "hi there" {
		t.Fatal("message content should be unchanged")
	}
}

func TestPruneContext_SoftTrimLargeResults(t *testing.T) {
	// Use a small context budget so we trigger soft trim easily
	cfg := defaultPruningConfig()
	cfg.ContextTokens = 1000 // 1000 * 4 = 4000 char budget, soft at 1200
	cfg.KeepLastAssistant = 1
	cfg.SoftTrimMaxChars = 100
	cfg.SoftTrimHead = 20
	cfg.SoftTrimTail = 20

	bigContent := strings.Repeat("x", 5000)

	messages := []session.Message{
		// Old assistant with tool call (NOT protected — only last 1 assistant is protected)
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(session.ToolCall{
				ID:    "tc1",
				Name:  "web",
				Input: json.RawMessage(`{"action":"navigate","url":"example.com"}`),
			}),
		},
		// Old tool result (big, should be trimmed)
		{
			Role: "tool",
			ToolResults: makeToolResults(session.ToolResult{
				ToolCallID: "tc1",
				Content:    bigContent,
			}),
		},
		// Recent user message
		{Role: "user", Content: "what did you find?"},
		// Recent assistant (protected — last 1)
		{Role: "assistant", Content: "Here are the details"},
	}

	result := pruneContext(messages, cfg)

	// The tool result at index 1 should be soft-trimmed
	var results []session.ToolResult
	if err := json.Unmarshal(result[1].ToolResults, &results); err != nil {
		t.Fatalf("failed to unmarshal tool results: %v", err)
	}

	if len(results[0].Content) >= len(bigContent) {
		t.Fatalf("tool result should have been trimmed, still has %d chars", len(results[0].Content))
	}

	// Should contain the header
	if !strings.Contains(results[0].Content, "web(action: navigate") {
		t.Fatal("trimmed result should contain tool call summary")
	}
	if !strings.Contains(results[0].Content, "succeeded") {
		t.Fatal("trimmed result should contain status")
	}
	// Should contain head and tail marker
	if !strings.Contains(results[0].Content, "...") {
		t.Fatal("trimmed result should contain ... separator")
	}
}

func TestPruneContext_HardClearAfterSoftTrim(t *testing.T) {
	// Budget is very tiny. After soft trim (head=500, tail=500), each result is ~1100 chars.
	// With 3 results that's ~3300 chars, still over hard threshold of 200.
	// Hard clear replaces them with ~50-char placeholders.
	cfg := defaultPruningConfig()
	cfg.ContextTokens = 100 // 100 * 4 = 400 char budget, soft at 120, hard at 200
	cfg.KeepLastAssistant = 1
	cfg.SoftTrimMaxChars = 100
	cfg.SoftTrimHead = 500
	cfg.SoftTrimTail = 500

	bigContent := strings.Repeat("y", 3000)

	messages := []session.Message{
		// Old assistant with 3 tool calls (not protected)
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(
				session.ToolCall{ID: "tc1", Name: "file", Input: json.RawMessage(`{"action":"read","path":"/tmp/a.txt"}`)},
				session.ToolCall{ID: "tc2", Name: "file", Input: json.RawMessage(`{"action":"read","path":"/tmp/b.txt"}`)},
				session.ToolCall{ID: "tc3", Name: "file", Input: json.RawMessage(`{"action":"read","path":"/tmp/c.txt"}`)},
			),
		},
		// 3 big tool results
		{
			Role: "tool",
			ToolResults: makeToolResults(
				session.ToolResult{ToolCallID: "tc1", Content: bigContent},
				session.ToolResult{ToolCallID: "tc2", Content: bigContent},
				session.ToolResult{ToolCallID: "tc3", Content: bigContent},
			),
		},
		{Role: "user", Content: "ok"},
		// Recent assistant (protected)
		{Role: "assistant", Content: "done"},
	}

	result := pruneContext(messages, cfg)

	var results []session.ToolResult
	if err := json.Unmarshal(result[1].ToolResults, &results); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	// All 3 should be hard-cleared
	for i, r := range results {
		if !strings.Contains(r.Content, "[Old tool result cleared]") {
			t.Fatalf("result %d: expected hard clear placeholder, got: %s", i, r.Content)
		}
		if !strings.Contains(r.Content, "file(action: read") {
			t.Fatalf("result %d: should contain tool call summary", i)
		}
	}
}

func TestPruneContext_ProtectsRecentAssistant(t *testing.T) {
	cfg := defaultPruningConfig()
	cfg.ContextTokens = 2000 // small budget
	cfg.KeepLastAssistant = 1
	cfg.SoftTrimMaxChars = 100
	cfg.SoftTrimHead = 20
	cfg.SoftTrimTail = 20

	bigContent := strings.Repeat("z", 5000)

	messages := []session.Message{
		// Old assistant + tool result (should be trimmed)
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(session.ToolCall{
				ID:    "tc_old",
				Name:  "web",
				Input: json.RawMessage(`{"action":"fetch","url":"old.com"}`),
			}),
		},
		{
			Role: "tool",
			ToolResults: makeToolResults(session.ToolResult{
				ToolCallID: "tc_old",
				Content:    bigContent,
			}),
		},
		// Recent (protected) assistant + tool result
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(session.ToolCall{
				ID:    "tc_new",
				Name:  "web",
				Input: json.RawMessage(`{"action":"fetch","url":"new.com"}`),
			}),
		},
		{
			Role: "tool",
			ToolResults: makeToolResults(session.ToolResult{
				ToolCallID: "tc_new",
				Content:    bigContent,
			}),
		},
	}

	result := pruneContext(messages, cfg)

	// Old tool result (index 1) should be modified
	var oldResults []session.ToolResult
	json.Unmarshal(result[1].ToolResults, &oldResults)
	if len(oldResults[0].Content) >= len(bigContent) {
		t.Fatal("old tool result should have been pruned")
	}

	// New tool result (index 3) should be preserved
	var newResults []session.ToolResult
	json.Unmarshal(result[3].ToolResults, &newResults)
	if newResults[0].Content != bigContent {
		t.Fatal("protected tool result should be unchanged")
	}
}

func TestBuildToolCallIndex(t *testing.T) {
	messages := []session.Message{
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(
				session.ToolCall{
					ID:    "tc1",
					Name:  "web",
					Input: json.RawMessage(`{"action":"navigate","url":"example.com","profile":"chrome"}`),
				},
				session.ToolCall{
					ID:    "tc2",
					Name:  "file",
					Input: json.RawMessage(`{"action":"read","path":"/tmp/test.txt"}`),
				},
			),
		},
	}

	index := buildToolCallIndex(messages)

	if info, ok := index["tc1"]; !ok {
		t.Fatal("missing tc1 in index")
	} else {
		if !strings.Contains(info, "web") {
			t.Fatalf("expected web in summary, got: %s", info)
		}
		if !strings.Contains(info, "navigate") {
			t.Fatalf("expected navigate in summary, got: %s", info)
		}
		if !strings.Contains(info, "example.com") {
			t.Fatalf("expected url in summary, got: %s", info)
		}
		if !strings.Contains(info, "chrome") {
			t.Fatalf("expected profile in summary, got: %s", info)
		}
	}

	if info, ok := index["tc2"]; !ok {
		t.Fatal("missing tc2 in index")
	} else {
		if !strings.Contains(info, "file") || !strings.Contains(info, "read") {
			t.Fatalf("expected file/read in summary, got: %s", info)
		}
	}
}

func TestEstimateMessageChars(t *testing.T) {
	msg := &session.Message{
		Content:     "hello",
		ToolCalls:   json.RawMessage(`[{"id":"tc1","name":"test"}]`),
		ToolResults: json.RawMessage(`[{"tool_call_id":"tc1","content":"result"}]`),
	}
	chars := estimateMessageChars(msg)
	expected := len(msg.Content) + len(msg.ToolCalls) + len(msg.ToolResults)
	if chars != expected {
		t.Fatalf("expected %d chars, got %d", expected, chars)
	}
}

func TestIdentifyProtected(t *testing.T) {
	messages := []session.Message{
		{Role: "user", Content: "first"},           // 0 — not protected
		{Role: "assistant", Content: "response 1"},  // 1 — not protected
		{Role: "user", Content: "second"},           // 2 — not protected
		{Role: "assistant", Content: "response 2"},  // 3 — not protected
		{Role: "user", Content: "third"},            // 4 — protected (between 2nd-last and last assistant)
		{Role: "assistant", Content: "response 3"},  // 5 — protected (2nd-last assistant)
		{Role: "user", Content: "fourth"},           // 6 — protected
		{Role: "assistant", Content: "response 4"},  // 7 — protected (last assistant)
	}

	protected := identifyProtected(messages, 2)

	// Everything from index 5 (2nd-last assistant) onward is protected
	for i := 5; i <= 7; i++ {
		if !protected[i] {
			t.Fatalf("message at index %d should be protected", i)
		}
	}

	// Earlier messages should NOT be protected
	for i := 0; i <= 4; i++ {
		if protected[i] {
			t.Fatalf("message at index %d should not be protected", i)
		}
	}
}

func TestPruneContext_EmptyMessages(t *testing.T) {
	cfg := defaultPruningConfig()
	result := pruneContext(nil, cfg)
	if result != nil {
		t.Fatal("nil input should return nil")
	}

	result = pruneContext([]session.Message{}, cfg)
	if len(result) != 0 {
		t.Fatal("empty input should return empty")
	}
}

func TestPruneContext_ErrorResultPreservesIsError(t *testing.T) {
	cfg := defaultPruningConfig()
	cfg.ContextTokens = 1000
	cfg.KeepLastAssistant = 1
	cfg.SoftTrimMaxChars = 50
	cfg.SoftTrimHead = 10
	cfg.SoftTrimTail = 10

	messages := []session.Message{
		// Old assistant (not protected)
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(session.ToolCall{
				ID:    "tc1",
				Name:  "shell",
				Input: json.RawMessage(`{"action":"exec","command":"failing"}`),
			}),
		},
		{
			Role: "tool",
			ToolResults: makeToolResults(session.ToolResult{
				ToolCallID: "tc1",
				Content:    strings.Repeat("error: ", 500),
				IsError:    true,
			}),
		},
		{Role: "user", Content: "ok"},
		// Recent assistant (protected)
		{Role: "assistant", Content: "sorry about that"},
	}

	result := pruneContext(messages, cfg)

	var results []session.ToolResult
	json.Unmarshal(result[1].ToolResults, &results)

	if !results[0].IsError {
		t.Fatal("IsError should be preserved after pruning")
	}
	if !strings.Contains(results[0].Content, "failed") {
		t.Fatal("error result should show 'failed' status")
	}
}
