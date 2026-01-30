package ai

import (
	"encoding/base64"
	"encoding/json"
	"strings"
	"sync"
	"time"

	"nebo/agent/session"
	"nebo/internal/provider"
)

// TaskType represents the type of task being performed
type TaskType string

const (
	TaskTypeVision    TaskType = "vision"
	TaskTypeAudio     TaskType = "audio"
	TaskTypeReasoning TaskType = "reasoning"
	TaskTypeCode      TaskType = "code"
	TaskTypeGeneral   TaskType = "general"
)

// modelCooldownState tracks failure state for a model
type modelCooldownState struct {
	failedAt      time.Time
	failureCount  int
	cooldownUntil time.Time
}

// ModelSelector selects the best model based on task type and available models
type ModelSelector struct {
	config     *provider.ModelsConfig
	excludedMu sync.RWMutex
	excluded   map[string]bool // Models that have failed and should be skipped
	cooldownMu sync.RWMutex
	cooldowns  map[string]*modelCooldownState // modelID -> cooldown state
}

// NewModelSelector creates a new model selector
func NewModelSelector(config *provider.ModelsConfig) *ModelSelector {
	return &ModelSelector{
		config:    config,
		excluded:  make(map[string]bool),
		cooldowns: make(map[string]*modelCooldownState),
	}
}

// Select returns the best model ID for the given messages
// Returns format: "provider/model" (e.g., "anthropic/claude-sonnet-4-5")
func (s *ModelSelector) Select(messages []session.Message) string {
	taskType := s.classifyTask(messages)
	return s.selectForTask(taskType)
}

// SelectWithExclusions returns the best model, excluding specified models
func (s *ModelSelector) SelectWithExclusions(messages []session.Message, excludeModels []string) string {
	taskType := s.classifyTask(messages)
	return s.selectForTaskWithExclusions(taskType, excludeModels)
}

// MarkFailed marks a model as failed with exponential backoff cooldown
func (s *ModelSelector) MarkFailed(modelID string) {
	s.excludedMu.Lock()
	s.excluded[modelID] = true
	s.excludedMu.Unlock()

	s.cooldownMu.Lock()
	defer s.cooldownMu.Unlock()

	state := s.cooldowns[modelID]
	if state == nil {
		state = &modelCooldownState{}
		s.cooldowns[modelID] = state
	}

	state.failureCount++
	state.failedAt = time.Now()

	// Exponential backoff: 5s, 10s, 20s, 40s, 80s... max 1 hour
	backoffSeconds := 5 << (state.failureCount - 1) // 5, 10, 20, 40, 80, 160...
	if backoffSeconds > 3600 {
		backoffSeconds = 3600 // Max 1 hour
	}
	state.cooldownUntil = time.Now().Add(time.Duration(backoffSeconds) * time.Second)
}

// isInCooldown checks if a model is in cooldown period
func (s *ModelSelector) isInCooldown(modelID string) bool {
	s.cooldownMu.RLock()
	defer s.cooldownMu.RUnlock()

	state := s.cooldowns[modelID]
	if state == nil {
		return false
	}
	return time.Now().Before(state.cooldownUntil)
}

// GetCooldownRemaining returns the remaining cooldown time for a model (0 if not in cooldown)
func (s *ModelSelector) GetCooldownRemaining(modelID string) time.Duration {
	s.cooldownMu.RLock()
	defer s.cooldownMu.RUnlock()

	state := s.cooldowns[modelID]
	if state == nil {
		return 0
	}
	remaining := time.Until(state.cooldownUntil)
	if remaining < 0 {
		return 0
	}
	return remaining
}

// ClearFailed clears all failed model markers and cooldowns
func (s *ModelSelector) ClearFailed() {
	s.excludedMu.Lock()
	s.excluded = make(map[string]bool)
	s.excludedMu.Unlock()

	s.cooldownMu.Lock()
	s.cooldowns = make(map[string]*modelCooldownState)
	s.cooldownMu.Unlock()
}

