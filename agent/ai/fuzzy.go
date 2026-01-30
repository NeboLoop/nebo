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

// buildAliases creates aliases for models from config and kind tags
func (f *FuzzyMatcher) buildAliases() {
	if f.config == nil {
		return
	}

	// 1. First, load user-configured aliases from models.yaml (highest priority)
	for _, alias := range f.config.Aliases {
		f.aliases[strings.ToLower(alias.Alias)] = alias.ModelId
	}

	// 2. Build aliases from model kind tags (preferred models first)
	// This replaces hardcoded semantic aliases - now driven by models.yaml
	kindToModels := make(map[string][]string) // kind -> list of model IDs
	kindPreferred := make(map[string]string)  // kind -> preferred model ID

	var firstAPIProvider string
	var firstCLIProvider string

	for providerName, models := range f.config.Providers {
		if len(models) == 0 {
			continue
		}

		// Get first active model for this provider
		var firstModel string
		for _, m := range models {
			if m.IsActive() {
				firstModel = m.ID
				break
			}
		}
		if firstModel == "" {
			continue
		}

		fullID := providerName + "/" + firstModel

		// Add provider name as alias (e.g., "anthropic" -> "anthropic/claude-sonnet-4-5")
		f.aliases[strings.ToLower(providerName)] = fullID

		// Check if this is a CLI provider (has command in credentials)
		if f.config.Credentials != nil {
			if cred, ok := f.config.Credentials[providerName]; ok && cred.Command != "" {
				// CLI provider - add CLI-related aliases
				if firstCLIProvider == "" {
					firstCLIProvider = fullID
				}
				// Add command name as alias (e.g., "claude" for claude-code)
				f.aliases[strings.ToLower(cred.Command)] = fullID
			} else if cred.APIKey != "" || cred.BaseURL != "" {
				// API provider
				if firstAPIProvider == "" {
					firstAPIProvider = fullID
				}
			}
		}

		// Add all model IDs, display names, and kind tags as aliases
		for _, m := range models {
			if !m.IsActive() {
				continue
			}
			mFullID := providerName + "/" + m.ID
			f.aliases[strings.ToLower(m.ID)] = mFullID
			f.aliases[strings.ToLower(mFullID)] = mFullID
			if m.DisplayName != "" {
				f.aliases[strings.ToLower(m.DisplayName)] = mFullID
			}

			// Build kind mappings
			for _, kind := range m.Kind {
				kindLower := strings.ToLower(kind)
				kindToModels[kindLower] = append(kindToModels[kindLower], mFullID)
				// Track preferred model for each kind
				if m.Preferred {
					kindPreferred[kindLower] = mFullID
				}
			}
		}
	}

	// 3. Add kind tags as aliases (preferred model, or first available)
	for kind, models := range kindToModels {
		if _, exists := f.aliases[kind]; exists {
			continue // User-configured alias takes precedence
		}
		// Use preferred model if set, otherwise first in list
		if preferred, ok := kindPreferred[kind]; ok {
			f.aliases[kind] = preferred
		} else if len(models) > 0 {
			f.aliases[kind] = models[0]
		}
	}

	// 4. Add "api" alias pointing to first API provider
	if firstAPIProvider != "" {
		if _, exists := f.aliases["api"]; !exists {
			f.aliases["api"] = firstAPIProvider
		}
	}

	// 5. Add "cli"/"terminal"/"agentic" aliases pointing to first CLI provider
	if firstCLIProvider != "" {
		for _, alias := range []string{"cli", "terminal", "agentic"} {
			if _, exists := f.aliases[alias]; !exists {
				f.aliases[alias] = firstCLIProvider
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
