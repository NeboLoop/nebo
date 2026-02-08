package runner

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/session"
)

// Token estimation constants
const (
	CharsPerTokenEstimate = 4
	ImageCharEstimate     = 8000
)

// pruneContext applies two-stage context pruning to reduce token usage while
// preserving the most recent conversation turns in full.
//
// Stage 1 (soft trim): when estimated chars exceed SoftTrimRatio of the char budget,
// trim unprotected tool results to head + "..." + tail.
//
// Stage 2 (hard clear): when estimated chars still exceed HardClearRatio,
// replace unprotected tool results entirely with a short placeholder.
func pruneContext(messages []session.Message, cfg config.ContextPruningConfig) []session.Message {
	if len(messages) == 0 {
		return messages
	}

	charBudget := cfg.ContextTokens * CharsPerTokenEstimate

	// Estimate total chars
	totalChars := 0
	for i := range messages {
		totalChars += estimateMessageChars(&messages[i])
	}

	softThreshold := int(float64(charBudget) * cfg.SoftTrimRatio)
	hardThreshold := int(float64(charBudget) * cfg.HardClearRatio)

	// Nothing to do if under soft threshold
	if totalChars <= softThreshold {
		return messages
	}

	// Build tool call index for summary headers
	toolCallIndex := buildToolCallIndex(messages)

	// Identify protected message indices
	protected := identifyProtected(messages, cfg.KeepLastAssistant)

	// Stage 1: soft trim
	result := make([]session.Message, len(messages))
	copy(result, messages)

	softCount := 0
	result, softCount, totalChars = softTrimToolResults(result, protected, toolCallIndex, totalChars, cfg)
	if softCount > 0 {
		fmt.Printf("[Runner] Soft-trimmed %d tool results (total chars: %d, budget: %d)\n",
			softCount, totalChars, charBudget)
	}

	// Stage 2: hard clear (only if still over threshold)
	if totalChars > hardThreshold {
		hardCount := 0
		result, hardCount, totalChars = hardClearToolResults(result, protected, toolCallIndex, totalChars, cfg)
		if hardCount > 0 {
			fmt.Printf("[Runner] Hard-cleared %d tool results (total chars: %d, budget: %d)\n",
				hardCount, totalChars, charBudget)
		}
	}

	return result
}

// identifyProtected returns a set of message indices that should NOT be pruned.
// Protected messages are:
//  1. The last N assistant messages
//  2. Tool result messages whose ToolCallID matches a tool call from a protected assistant message
//  3. User messages within the last N assistant turns
func identifyProtected(messages []session.Message, keepLastAssistant int) map[int]bool {
	protected := make(map[int]bool)

	// Walk backwards to find the Nth assistant message from the end.
	// Everything from that point forward is protected — this covers the
	// assistant messages themselves, their tool results, and interleaved user messages.
	assistantCount := 0
	cutoff := len(messages) // nothing protected yet

	for i := len(messages) - 1; i >= 0; i-- {
		if messages[i].Role == "assistant" {
			assistantCount++
			cutoff = i
			if assistantCount >= keepLastAssistant {
				break
			}
		}
	}

	// Protect everything from cutoff to end
	for i := cutoff; i < len(messages); i++ {
		protected[i] = true
	}

	return protected
}

// softTrimToolResults trims unprotected tool results that exceed SoftTrimMaxChars
// to keep head + "\n...\n" + tail, preserving both beginning and end of the result.
func softTrimToolResults(
	messages []session.Message,
	protected map[int]bool,
	toolCallIndex map[string]string,
	totalChars int,
	cfg config.ContextPruningConfig,
) ([]session.Message, int, int) {
	trimmed := 0

	for i := range messages {
		if protected[i] {
			continue
		}
		if len(messages[i].ToolResults) == 0 {
			continue
		}

		var results []session.ToolResult
		if err := json.Unmarshal(messages[i].ToolResults, &results); err != nil {
			continue
		}

		changed := false
		for j := range results {
			if len(results[j].Content) <= cfg.SoftTrimMaxChars {
				continue
			}

			oldLen := len(results[j].Content)

			status := "succeeded"
			if results[j].IsError {
				status = "failed"
			}
			header := "[" + status + "]"
			if info, ok := toolCallIndex[results[j].ToolCallID]; ok {
				header = "[" + info + " — " + status + "]"
			}

			head := results[j].Content[:cfg.SoftTrimHead]
			tail := results[j].Content[oldLen-cfg.SoftTrimTail:]
			results[j].Content = header + "\n" + head + "\n...\n" + tail

			totalChars -= oldLen - len(results[j].Content)
			trimmed++
			changed = true
		}

		if changed {
			if newData, err := json.Marshal(results); err == nil {
				messages[i].ToolResults = newData
			}
		}
	}

	return messages, trimmed, totalChars
}

// hardClearToolResults replaces unprotected tool results entirely with a short
// summary header + placeholder, keeping only the tool call metadata.
func hardClearToolResults(
	messages []session.Message,
	protected map[int]bool,
	toolCallIndex map[string]string,
	totalChars int,
	cfg config.ContextPruningConfig,
) ([]session.Message, int, int) {
	cleared := 0

	for i := range messages {
		if protected[i] {
			continue
		}
		if len(messages[i].ToolResults) == 0 {
			continue
		}

		var results []session.ToolResult
		if err := json.Unmarshal(messages[i].ToolResults, &results); err != nil {
			continue
		}

		changed := false
		for j := range results {
			// Skip results that are already just a placeholder
			if strings.Contains(results[j].Content, cfg.HardClearPlaceholder) {
				continue
			}
			// Skip very short results that don't need clearing
			if len(results[j].Content) <= 200 {
				continue
			}

			oldLen := len(results[j].Content)

			status := "succeeded"
			if results[j].IsError {
				status = "failed"
			}
			header := "[" + status + "]"
			if info, ok := toolCallIndex[results[j].ToolCallID]; ok {
				header = "[" + info + " — " + status + "]"
			}

			results[j].Content = header + "\n" + cfg.HardClearPlaceholder

			totalChars -= oldLen - len(results[j].Content)
			cleared++
			changed = true
		}

		if changed {
			if newData, err := json.Marshal(results); err == nil {
				messages[i].ToolResults = newData
			}
		}
	}

	return messages, cleared, totalChars
}

// estimateMessageChars estimates the character count of a message including
// content, tool calls, and tool results.
func estimateMessageChars(msg *session.Message) int {
	chars := len(msg.Content) + len(msg.ToolCalls) + len(msg.ToolResults)
	return chars
}

// buildToolCallIndex builds a map from tool call ID to a human-readable summary
// string like "web(action: navigate, url: x.com, profile: chrome)".
func buildToolCallIndex(messages []session.Message) map[string]string {
	index := make(map[string]string)

	for _, msg := range messages {
		if len(msg.ToolCalls) == 0 {
			continue
		}
		var calls []session.ToolCall
		if err := json.Unmarshal(msg.ToolCalls, &calls); err != nil {
			continue
		}
		for _, tc := range calls {
			summary := tc.Name
			var input map[string]any
			if json.Unmarshal(tc.Input, &input) == nil {
				if action, ok := input["action"].(string); ok {
					summary += "(action: " + action
					if url, ok := input["url"].(string); ok {
						summary += ", url: " + url
					}
					if profile, ok := input["profile"].(string); ok {
						summary += ", profile: " + profile
					}
					if resource, ok := input["resource"].(string); ok {
						summary += ", resource: " + resource
					}
					summary += ")"
				}
			}
			index[tc.ID] = summary
		}
	}

	return index
}
