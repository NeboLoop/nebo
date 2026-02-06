package ai

import (
	"testing"
	"time"

	"github.com/nebolabs/nebo/internal/agent/session"
	"github.com/nebolabs/nebo/internal/provider"
)

func TestNewModelSelector(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Vision:    "anthropic/claude-sonnet-4-5",
			Reasoning: "anthropic/claude-opus-4-5",
			Code:      "anthropic/claude-sonnet-4-5",
			General:   "anthropic/claude-sonnet-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Capabilities: []string{"vision", "tools", "streaming", "code"}},
				{ID: "claude-opus-4-5", Capabilities: []string{"vision", "tools", "streaming", "code", "reasoning"}},
			},
		},
	}

	selector := NewModelSelector(config)
	if selector == nil {
		t.Fatal("expected non-nil selector")
	}
}

func TestClassifyTask_Vision(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Vision:  "anthropic/claude-sonnet-4-5",
			General: "anthropic/claude-sonnet-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	// Test with image content marker
	messages := []session.Message{
		{Role: "user", Content: `[{"type": "image", "source": {"type": "base64"}}]`},
	}

	taskType := selector.classifyTask(messages)
	if taskType != TaskTypeVision {
		t.Errorf("expected vision task type, got %s", taskType)
	}
}

func TestClassifyTask_Reasoning(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Reasoning: "anthropic/claude-opus-4-5",
			General:   "anthropic/claude-sonnet-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-opus-4-5"},
				{ID: "claude-sonnet-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	testCases := []struct {
		input    string
		expected TaskType
	}{
		{"Think through this problem step by step", TaskTypeReasoning},
		{"Analyze this complex situation", TaskTypeReasoning},
		{"Prove that this algorithm is correct", TaskTypeReasoning},
		{"Hello world", TaskTypeGeneral},
	}

	for _, tc := range testCases {
		messages := []session.Message{
			{Role: "user", Content: tc.input},
		}

		taskType := selector.classifyTask(messages)
		if taskType != tc.expected {
			t.Errorf("for input %q: expected %s, got %s", tc.input, tc.expected, taskType)
		}
	}
}

func TestClassifyTask_Code(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Code:    "anthropic/claude-sonnet-4-5",
			General: "anthropic/claude-haiku-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
				{ID: "claude-haiku-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	testCases := []struct {
		input    string
		expected TaskType
	}{
		{"Write a function that calculates fibonacci", TaskTypeCode},
		{"Implement this algorithm in Python", TaskTypeCode},
		{"Debug this JavaScript code", TaskTypeCode},
		{"Refactor the API endpoint", TaskTypeCode},
		{"What is the weather?", TaskTypeGeneral},
	}

	for _, tc := range testCases {
		messages := []session.Message{
			{Role: "user", Content: tc.input},
		}

		taskType := selector.classifyTask(messages)
		if taskType != tc.expected {
			t.Errorf("for input %q: expected %s, got %s", tc.input, tc.expected, taskType)
		}
	}
}

func TestClassifyTask_Audio(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Audio:   "openai/whisper-1",
			General: "anthropic/claude-sonnet-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"openai": {
				{ID: "whisper-1"},
			},
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	// Test keyword-based audio detection
	keywordTests := []struct {
		input    string
		expected TaskType
	}{
		{"Transcribe this audio file", TaskTypeAudio},
		{"Please do speech to text conversion", TaskTypeAudio},
		{"Convert this voice memo to text", TaskTypeAudio},
		{"I need text-to-speech for this", TaskTypeAudio},
		{"Listen to this recording and summarize", TaskTypeAudio},
		{"What is the weather?", TaskTypeGeneral},
	}

	for _, tc := range keywordTests {
		messages := []session.Message{
			{Role: "user", Content: tc.input},
		}

		taskType := selector.classifyTask(messages)
		if taskType != tc.expected {
			t.Errorf("for input %q: expected %s, got %s", tc.input, tc.expected, taskType)
		}
	}

	// Test multimodal audio content detection
	audioContentMessages := []session.Message{
		{Role: "user", Content: `[{"type": "audio", "source": {"type": "base64"}}]`},
	}
	taskType := selector.classifyTask(audioContentMessages)
	if taskType != TaskTypeAudio {
		t.Errorf("expected audio task type for audio content, got %s", taskType)
	}

	// Test input_audio type (OpenAI style)
	inputAudioMessages := []session.Message{
		{Role: "user", Content: `[{"type": "input_audio", "input_audio": {"data": "base64..."}}]`},
	}
	taskType = selector.classifyTask(inputAudioMessages)
	if taskType != TaskTypeAudio {
		t.Errorf("expected audio task type for input_audio content, got %s", taskType)
	}

	// Test base64 audio data pattern
	base64AudioMessages := []session.Message{
		{Role: "user", Content: "data:audio/mp3;base64,SGVsbG8gV29ybGQ="},
	}
	taskType = selector.classifyTask(base64AudioMessages)
	if taskType != TaskTypeAudio {
		t.Errorf("expected audio task type for base64 audio data, got %s", taskType)
	}
}

