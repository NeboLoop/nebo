package ai

import (
	"fmt"
	"sort"
	"strings"
	"unicode"

	"github.com/nebolabs/nebo/internal/provider"
)

// Variant tokens - common model suffixes that affect scoring
var variantTokens = []string{
	"lightning", "preview", "mini", "fast", "turbo", "lite",
	"beta", "small", "nano", "instant", "pro", "thinking",
}

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

		// Add provider name as alias
		f.aliases[strings.ToLower(providerName)] = fullID

		// Check if this is a CLI provider (has command in credentials)
		if f.config.Credentials != nil {
			if cred, ok := f.config.Credentials[providerName]; ok && cred.Command != "" {
				if firstCLIProvider == "" {
					firstCLIProvider = fullID
				}
				f.aliases[strings.ToLower(cred.Command)] = fullID
			} else if cred.APIKey != "" || cred.BaseURL != "" {
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

			// Add short-form aliases from model ID parts
			parts := strings.FieldsFunc(strings.ToLower(m.ID), func(r rune) bool {
				return r == '-' || r == '_' || r == '.'
			})
			for _, part := range parts {
				if len(part) < 3 || isNumeric(part) || part == "claude" || part == "gpt" {
					continue
				}
				if _, exists := f.aliases[part]; !exists {
					f.aliases[part] = mFullID
				}
			}

			// Build kind mappings
			for _, kind := range m.Kind {
				kindLower := strings.ToLower(kind)
				kindToModels[kindLower] = append(kindToModels[kindLower], mFullID)
				if m.Preferred {
					kindPreferred[kindLower] = mFullID
				}
			}
		}
	}

	// 3. Add kind tags as aliases (preferred model, or first available)
	for kind, models := range kindToModels {
		if _, exists := f.aliases[kind]; exists {
			continue
		}
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

// normalize removes dashes, dots, spaces, and underscores for fuzzy comparison
func normalize(s string) string {
	var result strings.Builder
	for _, r := range strings.ToLower(s) {
		if r != '-' && r != '.' && r != ' ' && r != '_' {
			result.WriteRune(r)
		}
	}
	return result.String()
}

// isNumeric returns true if the string contains only digits
func isNumeric(s string) bool {
	for _, r := range s {
		if r < '0' || r > '9' {
			return false
		}
	}
	return len(s) > 0
}

// matchCandidate represents a potential match with its score
type matchCandidate struct {
	modelID  string
	score    int
	alias    string
}

// Match returns the best matching model ID for the given user input
// Uses score-based matching to find the best candidate
func (f *FuzzyMatcher) Match(input string) string {
	input = strings.ToLower(strings.TrimSpace(input))
	if input == "" {
		return ""
	}

	// Collect all candidates with scores
	var candidates []matchCandidate
	normalizedInput := normalize(input)
	inputWords := strings.Fields(input)

	// Extract variant tokens from input
	inputVariants := extractVariants(input)

	for alias, modelID := range f.aliases {
		if !f.isModelAvailable(modelID) {
			continue
		}

		score := f.scoreMatch(input, normalizedInput, inputWords, inputVariants, alias, modelID)
		if score > 0 {
			candidates = append(candidates, matchCandidate{
				modelID: modelID,
				score:   score,
				alias:   alias,
			})
		}
	}

	if len(candidates) == 0 {
		return ""
	}

	// Find the best candidate
	best := candidates[0]
	for _, c := range candidates[1:] {
		if c.score > best.score {
			best = c
		} else if c.score == best.score {
			// Prefer shorter model IDs (more specific matches)
			if len(c.modelID) < len(best.modelID) {
				best = c
			}
		}
	}

	// Minimum score threshold to avoid weak matches
	if best.score < 50 {
		return ""
	}

	return best.modelID
}

// scoreMatch calculates a match score between input and an alias
func (f *FuzzyMatcher) scoreMatch(input, normalizedInput string, inputWords, inputVariants []string, alias, modelID string) int {
	score := 0
	aliasLower := strings.ToLower(alias)
	normalizedAlias := normalize(alias)

	// Extract provider and model from modelID for additional matching
	parts := strings.SplitN(modelID, "/", 2)
	providerLower := ""
	modelLower := ""
	if len(parts) == 2 {
		providerLower = strings.ToLower(parts[0])
		modelLower = strings.ToLower(parts[1])
	}

	// 1. Exact match (highest score)
	if input == aliasLower {
		score += 300
	}

	// 2. Normalized exact match (e.g., "gpt52" == "gpt-5.2")
	if normalizedInput == normalizedAlias {
		score += 250
	}

	// 3. Input starts with alias or alias starts with input
	if strings.HasPrefix(input, aliasLower) {
		score += 150
	}
	if strings.HasPrefix(aliasLower, input) {
		score += 140
	}

	// 4. Normalized prefix matching
	if strings.HasPrefix(normalizedInput, normalizedAlias) {
		score += 130
	}
	if strings.HasPrefix(normalizedAlias, normalizedInput) {
		score += 120
	}

	// 5. Contains matching
	if strings.Contains(input, aliasLower) {
		score += 100
	}
	if strings.Contains(aliasLower, input) {
		score += 90
	}

	// 6. Normalized contains matching
	if strings.Contains(normalizedInput, normalizedAlias) && len(normalizedAlias) >= 3 {
		score += 80
	}
	if strings.Contains(normalizedAlias, normalizedInput) && len(normalizedInput) >= 3 {
		score += 70
	}

	// 7. Word matching - check if any input word matches alias or model parts
	for _, word := range inputWords {
		if len(word) < 3 {
			continue
		}
		if word == aliasLower {
			score += 120
		} else if strings.Contains(aliasLower, word) {
			score += 60
		} else if strings.Contains(modelLower, word) {
			score += 50
		} else if strings.Contains(providerLower, word) {
			score += 40
		}
	}

	// 8. Levenshtein distance for typo tolerance
	dist := boundedLevenshtein(input, aliasLower, 3)
	if dist != nil {
		score += (4 - *dist) * 50 // 0 dist = 200, 1 = 150, 2 = 100, 3 = 50
	}

	// Also check normalized Levenshtein
	normDist := boundedLevenshtein(normalizedInput, normalizedAlias, 3)
	if normDist != nil {
		score += (4 - *normDist) * 40
	}

	// 9. Variant token handling
	aliasVariants := extractVariants(aliasLower)
	modelVariants := extractVariants(modelLower)
	allModelVariants := unique(append(aliasVariants, modelVariants...))

	if len(inputVariants) > 0 {
		// User asked for specific variants
		matchCount := 0
		for _, iv := range inputVariants {
			for _, mv := range allModelVariants {
				if iv == mv {
					matchCount++
					break
				}
			}
		}
		if matchCount > 0 {
			score += matchCount * 60 // Reward matching variants
		} else if len(allModelVariants) > 0 {
			score -= 30 // Penalty: user asked for variant but model has different ones
		}
	} else if len(allModelVariants) > 0 {
		// User didn't ask for variant but model has variants - slight penalty
		score -= len(allModelVariants) * 15
	}

	return score
}

// extractVariants returns variant tokens found in the string
func extractVariants(s string) []string {
	var found []string
	lower := strings.ToLower(s)
	for _, v := range variantTokens {
		if strings.Contains(lower, v) {
			found = append(found, v)
		}
	}
	return found
}

// unique returns unique strings from a slice
func unique(strs []string) []string {
	seen := make(map[string]bool)
	var result []string
	for _, s := range strs {
		if !seen[s] {
			seen[s] = true
			result = append(result, s)
		}
	}
	return result
}

// boundedLevenshtein calculates Levenshtein distance with early exit
// Returns nil if distance exceeds maxDist
func boundedLevenshtein(a, b string, maxDist int) *int {
	if a == b {
		zero := 0
		return &zero
	}
	if len(a) == 0 || len(b) == 0 {
		return nil
	}
	if abs(len(a)-len(b)) > maxDist {
		return nil
	}

	r1 := []rune(a)
	r2 := []rune(b)
	len1, len2 := len(r1), len(r2)

	prev := make([]int, len2+1)
	curr := make([]int, len2+1)

	for j := 0; j <= len2; j++ {
		prev[j] = j
	}

	for i := 1; i <= len1; i++ {
		curr[0] = i
		rowMin := curr[0]

		for j := 1; j <= len2; j++ {
			cost := 0
			if r1[i-1] != r2[j-1] {
				cost = 1
			}
			curr[j] = min(prev[j]+1, curr[j-1]+1, prev[j-1]+cost)
			if curr[j] < rowMin {
				rowMin = curr[j]
			}
		}

		// Early exit if minimum in row exceeds threshold
		if rowMin > maxDist {
			return nil
		}

		prev, curr = curr, prev
	}

	dist := prev[len2]
	if dist > maxDist {
		return nil
	}
	return &dist
}

func abs(x int) int {
	if x < 0 {
		return -x
	}
	return x
}

// levenshteinDistance calculates the edit distance between two strings
// Exported for testing
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

	r1 := []rune(s1)
	r2 := []rune(s2)
	len1, len2 := len(r1), len(r2)

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
			curr[j] = min(prev[j]+1, curr[j-1]+1, prev[j-1]+cost)
		}
		prev, curr = curr, prev
	}

	return prev[len2]
}

