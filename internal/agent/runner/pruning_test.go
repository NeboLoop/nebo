package runner

import (
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/session"
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

// --- Micro-Compact Tests ---

// buildMicroCompactMessages creates a message history with tool calls for testing.
// Each call generates ~tokensPerResult tokens of tool result content.
func buildMicroCompactMessages(numCalls int, tokensPerResult int) []session.Message {
	messages := make([]session.Message, 0, numCalls*3+2)
	messages = append(messages, session.Message{Role: "user", Content: "do something"})

	for i := 0; i < numCalls; i++ {
		tcID := "tc" + strings.Repeat("0", i+1)
		messages = append(messages, session.Message{
			Role: "assistant",
			ToolCalls: makeToolCalls(session.ToolCall{
				ID:    tcID,
				Name:  "file",
				Input: json.RawMessage(`{"action":"read","path":"/tmp/test.go"}`),
			}),
		})
		messages = append(messages, session.Message{
			Role: "tool",
			ToolResults: makeToolResults(session.ToolResult{
				ToolCallID: tcID,
				Content:    strings.Repeat("x", tokensPerResult*CharsPerTokenEstimate),
			}),
		})
	}

	messages = append(messages, session.Message{Role: "assistant", Content: "done"})
	return messages
}

func TestMicroCompact_ProtectsLast3Results(t *testing.T) {
	// 6 tool calls with 10k tokens each = 60k tokens (above any threshold)
	// The last 3 should be protected
	msgs := buildMicroCompactMessages(6, 10000)
	result, saved := microCompact(msgs, 1000) // very low threshold so it always fires

	if saved == 0 {
		t.Fatal("expected some savings")
	}

	// Count how many results are trimmed vs preserved
	trimmed, preserved := 0, 0
	for _, msg := range result {
		if len(msg.ToolResults) == 0 {
			continue
		}
		var results []session.ToolResult
		json.Unmarshal(msg.ToolResults, &results)
		for _, r := range results {
			if strings.HasPrefix(r.Content, "[trimmed:") {
				trimmed++
			} else {
				preserved++
			}
		}
	}

	if preserved != MicroCompactKeepRecent {
		t.Fatalf("expected %d preserved results, got %d", MicroCompactKeepRecent, preserved)
	}
	if trimmed != 3 { // 6 total - 3 protected = 3 trimmed
		t.Fatalf("expected 3 trimmed results, got %d", trimmed)
	}
}

func TestMicroCompact_SkipsBelowWarningThreshold(t *testing.T) {
	// Small messages that are well below the warning threshold
	msgs := []session.Message{
		{Role: "user", Content: "hello"},
		{Role: "assistant", Content: "hi"},
	}

	result, saved := microCompact(msgs, 100000) // high threshold
	if saved != 0 {
		t.Fatalf("expected 0 savings below threshold, got %d", saved)
	}
	if len(result) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(result))
	}
}

func TestMicroCompact_RequiresMinSavings(t *testing.T) {
	// 4 tool calls with 1000 tokens each = 4000 tokens total
	// After protecting 3, only 1 would be trimmed (1000 tokens) < MicroCompactMinSavings (20000)
	msgs := buildMicroCompactMessages(4, 1000)

	_, saved := microCompact(msgs, 1) // threshold = 1 so it tries to run
	if saved != 0 {
		t.Fatalf("expected 0 savings when below min savings, got %d", saved)
	}
}

func TestMicroCompact_TrimsToolUseAndResult(t *testing.T) {
	msgs := buildMicroCompactMessages(6, 10000)
	result, _ := microCompact(msgs, 1000)

	// Check that trimmed tool results also have their tool_use inputs trimmed
	for _, msg := range result {
		if msg.Role != "assistant" || len(msg.ToolCalls) == 0 {
			continue
		}
		var calls []session.ToolCall
		json.Unmarshal(msg.ToolCalls, &calls)
		for _, c := range calls {
			// Find if this call's result was trimmed
			for _, rmsg := range result {
				if len(rmsg.ToolResults) == 0 {
					continue
				}
				var results []session.ToolResult
				json.Unmarshal(rmsg.ToolResults, &results)
				for _, r := range results {
					if r.ToolCallID == c.ID && strings.HasPrefix(r.Content, "[trimmed:") {
						// The tool_use input should also be trimmed
						if !strings.Contains(string(c.Input), "trimmed") {
							t.Fatalf("tool_use input for %s should be trimmed when result is trimmed", c.ID)
						}
					}
				}
			}
		}
	}
}

func TestMicroCompact_StripAcknowledgedImages(t *testing.T) {
	msgs := []session.Message{
		{Role: "user", Content: "here is a screenshot data:image/png;base64,abc123..."},
		{Role: "assistant", Content: "I can see the image shows a dashboard"},
		{Role: "user", Content: "what about this one data:image/jpeg;base64,def456..."},
		// No assistant response after this one — image is NOT acknowledged
	}

	// Low threshold to trigger, but there are no tool results to trim
	result, saved := microCompact(msgs, 1)

	// First user message should have image stripped (it was acknowledged)
	if result[0].Content != "[image]" {
		t.Fatalf("acknowledged image should be stripped, got: %s", result[0].Content)
	}

	// Second user message should be preserved (no assistant followed it)
	if !strings.Contains(result[2].Content, "data:image/jpeg") {
		t.Fatal("unacknowledged image should be preserved")
	}

	_ = saved // image savings
}