func TestSelectModel_AudioRouting(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Audio:   "openai/whisper-1",
			General: "anthropic/claude-sonnet-4-5",
			Fallbacks: map[string][]string{
				"audio": {"groq/whisper-large-v3"},
			},
		},
		Providers: map[string][]provider.ModelInfo{
			"openai": {
				{ID: "whisper-1"},
			},
			"groq": {
				{ID: "whisper-large-v3"},
			},
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	// Audio task should select audio model
	audioMessages := []session.Message{
		{Role: "user", Content: "transcribe this recording"},
	}
	audioModel := selector.Select(audioMessages)
	if audioModel != "openai/whisper-1" {
		t.Errorf("expected openai/whisper-1 for audio task, got %s", audioModel)
	}

	// With primary excluded, should use fallback
	audioModel = selector.SelectWithExclusions(audioMessages, []string{"openai/whisper-1"})
	if audioModel != "groq/whisper-large-v3" {
		t.Errorf("expected groq/whisper-large-v3 as audio fallback, got %s", audioModel)
	}
}

func TestSelectModel_WithRouting(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Vision:    "anthropic/claude-sonnet-4-5",
			Reasoning: "anthropic/claude-opus-4-5",
			Code:      "anthropic/claude-sonnet-4-5",
			General:   "anthropic/claude-haiku-4-5",
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
				{ID: "claude-opus-4-5"},
				{ID: "claude-haiku-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	// Test code task selects code model
	codeMessages := []session.Message{
		{Role: "user", Content: "implement a function to sort an array"},
	}
	codeModel := selector.Select(codeMessages)
	if codeModel != "anthropic/claude-sonnet-4-5" {
		t.Errorf("expected anthropic/claude-sonnet-4-5 for code task, got %s", codeModel)
	}

	// Test reasoning task selects reasoning model
	reasoningMessages := []session.Message{
		{Role: "user", Content: "analyze this complex problem step by step"},
	}
	reasoningModel := selector.Select(reasoningMessages)
	if reasoningModel != "anthropic/claude-opus-4-5" {
		t.Errorf("expected anthropic/claude-opus-4-5 for reasoning task, got %s", reasoningModel)
	}

	// Test general task selects general model
	generalMessages := []session.Message{
		{Role: "user", Content: "hello there"},
	}
	generalModel := selector.Select(generalMessages)
	if generalModel != "anthropic/claude-haiku-4-5" {
		t.Errorf("expected anthropic/claude-haiku-4-5 for general task, got %s", generalModel)
	}
}

func TestSelectModel_WithFallbacks(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			Vision:  "anthropic/claude-sonnet-4-5",
			General: "anthropic/claude-sonnet-4-5",
			Fallbacks: map[string][]string{
				"vision": {"anthropic/claude-opus-4-5", "openai/gpt-5.2"},
			},
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
				{ID: "claude-opus-4-5"},
			},
			"openai": {
				{ID: "gpt-5.2"},
			},
		},
	}

	selector := NewModelSelector(config)

	// Exclude primary model
	visionMessages := []session.Message{
		{Role: "user", Content: `[{"type": "image"}]`},
	}

	// First selection should be primary
	model := selector.Select(visionMessages)
	if model != "anthropic/claude-sonnet-4-5" {
		t.Errorf("expected primary model, got %s", model)
	}

	// Selection with exclusion should fallback
	model = selector.SelectWithExclusions(visionMessages, []string{"anthropic/claude-sonnet-4-5"})
	if model != "anthropic/claude-opus-4-5" {
		t.Errorf("expected first fallback model, got %s", model)
	}

	// Exclude both primary and first fallback
	model = selector.SelectWithExclusions(visionMessages, []string{
		"anthropic/claude-sonnet-4-5",
		"anthropic/claude-opus-4-5",
	})
	if model != "openai/gpt-5.2" {
		t.Errorf("expected second fallback model, got %s", model)
	}
}

