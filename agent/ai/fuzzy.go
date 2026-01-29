package ai

import (
	"strings"
	"unicode"

	"gobot/internal/provider"
)

// FuzzyMatcher provides fuzzy matching for model names from user input
type FuzzyMatcher struct {
	config  *provider.ModelsConfig
	aliases map[string]string // lowercase alias -> full model ID
}

// NewFuzzyMatcher creates a new fuzzy matcher with the given config
func NewFuzzyMatcher(config *provider.ModelsConfig) *FuzzyMatcher {
	f := &FuzzyMatcher{
		config:  config,
		aliases: make(map[string]string),
	}
	f.buildAliases()
	return f
}

// buildAliases creates common aliases for models
func (f *FuzzyMatcher) buildAliases() {
	// Common keyword mappings (2026 models)
	keywords := map[string]string{
		// Anthropic Claude 4.5 aliases
		"claude":       "anthropic/claude-sonnet-4-5",
		"sonnet":       "anthropic/claude-sonnet-4-5",
		"opus":         "anthropic/claude-opus-4-5",
		"haiku":        "anthropic/claude-haiku-4-5",
		"claude-opus":  "anthropic/claude-opus-4-5",
		"claude-haiku": "anthropic/claude-haiku-4-5",
		"anthropic":    "anthropic/claude-sonnet-4-5",

		// OpenAI GPT-5.2 aliases (2026 - NO GPT-4!)
		"gpt":         "openai/gpt-5.2",
		"gpt5":        "openai/gpt-5.2",
		"gpt-5":       "openai/gpt-5.2",
		"gpt5.2":      "openai/gpt-5.2",
		"gpt-5.2":     "openai/gpt-5.2",
		"chatgpt":     "openai/gpt-5.2",
		"openai":      "openai/gpt-5.2",
		"gpt-instant": "openai/gpt-5.2-instant",
		"instant":     "openai/gpt-5.2-instant",
		"gpt-pro":     "openai/gpt-5.2-pro",
		"codex":       "openai/gpt-5.2-codex",
		"gpt-codex":   "openai/gpt-5.2-codex",
		"gpt-think":   "openai/gpt-5.2-thinking",

		// Google Gemini 3 aliases (2026)
		"gemini":        "google/gemini-3-flash",
		"gemini-flash":  "google/gemini-3-flash",
		"gemini-pro":    "google/gemini-3-pro",
		"gemini3":       "google/gemini-3-flash",
		"gemini-3":      "google/gemini-3-flash",
		"google":        "google/gemini-3-flash",
		"gemini-2.5":    "google/gemini-2.5-flash",
		"gemini-lite":   "google/gemini-2.5-flash-lite",

		// DeepSeek aliases
		"deepseek":      "deepseek/deepseek-chat",
		"deepseek-chat": "deepseek/deepseek-chat",
		"reasoner":      "deepseek/deepseek-reasoner",

		// Ollama aliases
		"llama":    "ollama/llama3.3",
		"llama3":   "ollama/llama3.3",
		"qwen":     "ollama/qwen2.5",
		"mistral":  "ollama/mistral",
		"local":    "ollama/llama3.3",

		// Semantic aliases
		"fast":    "anthropic/claude-haiku-4-5",
		"quick":   "anthropic/claude-haiku-4-5",
		"cheap":   "deepseek/deepseek-chat",
		"smart":   "anthropic/claude-opus-4-5",
		"best":    "anthropic/claude-opus-4-5",
		"reason":  "anthropic/claude-opus-4-5",
		"think":   "anthropic/claude-opus-4-5",
		"code":    "anthropic/claude-sonnet-4-5",
		"default": "anthropic/claude-sonnet-4-5",
	}

	for alias, modelID := range keywords {
		f.aliases[alias] = modelID
	}

	// Also add all actual model IDs as aliases
	if f.config != nil {
		for providerName, models := range f.config.Providers {
			for _, m := range models {
				fullID := providerName + "/" + m.ID
				f.aliases[strings.ToLower(m.ID)] = fullID
				f.aliases[strings.ToLower(fullID)] = fullID
			}
		}
	}
}