// classifyTask determines the task type from the messages
func (s *ModelSelector) classifyTask(messages []session.Message) TaskType {
	// Check for vision task (image content)
	if s.hasImageContent(messages) {
		return TaskTypeVision
	}

	// Check for audio task (audio content)
	if s.hasAudioContent(messages) {
		return TaskTypeAudio
	}

	// Get the last user message for keyword analysis
	var lastUserMessage string
	for i := len(messages) - 1; i >= 0; i-- {
		if messages[i].Role == "user" && messages[i].Content != "" {
			lastUserMessage = strings.ToLower(messages[i].Content)
			break
		}
	}

	if lastUserMessage == "" {
		return TaskTypeGeneral
	}

	// Check for audio-related task by keywords
	if s.isAudioTask(lastUserMessage) {
		return TaskTypeAudio
	}

	// Check for reasoning task
	if s.isReasoningTask(lastUserMessage) {
		return TaskTypeReasoning
	}

	// Check for code task
	if s.isCodeTask(lastUserMessage) {
		return TaskTypeCode
	}

	return TaskTypeGeneral
}

// hasImageContent checks if any message contains image data
func (s *ModelSelector) hasImageContent(messages []session.Message) bool {
	for _, msg := range messages {
		if msg.Role != "user" {
			continue
		}

		// Check if content is a JSON array (multimodal content)
		content := strings.TrimSpace(msg.Content)
		if strings.HasPrefix(content, "[") {
			var parts []map[string]interface{}
			if err := json.Unmarshal([]byte(content), &parts); err == nil {
				for _, part := range parts {
					if partType, ok := part["type"].(string); ok && partType == "image" {
						return true
					}
					// Check for image_url format (OpenAI style)
					if partType, ok := part["type"].(string); ok && partType == "image_url" {
						return true
					}
				}
			}
		}

		// Check for base64 image data patterns
		if strings.Contains(content, "data:image/") {
			return true
		}

		// Check if content looks like base64-encoded image
		if len(content) > 1000 && isBase64Image(content) {
			return true
		}
	}

	return false
}

// isBase64Image checks if a string appears to be a base64-encoded image
func isBase64Image(s string) bool {
	// Remove whitespace
	s = strings.ReplaceAll(s, "\n", "")
	s = strings.ReplaceAll(s, " ", "")

	// Check if it starts with base64 image prefix
	if strings.HasPrefix(s, "data:image/") {
		return true
	}

	// Try to decode a sample to see if it's valid base64
	if len(s) > 100 {
		sample := s[:100]
		_, err := base64.StdEncoding.DecodeString(sample)
		return err == nil
	}

	return false
}

// hasAudioContent checks if any message contains audio data
func (s *ModelSelector) hasAudioContent(messages []session.Message) bool {
	for _, msg := range messages {
		if msg.Role != "user" {
			continue
		}

		content := strings.TrimSpace(msg.Content)

		// Check if content is a JSON array (multimodal content)
		if strings.HasPrefix(content, "[") {
			var parts []map[string]any
			if err := json.Unmarshal([]byte(content), &parts); err == nil {
				for _, part := range parts {
					if partType, ok := part["type"].(string); ok {
						if partType == "audio" || partType == "input_audio" {
							return true
						}
					}
				}
			}
		}

		// Check for base64 audio data patterns
		if strings.Contains(content, "data:audio/") {
			return true
		}
	}

	return false
}

// isAudioTask checks if the message indicates an audio-related task
func (s *ModelSelector) isAudioTask(msg string) bool {
	audioKeywords := []string{
		"transcribe",
		"transcription",
		"audio",
		"voice",
		"speech to text",
		"speech-to-text",
		"text to speech",
		"text-to-speech",
		"tts",
		"stt",
		"dictation",
		"recording",
		"podcast",
		"listen to",
		"voice memo",
		"voice note",
	}

	for _, kw := range audioKeywords {
		if strings.Contains(msg, kw) {
			return true
		}
	}

	return false
}

// isReasoningTask checks if the message indicates a reasoning task
func (s *ModelSelector) isReasoningTask(msg string) bool {
	reasoningKeywords := []string{
		"think through",
		"analyze",
		"prove",
		"complex",
		"step by step",
		"reasoning",
		"logical",
		"deduce",
		"infer",
		"evaluate",
		"compare and contrast",
		"weigh the options",
		"consider all",
		"philosophical",
		"mathematical proof",
		"derive",
		"formalize",
	}

	for _, kw := range reasoningKeywords {
		if strings.Contains(msg, kw) {
			return true
		}
	}

	return false
}

