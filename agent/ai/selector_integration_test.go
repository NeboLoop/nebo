package ai

import (
	"testing"

	"gobot/agent/session"
	"gobot/internal/provider"
)

// TestIntegration_ModelSelectionWithProviderSwitching verifies the full model selection flow
func TestIntegration_ModelSelectionWithProviderSwitching(t *testing.T) {
	// Simulate a config with multiple providers (2026 models)
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Vision:    "anthropic/claude-sonnet-4-5",
			Reasoning: "anthropic/claude-opus-4-5",
			Code:      "openai/gpt-5.2-codex", // Code goes to OpenAI Codex
			General:   "anthropic/claude-sonnet-4-5",
			Fallbacks: map[string][]string{
				"code":      {"anthropic/claude-sonnet-4-5"},
				"reasoning": {"openai/gpt-5.2-thinking"},
				"general":   {"openai/gpt-5.2"},
			},
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Capabilities: []string{"code", "vision"}},
				{ID: "claude-opus-4-5", Capabilities: []string{"reasoning", "thinking"}},
			},
			"openai": {
				{ID: "gpt-5.2", Capabilities: []string{"general"}},
				{ID: "gpt-5.2-codex", Capabilities: []string{"code"}},
				{ID: "gpt-5.2-thinking", Capabilities: []string{"reasoning", "thinking"}},
			},
		},
	}

	selector := NewModelSelector(config)

	// Test 1: Code task should select OpenAI
	codeMessages := []session.Message{
		{Role: "user", Content: "Write a function to calculate fibonacci numbers in Python"},
	}
	selectedModel := selector.Select(codeMessages)
	providerID, modelName := ParseModelID(selectedModel)

	if providerID != "openai" {
		t.Errorf("Code task: expected provider 'openai', got '%s' (model: %s)", providerID, selectedModel)
	}
	if modelName != "gpt-5.2-codex" {
		t.Errorf("Code task: expected model 'gpt-5.2-codex', got '%s'", modelName)
	}
	t.Logf("Code task selected: %s (provider: %s, model: %s)", selectedModel, providerID, modelName)

	// Test 2: Reasoning task should select Anthropic Opus
	reasoningMessages := []session.Message{
		{Role: "user", Content: "Think through this complex mathematical proof step by step"},
	}
	selectedModel = selector.Select(reasoningMessages)
	providerID, modelName = ParseModelID(selectedModel)

	if providerID != "anthropic" {
		t.Errorf("Reasoning task: expected provider 'anthropic', got '%s'", providerID)
	}
	if modelName != "claude-opus-4-5" {
		t.Errorf("Reasoning task: expected model 'claude-opus-4-5', got '%s'", modelName)
	}
	t.Logf("Reasoning task selected: %s (provider: %s, model: %s)", selectedModel, providerID, modelName)

	// Test 3: Check thinking support for opus
	if !selector.SupportsThinking(selectedModel) {
		t.Error("Opus should support thinking mode")
	}

	// Test 4: General task should select Anthropic Sonnet
	generalMessages := []session.Message{
		{Role: "user", Content: "Hello, how are you today?"},
	}
	selectedModel = selector.Select(generalMessages)
	providerID, modelName = ParseModelID(selectedModel)

	if providerID != "anthropic" {
		t.Errorf("General task: expected provider 'anthropic', got '%s'", providerID)
	}
	t.Logf("General task selected: %s (provider: %s, model: %s)", selectedModel, providerID, modelName)

	// Test 5: Fallback when primary is in cooldown
	selector.MarkFailed("openai/gpt-5.2-codex")
	selectedModel = selector.Select(codeMessages)
	providerID, modelName = ParseModelID(selectedModel)

	if providerID != "anthropic" {
		t.Errorf("Code task (fallback): expected provider 'anthropic' after OpenAI failure, got '%s'", providerID)
	}
	t.Logf("Code task (fallback) selected: %s (provider: %s, model: %s)", selectedModel, providerID, modelName)

	// Test 6: Clear failed models and verify original selection returns
	selector.ClearFailed()
	selectedModel = selector.Select(codeMessages)
	providerID, _ = ParseModelID(selectedModel)

	if providerID != "openai" {
		t.Errorf("Code task (after clear): expected provider 'openai', got '%s'", providerID)
	}
	t.Logf("Code task (after clear) selected: %s", selectedModel)
}

// TestIntegration_FuzzyModelMatching verifies fuzzy matching works end-to-end
func TestIntegration_FuzzyModelMatching(t *testing.T) {
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
				{ID: "gpt-5.2-thinking", Active: &active},
			},
			"google": {
				{ID: "gemini-3-flash", Active: &active},
			},
		},
	}

	matcher := NewFuzzyMatcher(config)

	testCases := []struct {
		input          string
		wantProvider   string
		wantContains   string
		description    string
	}{
		{"use opus", "anthropic", "opus", "explicit model request"},
		{"switch to gpt", "openai", "gpt-5.2", "OpenAI GPT-5.2 alias"},
		{"try gemini", "google", "gemini", "Google Gemini 3 alias"},
		{"use the fast model", "anthropic", "haiku", "semantic 'fast' alias"},
		{"i want the smart one", "anthropic", "opus", "semantic 'smart' alias"},
		{"sonet", "anthropic", "sonnet", "typo tolerance"},
	}

	for _, tc := range testCases {
		// First parse the model request
		modelRequest := ParseModelRequest(tc.input)
		if modelRequest == "" {
			// If no explicit "use X" pattern, use the full input for matching
			modelRequest = tc.input
		}

		// Then match
		result := matcher.Match(modelRequest)
		if result == "" {
			t.Errorf("%s: Match(%q) returned empty", tc.description, tc.input)
			continue
		}

		providerID, _ := ParseModelID(result)
		if providerID != tc.wantProvider {
			t.Errorf("%s: Match(%q) = %s, want provider %s", tc.description, tc.input, result, tc.wantProvider)
		} else {
			t.Logf("%s: '%s' -> %s ✓", tc.description, tc.input, result)
		}
	}
}

// TestIntegration_CooldownExponentialBackoff verifies cooldown timing
func TestIntegration_CooldownExponentialBackoff(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			General: "anthropic/claude-sonnet-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {{ID: "claude-sonnet-4-5"}},
		},
	}

	selector := NewModelSelector(config)
	modelID := "anthropic/claude-sonnet-4-5"

	// First failure: 5 seconds
	selector.MarkFailed(modelID)
	cooldown1 := selector.GetCooldownRemaining(modelID)

	// Second failure: 10 seconds
	selector.MarkFailed(modelID)
	cooldown2 := selector.GetCooldownRemaining(modelID)

	// Third failure: 20 seconds
	selector.MarkFailed(modelID)
	cooldown3 := selector.GetCooldownRemaining(modelID)

	t.Logf("Cooldown after 1st failure: %v", cooldown1)
	t.Logf("Cooldown after 2nd failure: %v", cooldown2)
	t.Logf("Cooldown after 3rd failure: %v", cooldown3)

	// Verify exponential increase
	if cooldown2 <= cooldown1 {
		t.Errorf("Expected cooldown to increase: %v should be > %v", cooldown2, cooldown1)
	}
	if cooldown3 <= cooldown2 {
		t.Errorf("Expected cooldown to increase: %v should be > %v", cooldown3, cooldown2)
	}
}