func TestMarkFailed(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			General: "anthropic/claude-sonnet-4-5",
			Fallbacks: map[string][]string{
				"general": {"anthropic/claude-haiku-4-5"},
			},
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
				{ID: "claude-haiku-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	messages := []session.Message{
		{Role: "user", Content: "hello"},
	}

	// Initially selects primary
	model := selector.Select(messages)
	if model != "anthropic/claude-sonnet-4-5" {
		t.Errorf("expected primary model, got %s", model)
	}

	// Mark primary as failed
	selector.MarkFailed("anthropic/claude-sonnet-4-5")

	// Now should select fallback
	model = selector.Select(messages)
	if model != "anthropic/claude-haiku-4-5" {
		t.Errorf("expected fallback model after marking primary failed, got %s", model)
	}

	// Clear failed models
	selector.ClearFailed()

	// Should select primary again
	model = selector.Select(messages)
	if model != "anthropic/claude-sonnet-4-5" {
		t.Errorf("expected primary model after clearing failed, got %s", model)
	}
}

func TestParseModelID(t *testing.T) {
	testCases := []struct {
		input          string
		wantProvider   string
		wantModel      string
	}{
		{"anthropic/claude-sonnet-4-5", "anthropic", "claude-sonnet-4-5"},
		{"openai/gpt-5.2", "openai", "gpt-5.2"},
		{"just-model-name", "", "just-model-name"},
		{"provider/model/with/slashes", "provider", "model/with/slashes"},
	}

	for _, tc := range testCases {
		provider, model := ParseModelID(tc.input)
		if provider != tc.wantProvider || model != tc.wantModel {
			t.Errorf("ParseModelID(%q) = (%q, %q), want (%q, %q)",
				tc.input, provider, model, tc.wantProvider, tc.wantModel)
		}
	}
}

func TestIsModelAvailable(t *testing.T) {
	active := true
	inactive := false

	config := &provider.ModelsConfig{
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5", Active: &active},
				{ID: "claude-opus-4-5", Active: &inactive},
			},
		},
	}

	selector := NewModelSelector(config)

	// Active model should be available
	if !selector.isModelAvailable("anthropic/claude-sonnet-4-5") {
		t.Error("expected claude-sonnet-4-5 to be available")
	}

	// Inactive model should not be available
	if selector.isModelAvailable("anthropic/claude-opus-4-5") {
		t.Error("expected inactive model to not be available")
	}

	// Non-existent provider should not be available
	if selector.isModelAvailable("unknown/model") {
		t.Error("expected unknown provider to not be available")
	}

	// Non-existent model should not be available
	if selector.isModelAvailable("anthropic/unknown-model") {
		t.Error("expected unknown model to not be available")
	}
}