// isCodeTask checks if the message indicates a code task
func (s *ModelSelector) isCodeTask(msg string) bool {
	codeKeywords := []string{
		"code",
		"function",
		"implement",
		"refactor",
		"debug",
		"fix the bug",
		"write a program",
		"create a script",
		"programming",
		"algorithm",
		"class",
		"method",
		"variable",
		"syntax",
		"compile",
		"runtime",
		"api",
		"endpoint",
		"database query",
		"sql",
		"javascript",
		"python",
		"golang",
		"typescript",
		"react",
		"vue",
		"html",
		"css",
	}

	for _, kw := range codeKeywords {
		if strings.Contains(msg, kw) {
			return true
		}
	}

	return false
}

// selectForTask returns the best model for a task type
func (s *ModelSelector) selectForTask(taskType TaskType) string {
	return s.selectForTaskWithExclusions(taskType, nil)
}

// selectForTaskWithExclusions returns the best model for a task type, excluding specified models
func (s *ModelSelector) selectForTaskWithExclusions(taskType TaskType, excludeModels []string) string {
	excluded := make(map[string]bool)
	for _, m := range excludeModels {
		excluded[m] = true
	}

	// Also add session-level excluded models
	s.excludedMu.RLock()
	for m := range s.excluded {
		excluded[m] = true
	}
	s.excludedMu.RUnlock()

	// Helper to check if model is available (not excluded, not in cooldown, is active)
	isUsable := func(modelID string) bool {
		if excluded[modelID] {
			return false
		}
		if s.isInCooldown(modelID) {
			return false
		}
		return s.isModelAvailable(modelID)
	}

	// Get task routing config
	routing := s.config.TaskRouting
	if routing == nil {
		// Fall back to defaults
		return s.getDefaultModelFiltered(isUsable)
	}

	// Get primary model for task type
	var primary string
	switch taskType {
	case TaskTypeVision:
		primary = routing.Vision
	case TaskTypeAudio:
		primary = routing.Audio
	case TaskTypeReasoning:
		primary = routing.Reasoning
	case TaskTypeCode:
		primary = routing.Code
	default:
		primary = routing.General
	}

	// Try primary if usable
	if primary != "" && isUsable(primary) {
		return primary
	}

	// Try fallbacks for this task type
	if routing.Fallbacks != nil {
		fallbacks := routing.Fallbacks[string(taskType)]
		for _, fb := range fallbacks {
			if isUsable(fb) {
				return fb
			}
		}
	}

	// Fall back to general routing
	if taskType != TaskTypeGeneral && routing.General != "" && isUsable(routing.General) {
		return routing.General
	}

	// Last resort: use defaults
	return s.getDefaultModelFiltered(isUsable)
}

// getDefaultModel returns the default model, respecting exclusions (legacy)
func (s *ModelSelector) getDefaultModel(excluded map[string]bool) string {
	isUsable := func(modelID string) bool {
		return !excluded[modelID] && s.isModelAvailable(modelID)
	}
	return s.getDefaultModelFiltered(isUsable)
}

// getDefaultModelFiltered returns the default model using a custom filter function
func (s *ModelSelector) getDefaultModelFiltered(isUsable func(string) bool) string {
	if s.config.Defaults == nil {
		return ""
	}

	// Try primary
	if s.config.Defaults.Primary != "" && isUsable(s.config.Defaults.Primary) {
		return s.config.Defaults.Primary
	}

	// Try fallbacks
	for _, fb := range s.config.Defaults.Fallbacks {
		if isUsable(fb) {
			return fb
		}
	}

	return ""
}