func TestMicroCompact_SkipsNonCompactableTools(t *testing.T) {
	// Tool call from "bot" tool — should NOT be trimmed by micro-compact
	// (bot is not in microCompactTools)
	msgs := []session.Message{
		{Role: "user", Content: "remember this"},
		{
			Role: "assistant",
			ToolCalls: makeToolCalls(session.ToolCall{
				ID:    "tc_bot",
				Name:  "bot",
				Input: json.RawMessage(`{"resource":"memory","action":"store"}`),
			}),
		},
		{
			Role: "tool",
			ToolResults: makeToolResults(session.ToolResult{
				ToolCallID: "tc_bot",
				Content:    strings.Repeat("stored ok ", 10000),
			}),
		},
		{Role: "assistant", Content: "done"},
	}

	result, saved := microCompact(msgs, 1)
	if saved != 0 {
		t.Fatalf("bot tool results should not be trimmed, saved %d tokens", saved)
	}

	var results []session.ToolResult
	json.Unmarshal(result[2].ToolResults, &results)
	if strings.HasPrefix(results[0].Content, "[trimmed:") {
		t.Fatal("bot tool result should NOT be trimmed")
	}
}

// --- FileAccessTracker tests ---

func TestFileAccessTracker_TrackAndSnapshot(t *testing.T) {
	tracker := NewFileAccessTracker()

	tracker.Track("/tmp/a.go")
	tracker.Track("/tmp/b.go")
	tracker.Track("/tmp/a.go") // update timestamp

	snap := tracker.Snapshot()
	if len(snap) != 2 {
		t.Fatalf("expected 2 entries, got %d", len(snap))
	}
	if _, ok := snap["/tmp/a.go"]; !ok {
		t.Fatal("missing /tmp/a.go")
	}
	if _, ok := snap["/tmp/b.go"]; !ok {
		t.Fatal("missing /tmp/b.go")
	}
}

func TestFileAccessTracker_Clear(t *testing.T) {
	tracker := NewFileAccessTracker()
	tracker.Track("/tmp/a.go")
	tracker.Clear()

	snap := tracker.Snapshot()
	if len(snap) != 0 {
		t.Fatalf("expected 0 entries after clear, got %d", len(snap))
	}
}

func TestBuildFileReinjectionMessage_EmptyTracker(t *testing.T) {
	tracker := NewFileAccessTracker()
	msg := buildFileReinjectionMessage(tracker)
	if msg != nil {
		t.Fatal("expected nil for empty tracker")
	}
}

func TestBuildFileReinjectionMessage_NilTracker(t *testing.T) {
	msg := buildFileReinjectionMessage(nil)
	if msg != nil {
		t.Fatal("expected nil for nil tracker")
	}
}

func TestBuildFileReinjectionMessage_WithRealFiles(t *testing.T) {
	// Create temp files
	dir := t.TempDir()
	for i := 0; i < 3; i++ {
		path := dir + "/" + string(rune('a'+i)) + ".txt"
		os.WriteFile(path, []byte("line 1\nline 2\nline 3\n"), 0644)
	}

	tracker := NewFileAccessTracker()
	for i := 0; i < 3; i++ {
		path := dir + "/" + string(rune('a'+i)) + ".txt"
		tracker.Track(path)
	}

	msg := buildFileReinjectionMessage(tracker)
	if msg == nil {
		t.Fatal("expected non-nil message")
	}
	if msg.Role != "user" {
		t.Errorf("expected role 'user', got %q", msg.Role)
	}
	if !strings.Contains(msg.Content, "[Context recovery") {
		t.Error("missing context recovery header")
	}
	if !strings.Contains(msg.Content, "a.txt") {
		t.Error("missing file a.txt in re-injection")
	}
}

func TestBuildFileReinjectionMessage_MaxFiles(t *testing.T) {
	// Create more files than MaxReinjectedFiles
	dir := t.TempDir()
	tracker := NewFileAccessTracker()

	for i := 0; i < MaxReinjectedFiles+3; i++ {
		path := dir + "/" + string(rune('a'+i)) + ".txt"
		os.WriteFile(path, []byte("content\n"), 0644)
		tracker.Track(path)
	}

	msg := buildFileReinjectionMessage(tracker)
	if msg == nil {
		t.Fatal("expected non-nil message")
	}

	// Count "===" markers (one per file)
	count := strings.Count(msg.Content, "===")
	// Each file has "=== path ===" so 2 per file
	filesIncluded := count / 2
	if filesIncluded > MaxReinjectedFiles {
		t.Errorf("expected at most %d files, got %d", MaxReinjectedFiles, filesIncluded)
	}
}

func TestBuildFileReinjectionMessage_MissingFileSkipped(t *testing.T) {
	tracker := NewFileAccessTracker()
	tracker.Track("/nonexistent/path/to/file.go")

	msg := buildFileReinjectionMessage(tracker)
	if msg != nil {
		t.Fatal("expected nil when all files are unreadable")
	}
}
