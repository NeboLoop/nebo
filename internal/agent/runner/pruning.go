package runner

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"

	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/session"
)

// Token estimation constants
const (
	CharsPerTokenEstimate = 4
	ImageCharEstimate     = 8000 // chars — rough estimate for base64 images
)

// Micro-compact constants
const (
	MicroCompactMinSavings = 5000 // tokens — skip if total savings below this
	MicroCompactKeepRecent = 3     // protect the N most recent individual tool results
	ImageTokenEstimate     = 2000  // tokens per image block
)

// microCompactTools lists tools whose results are candidates for micro-compaction.
// These tools produce the largest outputs (file reads, shell output, web content).
var microCompactTools = map[string]bool{
	"system": true, // file (read, grep, glob, edit) + shell (exec)
	"web":    true, // fetch, search
	"file":   true, // legacy name (pre-STRAP sessions)
	"shell":  true, // legacy name (pre-STRAP sessions)
}

// trimPriority returns a priority value for a tool type (lower = trim first).
func trimPriority(toolSummary string) int {
	if strings.HasPrefix(toolSummary, "system(resource: file, action: read") ||
		strings.HasPrefix(toolSummary, "file(action: read") {
		return 0 // File reads produce the largest output
	}
	if strings.HasPrefix(toolSummary, "system(resource: shell") ||
		strings.HasPrefix(toolSummary, "shell(") {
		return 1 // Shell output is often large
	}
	if strings.HasPrefix(toolSummary, "web(") {
		return 2 // Web content is moderate
	}
	return 3 // Other tools
}

