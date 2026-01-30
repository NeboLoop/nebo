package ai

import (
	"testing"

	"nebo/internal/provider"
)

func TestFuzzyMatcherExactAlias(t *testing.T) {
	active := true
	config := &provider.ModelsConfig{
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Active: &active, Kind: []string{"smart"}},
				{ID: "claude-opus-4-5", Active: &active, Kind: []string{"smart", "reason"}},
				{ID: "claude-haiku-4-5", Active: &active, Kind: []string{"fast"}},
			},
			"openai": {
				{ID: "gpt-5.2", Active: &active},
				{ID: "gpt-5.2-instant", Active: &active},
				{ID: "gpt-5-nano", Active: &active, Kind: []string{"cheap"}},
			},
			"deepseek": {
				{ID: "deepseek-chat", Active: &active, Kind: []string{"cheap"}},
			},
		},
	}

	matcher := NewFuzzyMatcher(config)

	tests := []struct {
		input string
		want  string
	}{
		{"sonnet", "anthropic/claude-sonnet-4-5"},
		{"opus", "anthropic/claude-opus-4-5"},
		{"haiku", "anthropic/claude-haiku-4-5"},
		{"gpt-5.2", "openai/gpt-5.2"},
		{"claude-sonnet-4-5", "anthropic/claude-sonnet-4-5"},
	}

	for _, tc := range tests {
		got := matcher.Match(tc.input)
		if got != tc.want {
			t.Errorf("Match(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestFuzzyMatcherNormalized(t *testing.T) {
	// Test that normalized matching works for user-friendly inputs
	// e.g., "gpt52 nano" should match "gpt-5.2-nano" or "gpt-5-nano"
	active := true
	config := &provider.ModelsConfig{
		Providers: map[string][]provider.ModelInfo{
			"openai": {
				{ID: "gpt-5.2", Active: &active},
				{ID: "gpt-5-nano", Active: &active},
				{ID: "gpt-5-mini", Active: &active},
			},
			"anthropic": {
				{ID: "claude-sonnet-4-5", Active: &active},
				{ID: "claude-opus-4-5", Active: &active},
			},
		},
	}

	matcher := NewFuzzyMatcher(config)

	tests := []struct {
		input string
		want  string
	}{
		// Normalized matching - removing dashes/dots should work
		{"gpt52", "openai/gpt-5.2"},
		{"gpt5nano", "openai/gpt-5-nano"},
		{"gpt5mini", "openai/gpt-5-mini"},
		{"gpt 5 nano", "openai/gpt-5-nano"},
		{"gpt5.2", "openai/gpt-5.2"},
		{"claudesonnet45", "anthropic/claude-sonnet-4-5"},
		// User-reported case: "gpt52 nano"
		{"gpt52 nano", "openai/gpt-5-nano"},
	}

	for _, tc := range tests {
		got := matcher.Match(tc.input)
		if got != tc.want {
			t.Errorf("Match(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestFuzzyMatcherKeywordContains(t *testing.T) {
	active := true
	config := &provider.ModelsConfig{
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Active: &active},
				{ID: "claude-opus-4-5", Active: &active},
			},
		},
	}

	matcher := NewFuzzyMatcher(config)

	// Should match when input contains a keyword
	tests := []struct {
		input string
		want  string
	}{
		{"use the opus model", "anthropic/claude-opus-4-5"},
		{"i want sonnet please", "anthropic/claude-sonnet-4-5"},
	}

	for _, tc := range tests {
		got := matcher.Match(tc.input)
		if got != tc.want {
			t.Errorf("Match(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestFuzzyMatcherTypoTolerance(t *testing.T) {
	active := true
	config := &provider.ModelsConfig{
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Active: &active},
				{ID: "claude-opus-4-5", Active: &active},
				{ID: "claude-haiku-4-5", Active: &active},
			},
		},
	}

	matcher := NewFuzzyMatcher(config)

	// Should match with small typos (edit distance <= 3 on normalized)
	tests := []struct {
		input string
		want  string
	}{
		{"sonet", "anthropic/claude-sonnet-4-5"},   // missing 'n'
		{"opuss", "anthropic/claude-opus-4-5"},     // extra 's'
		{"haiuk", "anthropic/claude-haiku-4-5"},    // transposed letters
		{"sonnett", "anthropic/claude-sonnet-4-5"}, // extra 't'
	}

	for _, tc := range tests {
		got := matcher.Match(tc.input)
		if got != tc.want {
			t.Errorf("Match(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestFuzzyMatcherNoMatch(t *testing.T) {
	active := true
	config := &provider.ModelsConfig{
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Active: &active},
			},
		},
	}

	matcher := NewFuzzyMatcher(config)

	// Should return empty for unrecognized inputs
	tests := []string{
		"xyzabc",
		"totally unknown model",
		"",
	}

	for _, input := range tests {
		got := matcher.Match(input)
		if got != "" {
			t.Errorf("Match(%q) = %q, want empty string", input, got)
		}
	}
}

func TestParseModelRequest(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{"use sonnet", "sonnet"},
		{"switch to opus", "opus"},
		{"change to gpt-4", "gpt4"},
		{"use the haiku model", "the haiku"},  // "the" is kept, fuzzy matcher handles it
		{"try claude please", "claude"},
		{"with gemini for this", "gemini"},
		{"hello world", ""},                   // Not a model request
		{"how do I use this?", "this"},        // Contains "use" but not a model switch
	}

	for _, tc := range tests {
		got := ParseModelRequest(tc.input)
		if got != tc.want {
			t.Errorf("ParseModelRequest(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestLevenshteinDistance(t *testing.T) {
	tests := []struct {
		s1   string
		s2   string
		want int
	}{
		{"", "", 0},
		{"a", "", 1},
		{"", "a", 1},
		{"abc", "abc", 0},
		{"abc", "abd", 1},     // substitution
		{"abc", "abcd", 1},    // insertion
		{"abcd", "abc", 1},    // deletion
		{"abc", "xyz", 3},     // all different
		{"kitten", "sitting", 3},
		{"sonnet", "sonet", 1}, // missing letter
	}

	for _, tc := range tests {
		got := levenshteinDistance(tc.s1, tc.s2)
		if got != tc.want {
			t.Errorf("levenshteinDistance(%q, %q) = %d, want %d", tc.s1, tc.s2, got, tc.want)
		}
	}
}