func TestCooldownWithExponentialBackoff(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			General: "anthropic/claude-sonnet-4-5",
			Fallbacks: map[string][]string{
				"general": {"anthropic/claude-haiku-4-5"},
			},
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
				{ID: "claude-haiku-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	// Mark model as failed
	selector.MarkFailed("anthropic/claude-sonnet-4-5")

	// Should be in cooldown
	if !selector.isInCooldown("anthropic/claude-sonnet-4-5") {
		t.Error("expected model to be in cooldown after first failure")
	}

	// Should have remaining cooldown time (~5 seconds for first failure)
	remaining := selector.GetCooldownRemaining("anthropic/claude-sonnet-4-5")
	if remaining < 4*time.Second || remaining > 6*time.Second {
		t.Errorf("expected ~5s cooldown on first failure, got %v", remaining)
	}

	// Model not in cooldown should have 0 remaining
	remaining = selector.GetCooldownRemaining("anthropic/claude-haiku-4-5")
	if remaining != 0 {
		t.Errorf("expected 0 cooldown for model not in cooldown, got %v", remaining)
	}

	// Mark same model failed again - should increase backoff
	selector.MarkFailed("anthropic/claude-sonnet-4-5")
	remaining = selector.GetCooldownRemaining("anthropic/claude-sonnet-4-5")
	if remaining < 9*time.Second || remaining > 11*time.Second {
		t.Errorf("expected ~10s cooldown on second failure, got %v", remaining)
	}

	// Mark failed third time - should be ~20s
	selector.MarkFailed("anthropic/claude-sonnet-4-5")
	remaining = selector.GetCooldownRemaining("anthropic/claude-sonnet-4-5")
	if remaining < 19*time.Second || remaining > 21*time.Second {
		t.Errorf("expected ~20s cooldown on third failure, got %v", remaining)
	}
}

func TestSelectSkipsCooldownModels(t *testing.T) {
	config := &provider.ModelsConfig{
		TaskRouting: &provider.TaskRouting{
			General: "anthropic/claude-sonnet-4-5",
			Fallbacks: map[string][]string{
				"general": {"anthropic/claude-haiku-4-5"},
			},
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-sonnet-4-5"},
				{ID: "claude-haiku-4-5"},
			},
		},
	}

	selector := NewModelSelector(config)

	messages := []session.Message{
		{Role: "user", Content: "hello"},
	}

	// Initially selects primary
	model := selector.Select(messages)
	if model != "anthropic/claude-sonnet-4-5" {
		t.Errorf("expected primary model, got %s", model)
	}

	// Mark primary as failed (enters cooldown)
	selector.MarkFailed("anthropic/claude-sonnet-4-5")

	// Now should select fallback due to cooldown
	model = selector.Select(messages)
	if model != "anthropic/claude-haiku-4-5" {
		t.Errorf("expected fallback model when primary in cooldown, got %s", model)
	}
}

func TestGetCheapestModel(t *testing.T) {
	active := true
	config := &provider.ModelsConfig{
		Credentials: map[string]provider.ProviderCredentials{
			"anthropic": {APIKey: "test-key"},
			"openai":    {APIKey: "test-key"},
			// Note: deepseek has NO credentials - should be skipped
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{
					ID:      "claude-opus-4-5",
					Active:  &active,
					Pricing: &provider.ModelPricing{Input: 5, Output: 25},
				},
				{
					ID:      "claude-haiku-4-5",
					Active:  &active,
					Pricing: &provider.ModelPricing{Input: 1, Output: 5},
				},
			},
			"openai": {
				{
					ID:      "gpt-5.2",
					Active:  &active,
					Pricing: &provider.ModelPricing{Input: 1.75, Output: 14},
				},
				{
					ID:      "gpt-5-nano",
					Active:  &active,
					Pricing: &provider.ModelPricing{Input: 0.5, Output: 2},
					Kind:    []string{"cheap", "fast"},
				},
			},
			"deepseek": {
				{
					ID:      "deepseek-chat",
					Active:  &active,
					Pricing: &provider.ModelPricing{Input: 0.28, Output: 0.42},
					Kind:    []string{"cheap"},
				},
			},
		},
	}

	selector := NewModelSelector(config)
	cheapest := selector.GetCheapestModel()

	// DeepSeek has lowest pricing BUT no credentials configured
	// GPT-5-nano is cheapest with credentials (0.5 + 2*2 = 4.5)
	// Compare to Haiku (1 + 5*2 = 11)
	if cheapest != "openai/gpt-5-nano" {
		t.Errorf("expected openai/gpt-5-nano (cheapest with credentials), got %s", cheapest)
	}
}

func TestGetCheapestModel_FallbackToKind(t *testing.T) {
	active := true
	config := &provider.ModelsConfig{
		Credentials: map[string]provider.ProviderCredentials{
			"anthropic": {APIKey: "test-key"},
			"openai":    {APIKey: "test-key"},
		},
		Providers: map[string][]provider.ModelInfo{
			"anthropic": {
				{ID: "claude-opus-4-5", Active: &active},
				{ID: "claude-haiku-4-5", Active: &active, Kind: []string{"fast"}},
			},
			"openai": {
				{ID: "gpt-5-nano", Active: &active, Kind: []string{"cheap"}},
			},
		},
	}

	selector := NewModelSelector(config)
	cheapest := selector.GetCheapestModel()

	// No pricing available, should fall back to "cheap" kind tag
	if cheapest != "openai/gpt-5-nano" {
		t.Errorf("expected openai/gpt-5-nano (has 'cheap' kind), got %s", cheapest)
	}
}