// isModelAvailable checks if a model is configured, active, AND has credentials
func (s *ModelSelector) isModelAvailable(modelID string) bool {
	parts := strings.SplitN(modelID, "/", 2)
	if len(parts) != 2 {
		return false
	}

	providerID := parts[0]
	modelName := parts[1]

	// Check if provider has credentials configured
	if s.config.Credentials != nil {
		creds, ok := s.config.Credentials[providerID]
		if !ok {
			return false
		}
		// Provider needs API key, base URL (Ollama), or command (CLI)
		if creds.APIKey == "" && creds.BaseURL == "" && creds.Command == "" {
			return false
		}
	}

	models, ok := s.config.Providers[providerID]
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

// GetModelInfo returns the model info for a given model ID
func (s *ModelSelector) GetModelInfo(modelID string) *provider.ModelInfo {
	parts := strings.SplitN(modelID, "/", 2)
	if len(parts) != 2 {
		return nil
	}

	providerID := parts[0]
	modelName := parts[1]

	models, ok := s.config.Providers[providerID]
	if !ok {
		return nil
	}

	for _, m := range models {
		if m.ID == modelName {
			return &m
		}
	}

	return nil
}

// SupportsThinking returns true if the model supports extended thinking mode
func (s *ModelSelector) SupportsThinking(modelID string) bool {
	info := s.GetModelInfo(modelID)
	if info != nil {
		for _, cap := range info.Capabilities {
			if cap == "thinking" || cap == "reasoning" || cap == "extended_thinking" {
				return true
			}
		}
	}

	// Known thinking-capable models (by name pattern)
	lowerID := strings.ToLower(modelID)
	return strings.Contains(lowerID, "opus") ||
		strings.Contains(lowerID, "o1") ||
		strings.Contains(lowerID, "o3")
}

// ClassifyTask exposes task classification for external use
func (s *ModelSelector) ClassifyTask(messages []session.Message) TaskType {
	return s.classifyTask(messages)
}

// GetCheapestModel returns the cheapest active model based on pricing
// Only considers API-based providers (excludes CLI providers like claude-cli)
// Falls back to models with "cheap" kind tag if no pricing is available
func (s *ModelSelector) GetCheapestModel() string {
	var cheapest string
	var cheapestCost float64 = -1

	// Helper to check if provider has API credentials (not CLI)
	// CLI providers are excluded because they're not suitable for background tasks
	isAPIProvider := func(providerID string) bool {
		if s.config.Credentials == nil {
			// No credentials section - check if it's a known CLI provider
			return !provider.IsCLIProvider(providerID)
		}
		creds, ok := s.config.Credentials[providerID]
		if !ok {
			return false
		}
		// Only API providers (have API key or base URL, NOT command-based CLI)
		return creds.APIKey != "" || creds.BaseURL != ""
	}

	// First pass: find cheapest by pricing (only from API providers)
	for providerID, models := range s.config.Providers {
		if !isAPIProvider(providerID) {
			continue
		}

		for _, m := range models {
			if !m.IsActive() {
				continue
			}

			modelID := providerID + "/" + m.ID

			// Check if model has pricing
			if m.Pricing != nil && (m.Pricing.Input > 0 || m.Pricing.Output > 0) {
				// Use combined cost (weighted toward output since extraction generates more output)
				cost := m.Pricing.Input + m.Pricing.Output*2
				if cheapestCost < 0 || cost < cheapestCost {
					cheapestCost = cost
					cheapest = modelID
				}
			}
		}
	}

	// If we found a model with pricing, return it
	if cheapest != "" {
		return cheapest
	}

	// Second pass: find model with "cheap" kind tag
	for providerID, models := range s.config.Providers {
		if !isAPIProvider(providerID) {
			continue
		}

		for _, m := range models {
			if !m.IsActive() {
				continue
			}

			for _, kind := range m.Kind {
				if strings.ToLower(kind) == "cheap" {
					return providerID + "/" + m.ID
				}
			}
		}
	}

	// Third pass: find model with "fast" kind tag (usually cheaper)
	for providerID, models := range s.config.Providers {
		if !isAPIProvider(providerID) {
			continue
		}

		for _, m := range models {
			if !m.IsActive() {
				continue
			}

			for _, kind := range m.Kind {
				if strings.ToLower(kind) == "fast" {
					return providerID + "/" + m.ID
				}
			}
		}
	}

	// Last resort: return first active model from a provider with credentials
	for providerID, models := range s.config.Providers {
		if !isAPIProvider(providerID) {
			continue
		}

		for _, m := range models {
			if m.IsActive() {
				return providerID + "/" + m.ID
			}
		}
	}

	return ""
}

// ParseModelID splits a model ID into provider and model parts
func ParseModelID(modelID string) (providerID, modelName string) {
	parts := strings.SplitN(modelID, "/", 2)
	if len(parts) == 2 {
		return parts[0], parts[1]
	}
	return "", modelID
}