// Match returns the best matching model ID for the given user input
// Returns empty string if no match found
func (f *FuzzyMatcher) Match(input string) string {
	input = strings.ToLower(strings.TrimSpace(input))
	if input == "" {
		return ""
	}

	// 1. Exact alias match
	if modelID, ok := f.aliases[input]; ok {
		if f.isModelAvailable(modelID) {
			return modelID
		}
	}

	// 2. Check if input contains any keyword
	for keyword, modelID := range f.aliases {
		if strings.Contains(input, keyword) {
			if f.isModelAvailable(modelID) {
				return modelID
			}
		}
	}

	// 3. Levenshtein distance for typo tolerance
	return f.findClosestMatch(input)
}

// isModelAvailable checks if a model is available in the config
func (f *FuzzyMatcher) isModelAvailable(modelID string) bool {
	if f.config == nil {
		return true // Assume available if no config
	}

	parts := strings.SplitN(modelID, "/", 2)
	if len(parts) != 2 {
		return false
	}

	providerID := parts[0]
	modelName := parts[1]

	models, ok := f.config.Providers[providerID]
	if !ok {
		return false
	}

	for _, m := range models {
		if m.ID == modelName && m.IsActive() {
			return true
		}
	}

	return false
}

// findClosestMatch finds the model with the smallest Levenshtein distance
func (f *FuzzyMatcher) findClosestMatch(input string) string {
	if f.config == nil {
		return ""
	}

	var bestMatch string
	bestDistance := 1000 // Large initial value

	// Check all model IDs and aliases
	for alias, modelID := range f.aliases {
		if !f.isModelAvailable(modelID) {
			continue
		}

		distance := levenshteinDistance(input, alias)
		// Only accept if distance is at most 3 (allowing for small typos)
		if distance < bestDistance && distance <= 3 {
			bestDistance = distance
			bestMatch = modelID
		}
	}

	return bestMatch
}

// levenshteinDistance calculates the edit distance between two strings
func levenshteinDistance(s1, s2 string) int {
	s1 = strings.ToLower(s1)
	s2 = strings.ToLower(s2)

	if len(s1) == 0 {
		return len(s2)
	}
	if len(s2) == 0 {
		return len(s1)
	}
	if s1 == s2 {
		return 0
	}

	// Create matrix
	r1 := []rune(s1)
	r2 := []rune(s2)
	len1, len2 := len(r1), len(r2)

	// Use two rows instead of full matrix for efficiency
	prev := make([]int, len2+1)
	curr := make([]int, len2+1)

	for j := 0; j <= len2; j++ {
		prev[j] = j
	}

	for i := 1; i <= len1; i++ {
		curr[0] = i
		for j := 1; j <= len2; j++ {
			cost := 0
			if r1[i-1] != r2[j-1] {
				cost = 1
			}
			curr[j] = min(
				prev[j]+1,      // deletion
				curr[j-1]+1,    // insertion
				prev[j-1]+cost, // substitution
			)
		}
		prev, curr = curr, prev
	}

	return prev[len2]
}

// ParseModelRequest parses user input for model switching requests
// Returns the model ID if user is requesting a model switch, empty string otherwise
// Patterns: "use X", "switch to X", "use the X model", etc.
func ParseModelRequest(input string) string {
	input = strings.ToLower(strings.TrimSpace(input))

	// Common patterns for model switching
	patterns := []string{
		"use ",
		"switch to ",
		"change to ",
		"try ",
		"with ",
	}

	for _, pattern := range patterns {
		if idx := strings.Index(input, pattern); idx != -1 {
			// Extract model name after the pattern
			remainder := input[idx+len(pattern):]
			// Remove common suffixes
			remainder = strings.TrimSuffix(remainder, " model")
			remainder = strings.TrimSuffix(remainder, " please")
			remainder = strings.TrimSuffix(remainder, " for this")

			// Clean up
			remainder = strings.TrimSpace(remainder)
			// Remove punctuation
			remainder = strings.Map(func(r rune) rune {
				if unicode.IsPunct(r) {
					return -1
				}
				return r
			}, remainder)

			if remainder != "" {
				return remainder
			}
		}
	}

	return ""
}
