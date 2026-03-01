package runner

import (
	"strings"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/session"
)

// Tool groups for adjacency-based inclusion.
// If any tool in a group was called in the session, include the entire group.
var toolGroups = [][]string{
	{"screenshot", "vision", "desktop"},
	{"organizer"},
	{"system"},
}

// contextualKeywords maps tool names to keyword triggers.
// Tools not listed here are always included (core tools).
var contextualKeywords = map[string][]string{
	"screenshot": {"screenshot", "screen", "image", "look at", "see the", "show me"},
	"vision":     {"screenshot", "screen", "image", "look at", "see the", "photo"},
	"desktop":    {"click", "type", "window", "automat", "launch", "open app"},
	"organizer":  {"email", "calendar", "contact", "reminder", "meeting", "schedule"},
	"system":     {"volume", "clipboard", "notification", "music", "battery", "wifi"},
}

// FilterTools selects which tools to include in the API request.
// Core tools (file, shell, web, agent, skill) are always included.
// Contextual tools are included when recent messages mention relevant keywords
// or when any tool in their group was recently called.
func FilterTools(allTools []ai.ToolDefinition, messages []session.Message, calledTools map[string]bool) []ai.ToolDefinition {
	// Build context string from recent messages (last 3 user + assistant messages)
	contextText := buildRecentContext(messages, 3)
	lower := strings.ToLower(contextText)

	// Determine which contextual tools are triggered
	triggered := make(map[string]bool)

	// Keyword matching
	for toolName, keywords := range contextualKeywords {
		for _, kw := range keywords {
			if strings.Contains(lower, kw) {
				triggered[toolName] = true
				break
			}
		}
	}

	// Group adjacency: if any tool in a group was called, include the entire group
	for _, group := range toolGroups {
		groupActive := false
		for _, name := range group {
			if calledTools[name] || triggered[name] {
				groupActive = true
				break
			}
		}
		if groupActive {
			for _, name := range group {
				triggered[name] = true
			}
		}
	}

	// Filter tools
	var result []ai.ToolDefinition
	for _, tool := range allTools {
		if isCoreTool(tool.Name) || triggered[tool.Name] {
			result = append(result, tool)
		}
	}

	// Safety: never return an empty tool list
	if len(result) == 0 {
		return allTools
	}

	return result
}

// isCoreTool returns true for tools that should always be included.
func isCoreTool(name string) bool {
	switch name {
	case "system", "web", "bot", "loop", "event", "message", "skill":
		return true
	default:
		return false
	}
}

// buildRecentContext extracts text from the N most recent user and assistant messages.
func buildRecentContext(messages []session.Message, n int) string {
	var parts []string
	count := 0
	for i := len(messages) - 1; i >= 0 && count < n; i-- {
		if messages[i].Role == "user" || messages[i].Role == "assistant" {
			if messages[i].Content != "" {
				parts = append(parts, messages[i].Content)
			}
			count++
		}
	}
	return strings.Join(parts, " ")
}