// GetAliases returns all aliases for system prompt injection
// Returns lines like "- sonnet: anthropic/claude-sonnet-4"
func (f *FuzzyMatcher) GetAliases() []string {
	if f == nil || len(f.aliases) == 0 {
		return nil
	}

	// Dedupe by model ID (keep shortest alias per model)
	modelToAlias := make(map[string]string)
	for alias, modelID := range f.aliases {
		// Skip very short aliases and full model IDs
		if len(alias) < 3 || strings.Contains(alias, "/") {
			continue
		}
		// Skip numeric-only aliases
		if isNumeric(alias) {
			continue
		}
		existing, ok := modelToAlias[modelID]
		if !ok || len(alias) < len(existing) {
			modelToAlias[modelID] = alias
		}
	}

	// Build sorted lines
	var lines []string
	for modelID, alias := range modelToAlias {
		if f.isModelAvailable(modelID) {
			lines = append(lines, fmt.Sprintf("- %s: %s", alias, modelID))
		}
	}

	// Sort for consistent output
	sort.Strings(lines)
	return lines
}

// isModelAvailable checks if a model is available in the config AND has credentials
func (f *FuzzyMatcher) isModelAvailable(modelID string) bool {
	if f.config == nil {
		return true
	}

	parts := strings.SplitN(modelID, "/", 2)
	if len(parts) != 2 {
		return false
	}

	providerID := parts[0]
	modelName := parts[1]

	// Check if provider has credentials configured
	if f.config.Credentials != nil {
		creds, ok := f.config.Credentials[providerID]
		if !ok {
			return false
		}
		// Provider needs API key, base URL (Ollama), or command (CLI)
		if creds.APIKey == "" && creds.BaseURL == "" && creds.Command == "" {
			return false
		}
	}

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

// ParseModelRequest parses user input for model switching requests
// Returns the model ID if user is requesting a model switch, empty string otherwise
func ParseModelRequest(input string) string {
	input = strings.ToLower(strings.TrimSpace(input))

	patterns := []string{
		"use ",
		"switch to ",
		"change to ",
		"try ",
		"with ",
	}

	for _, pattern := range patterns {
		if idx := strings.Index(input, pattern); idx != -1 {
			remainder := input[idx+len(pattern):]
			remainder = strings.TrimSuffix(remainder, " model")
			remainder = strings.TrimSuffix(remainder, " please")
			remainder = strings.TrimSuffix(remainder, " for this")
			remainder = strings.TrimSpace(remainder)
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