// microCompact silently trims old tool results in-place before every API call.
// It also strips images from messages that have already been acknowledged by the model.
//
// Unlike pruneContext (which is threshold-gated), this runs every iteration but
// protects the most recent 3 individual tool results to preserve working context.
//
// Two modes:
//   - Above warning threshold: trims all eligible candidates (original behavior).
//   - Below warning threshold: proactively trims only old candidates (>8 messages
//     from the end) with a lower savings floor, so the first compaction-under-pressure
//     is faster.
func microCompact(messages []session.Message, warningThreshold int) ([]session.Message, int) {
	if len(messages) == 0 {
		return messages, 0
	}

	estimatedTokens := 0
	for i := range messages {
		estimatedTokens += estimateMessageChars(&messages[i]) / CharsPerTokenEstimate
	}

	aboveWarning := estimatedTokens >= warningThreshold

	// Step 1: Find all tool_use/tool_result pairs for compactable tools.
	// Track tool call IDs from assistant messages and their result sizes.
	type candidate struct {
		toolCallID  string
		resultMsgIdx int
		resultIdx    int // index within the ToolResults array
		tokenSize    int
		toolSummary  string // e.g., "file(action: read)"
	}

	var candidates []candidate
	toolCallIndex := buildToolCallIndex(messages)

	for i, msg := range messages {
		if len(msg.ToolResults) == 0 {
			continue
		}
		var results []session.ToolResult
		if err := json.Unmarshal(msg.ToolResults, &results); err != nil {
			continue
		}
		for j, tr := range results {
			// Check if the originating tool call is from a compactable tool
			summary, ok := toolCallIndex[tr.ToolCallID]
			if !ok {
				continue
			}
			toolName := strings.SplitN(summary, "(", 2)[0]
			if !microCompactTools[toolName] {
				continue
			}
			// Skip already-trimmed results
			if strings.HasPrefix(tr.Content, "[trimmed:") {
				continue
			}
			tokenSize := len(tr.Content) / CharsPerTokenEstimate
			if tokenSize < 10 {
				continue // tiny results aren't worth tracking
			}
			candidates = append(candidates, candidate{
				toolCallID:   tr.ToolCallID,
				resultMsgIdx: i,
				resultIdx:    j,
				tokenSize:    tokenSize,
				toolSummary:  summary,
			})
		}
	}

	// Below warning: only trim candidates older than 8 messages from the end
	if !aboveWarning {
		const proactiveTrimAge = 8
		var oldCandidates []candidate
		for _, c := range candidates {
			if len(messages)-c.resultMsgIdx > proactiveTrimAge {
				oldCandidates = append(oldCandidates, c)
			}
		}
		candidates = oldCandidates
	}

	// Sort candidates by trim priority (largest output producers first), then age
	sort.Slice(candidates, func(i, j int) bool {
		pi, pj := trimPriority(candidates[i].toolSummary), trimPriority(candidates[j].toolSummary)
		if pi != pj {
			return pi < pj // Lower priority number = trim first
		}
		return candidates[i].resultMsgIdx < candidates[j].resultMsgIdx // Older first
	})

	// Step 2: Protect the most recent N tool results and calculate savings
	protectedIDs := make(map[string]bool)
	toTrim := make(map[string]string) // toolCallID → summary
	totalSavings := 0

	if len(candidates) > 0 {
		start := len(candidates) - MicroCompactKeepRecent
		if start < 0 {
			start = 0
		}
		for _, c := range candidates[start:] {
			protectedIDs[c.toolCallID] = true
		}

		// Step 3: Calculate potential savings
		for _, c := range candidates {
			if protectedIDs[c.toolCallID] {
				continue
			}
			toTrim[c.toolCallID] = c.toolSummary
			totalSavings += c.tokenSize
		}

		minSavings := MicroCompactMinSavings
		if !aboveWarning {
			minSavings = 2000 // Lower floor for proactive trimming
		}
		if totalSavings < minSavings {
			toTrim = nil // not worth it
			totalSavings = 0
		}
	}

	// Step 4: Image stripping — find user messages with images that have been
	// acknowledged (an assistant message followed them)
	acknowledgedMsgIdx := make(map[int]bool)
	imageTokensSaved := 0
	{
		var pendingUserIdxs []int
		for i, msg := range messages {
			if msg.Role == "user" {
				pendingUserIdxs = append(pendingUserIdxs, i)
			} else if msg.Role == "assistant" && len(pendingUserIdxs) > 0 {
				for _, idx := range pendingUserIdxs {
					acknowledgedMsgIdx[idx] = true
				}
				pendingUserIdxs = nil
			}
		}
	}

	// Step 5: Build result with trimmed content
	result := make([]session.Message, len(messages))
	copy(result, messages)

	trimCount := 0

	// Trim tool results
	if len(toTrim) > 0 {
		for i := range result {
			if len(result[i].ToolResults) == 0 {
				continue
			}
			var results []session.ToolResult
			if err := json.Unmarshal(result[i].ToolResults, &results); err != nil {
				continue
			}
			changed := false
			for j := range results {
				summary, ok := toTrim[results[j].ToolCallID]
				if !ok {
					continue
				}
				changed = true
				trimCount++
				results[j].Content = fmt.Sprintf("[trimmed: %s]", summary)
			}
			if changed {
				if data, err := json.Marshal(results); err == nil {
					result[i].ToolResults = data
				}
			}
		}

		// Also trim the tool_use inputs in assistant messages
		for i := range result {
			if result[i].Role != "assistant" || len(result[i].ToolCalls) == 0 {
				continue
			}
			var calls []session.ToolCall
			if err := json.Unmarshal(result[i].ToolCalls, &calls); err != nil {
				continue
			}
			changed := false
			for j := range calls {
				if _, ok := toTrim[calls[j].ID]; ok {
					changed = true
					calls[j].Input = json.RawMessage(`{"trimmed":true}`)
				}
			}
			if changed {
				if data, err := json.Marshal(calls); err == nil {
					result[i].ToolCalls = data
				}
			}
		}
	}

	// Strip acknowledged images
	for i := range result {
		if result[i].Role != "user" || !acknowledgedMsgIdx[i] {
			continue
		}
		// Check for base64 image content in the message
		if strings.Contains(result[i].Content, "data:image/") {
			oldLen := len(result[i].Content)
			result[i].Content = "[image]"
			imageTokensSaved += (oldLen - 7) / CharsPerTokenEstimate
		}
	}

	saved := totalSavings + imageTokensSaved
	if trimCount > 0 || imageTokensSaved > 0 {
		fmt.Printf("[Runner] Micro-compacted: saved ~%d tokens (%d tool results trimmed, %d image tokens stripped)\n",
			saved, trimCount, imageTokensSaved)
	}

	return result, saved
}

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
