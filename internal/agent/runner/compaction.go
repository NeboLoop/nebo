package runner

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/nebolabs/nebo/internal/agent/session"
)

// ToolFailure represents a failed tool execution to preserve in compaction summary
type ToolFailure struct {
	ToolCallID string `json:"tool_call_id"`
	ToolName   string `json:"tool_name"`
	Summary    string `json:"summary"`
	Meta       string `json:"meta,omitempty"` // status=failed exitCode=1
}

const (
	// MaxToolFailures caps the number of failures included in summary
	MaxToolFailures = 8
	// MaxToolFailureChars truncates individual failure messages
	MaxToolFailureChars = 240
)

// CollectToolFailures extracts failed tool results from messages.
// Deduplicates by tool_call_id to avoid repeating the same failure.
// (moltbot pattern: compaction-safeguard.ts)
func CollectToolFailures(messages []session.Message) []ToolFailure {
	var failures []ToolFailure
	seen := make(map[string]bool)

	for _, msg := range messages {
		// Only look at tool result messages
		if msg.Role != "tool" || len(msg.ToolResults) == 0 {
			continue
		}

		var results []session.ToolResult
		if err := json.Unmarshal(msg.ToolResults, &results); err != nil {
			continue
		}

		for _, r := range results {
			// Only collect errors
			if !r.IsError {
				continue
			}

			// Dedupe by tool_call_id
			if r.ToolCallID == "" || seen[r.ToolCallID] {
				continue
			}
			seen[r.ToolCallID] = true

			// Extract tool name from the preceding assistant message's tool_calls
			toolName := extractToolName(messages, r.ToolCallID)
			if toolName == "" {
				toolName = "tool"
			}

			// Normalize and truncate the content
			summary := normalizeFailureText(r.Content)
			if summary == "" {
				summary = "failed (no output)"
			}
			summary = truncateText(summary, MaxToolFailureChars)

			// Extract metadata (status, exit code) if embedded in content
			meta := extractFailureMeta(r.Content)

			failures = append(failures, ToolFailure{
				ToolCallID: r.ToolCallID,
				ToolName:   toolName,
				Summary:    summary,
				Meta:       meta,
			})
		}
	}

	return failures
}

// FormatToolFailuresSection formats failures for inclusion in compaction summary.
// Returns empty string if no failures.
func FormatToolFailuresSection(failures []ToolFailure) string {
	if len(failures) == 0 {
		return ""
	}

	var sb strings.Builder
	sb.WriteString("\n\n## Tool Failures\n")

	// Cap at MaxToolFailures
	displayCount := min(len(failures), MaxToolFailures)

	for i := 0; i < displayCount; i++ {
		f := failures[i]
		if f.Meta != "" {
			sb.WriteString(fmt.Sprintf("- %s (%s): %s\n", f.ToolName, f.Meta, f.Summary))
		} else {
			sb.WriteString(fmt.Sprintf("- %s: %s\n", f.ToolName, f.Summary))
		}
	}

	// Add overflow indicator
	if len(failures) > MaxToolFailures {
		sb.WriteString(fmt.Sprintf("- ...and %d more\n", len(failures)-MaxToolFailures))
	}

	return sb.String()
}

// extractToolName finds the tool name from the tool_calls that matches the given call ID
func extractToolName(messages []session.Message, toolCallID string) string {
	for _, msg := range messages {
		if msg.Role != "assistant" || len(msg.ToolCalls) == 0 {
			continue
		}

		var calls []session.ToolCall
		if err := json.Unmarshal(msg.ToolCalls, &calls); err != nil {
			continue
		}

		for _, call := range calls {
			if call.ID == toolCallID {
				return call.Name
			}
		}
	}
	return ""
}

// normalizeFailureText cleans up whitespace in error text
func normalizeFailureText(text string) string {
	// Replace multiple whitespace with single space, trim
	var sb strings.Builder
	lastWasSpace := true
	for _, r := range text {
		if r == ' ' || r == '\n' || r == '\r' || r == '\t' {
			if !lastWasSpace {
				sb.WriteRune(' ')
				lastWasSpace = true
			}
		} else {
			sb.WriteRune(r)
			lastWasSpace = false
		}
	}
	return strings.TrimSpace(sb.String())
}

// truncateText limits text to maxChars with ellipsis
func truncateText(text string, maxChars int) string {
	if len(text) <= maxChars {
		return text
	}
	if maxChars <= 3 {
		return text[:maxChars]
	}
	return text[:maxChars-3] + "..."
}

// extractFailureMeta extracts status and exit code from bash errors
func extractFailureMeta(content string) string {
	var parts []string

	// Look for common patterns
	lower := strings.ToLower(content)

	// Exit code patterns
	if idx := strings.Index(lower, "exit code"); idx >= 0 {
		// Try to extract number after "exit code"
		rest := content[idx+9:]
		for _, r := range rest {
			if r >= '0' && r <= '9' {
				code := extractNumber(rest)
				if code != "" {
					parts = append(parts, "exitCode="+code)
				}
				break
			} else if r != ' ' && r != ':' && r != '=' {
				break
			}
		}
	} else if idx := strings.Index(lower, "exited with code"); idx >= 0 {
		rest := content[idx+16:]
		code := extractNumber(rest)
		if code != "" {
			parts = append(parts, "exitCode="+code)
		}
	}

	// Status patterns
	if strings.Contains(lower, "command timed out") {
		parts = append(parts, "status=timeout")
	} else if strings.Contains(lower, "permission denied") {
		parts = append(parts, "status=permission_denied")
	} else if strings.Contains(lower, "not found") || strings.Contains(lower, "enoent") {
		parts = append(parts, "status=not_found")
	}

	return strings.Join(parts, " ")
}

// extractNumber extracts the first number from a string
func extractNumber(s string) string {
	var sb strings.Builder
	started := false
	for _, r := range s {
		if r >= '0' && r <= '9' {
			sb.WriteRune(r)
			started = true
		} else if started {
			break
		}
	}
	return sb.String()
}

// EnhancedSummary adds tool failures to a compaction summary
func EnhancedSummary(messages []session.Message, baseSummary string) string {
	failures := CollectToolFailures(messages)
	failureSection := FormatToolFailuresSection(failures)

	if failureSection == "" {
		return baseSummary
	}

	return baseSummary + failureSection
}
