package runner

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"gobot/agent/ai"
	"gobot/agent/config"
	"gobot/agent/memory"
	"gobot/agent/session"
	"gobot/agent/skills"
	"gobot/agent/tools"
)

// DefaultSystemPrompt is the default system prompt for the agent
const DefaultSystemPrompt = `You are a helpful AI assistant with access to tools for file operations, shell commands, and more.

When working on tasks:
1. Break down complex tasks into smaller steps
2. Use tools to gather information and make changes
3. If you encounter errors, analyze them and try to fix them
4. When the task is complete, provide a summary of what was done

Important:
- Use the 'read' tool to read files instead of 'cat'
- Use the 'write' tool to create/modify files
- Use the 'glob' tool to find files by pattern
- Use the 'grep' tool to search for content in files
- Use the 'bash' tool for shell commands

Always verify your changes work before considering a task complete.`

// Runner executes the agentic loop
type Runner struct {
	sessions        *session.Manager
	providers       []ai.Provider
	providerMap     map[string]ai.Provider // providerID -> Provider for model-based switching
	tools           *tools.Registry
	config          *config.Config
	skillLoader     *skills.Loader
	memoryTool      *tools.MemoryTool
	autoExtract     bool
	selector        *ai.ModelSelector
	fuzzyMatcher    *ai.FuzzyMatcher // For user model switch requests
}

// RunRequest contains parameters for a run
type RunRequest struct {
	SessionKey    string // Session identifier (uses "default" if empty)
	Prompt        string // User prompt
	System        string // Override system prompt
	ModelOverride string // User-specified model override (e.g., "anthropic/claude-opus-4-5")
}

// New creates a new runner
func New(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, toolRegistry *tools.Registry) *Runner {
	// Load skills from extensions/skills directory (in working directory)
	// Also load from ~/.gobot/skills/ for user-installed skills
	skillLoader := skills.NewLoader(filepath.Join("extensions", "skills"))
	if err := skillLoader.LoadAll(); err != nil {
		// Log error but continue - skills are optional
		fmt.Printf("[runner] Warning: failed to load skills: %v\n", err)
	}

	// Also load user skills from ~/.gobot/skills/
	userSkillsDir := filepath.Join(cfg.DataDir, "skills")
	userSkillLoader := skills.NewLoader(userSkillsDir)
	if err := userSkillLoader.LoadAll(); err == nil {
		// Merge user skills into main loader
		for _, skill := range userSkillLoader.List() {
			skillLoader.Add(skill)
		}
	}

	// Load disabled skills from settings file (if exists)
	// This syncs the runner with UI-configured skill states
	disabledSkills := loadDisabledSkills(cfg.DataDir)
	if len(disabledSkills) > 0 {
		skillLoader.SetDisabledSkills(disabledSkills)
	}

	// Build provider map for model-based switching
	providerMap := make(map[string]ai.Provider)
	for _, p := range providers {
		providerID := p.ID()
		// Store first provider for each ID (highest priority since they're added in order)
		if _, exists := providerMap[providerID]; !exists {
			providerMap[providerID] = p
		}
	}

	return &Runner{
		sessions:    sessions,
		providers:   providers,
		providerMap: providerMap,
		tools:       toolRegistry,
		config:      cfg,
		skillLoader: skillLoader,
	}
}

// SetModelSelector sets the model selector for task-based model routing
func (r *Runner) SetModelSelector(selector *ai.ModelSelector) {
	r.selector = selector
}

// SetFuzzyMatcher sets the fuzzy matcher for user model switch requests
func (r *Runner) SetFuzzyMatcher(matcher *ai.FuzzyMatcher) {
	r.fuzzyMatcher = matcher
}

// loadDisabledSkills reads the skill-settings.json file and returns disabled skill names
func loadDisabledSkills(dataDir string) []string {
	settingsPath := filepath.Join(dataDir, "skill-settings.json")
	data, err := os.ReadFile(settingsPath)
	if err != nil {
		return nil
	}

	var settings struct {
		DisabledSkills []string `json:"disabledSkills"`
	}
	if err := json.Unmarshal(data, &settings); err != nil {
		return nil
	}

	return settings.DisabledSkills
}

// SetPolicy updates the tool registry's policy
func (r *Runner) SetPolicy(policy *tools.Policy) {
	r.tools.SetPolicy(policy)
}

// SetMemoryTool enables automatic memory extraction after conversations
func (r *Runner) SetMemoryTool(mt *tools.MemoryTool) {
	r.memoryTool = mt
	r.autoExtract = mt != nil
}

// SetAutoExtract enables or disables automatic memory extraction
func (r *Runner) SetAutoExtract(enabled bool) {
	r.autoExtract = enabled && r.memoryTool != nil
}

// SkillLoader returns the skill loader for managing skills
func (r *Runner) SkillLoader() *skills.Loader {
	return r.skillLoader
}

// Run executes the agentic loop
func (r *Runner) Run(ctx context.Context, req *RunRequest) (<-chan ai.StreamEvent, error) {
	if len(r.providers) == 0 {
		return nil, fmt.Errorf("no providers configured")
	}

	if req.SessionKey == "" {
		req.SessionKey = "default"
	}

	// Get or create session
	sess, err := r.sessions.GetOrCreate(req.SessionKey)
	if err != nil {
		return nil, fmt.Errorf("failed to get session: %w", err)
	}

	// Add user message to session
	if req.Prompt != "" {
		err = r.sessions.AppendMessage(sess.ID, session.Message{
			SessionID: sess.ID,
			Role:      "user",
			Content:   req.Prompt,
		})
		if err != nil {
			return nil, fmt.Errorf("failed to save message: %w", err)
		}
	}

	resultCh := make(chan ai.StreamEvent, 100)
	go r.runLoop(ctx, sess.ID, req.System, req.ModelOverride, resultCh)

	return resultCh, nil
}

// runLoop is the main agentic execution loop
func (r *Runner) runLoop(ctx context.Context, sessionID, systemPrompt, modelOverride string, resultCh chan<- ai.StreamEvent) {
	defer close(resultCh)

	if systemPrompt == "" {
		systemPrompt = DefaultSystemPrompt
	}

	// Load AGENTS.md and MEMORY.md and inject into system prompt
	workspaceDir, _ := os.Getwd()
	memoryFiles := memory.LoadMemoryFiles(workspaceDir)
	if !memoryFiles.IsEmpty() {
		formatted := memoryFiles.FormatForSystemPrompt()
		systemPrompt = systemPrompt + "\n\n---\n\n" + formatted
	}

	// Apply matching skills based on the user's last message
	if r.skillLoader != nil {
		// Get the last user message to match against skills
		messages, _ := r.sessions.GetMessages(sessionID, r.config.MaxContext)
		var lastUserInput string
		for i := len(messages) - 1; i >= 0; i-- {
			if messages[i].Role == "user" && messages[i].Content != "" {
				lastUserInput = messages[i].Content
				break
			}
		}
		if lastUserInput != "" {
			systemPrompt = r.skillLoader.ApplyMatchingSkills(systemPrompt, lastUserInput)
		}
	}

	iteration := 0
	maxIterations := r.config.MaxIterations
	if maxIterations <= 0 {
		maxIterations = 100
	}

	compactionAttempted := false

	// MAIN LOOP: Model selection + agentic execution
	for iteration < maxIterations {
		iteration++

		// Get session messages
		messages, err := r.sessions.GetMessages(sessionID, r.config.MaxContext)
		if err != nil {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
			return
		}

		// Check for user model switch request (e.g., "use claude", "switch to opus")
		userModelOverride := r.detectUserModelSwitch(messages)
		if userModelOverride != "" && modelOverride == "" {
			modelOverride = userModelOverride
		}

		// Select model and provider
		var provider ai.Provider
		var selectedModel string
		var modelName string

		// Use model override if provided, otherwise use selector
		if modelOverride != "" {
			selectedModel = modelOverride
			providerID, mn := ai.ParseModelID(modelOverride)
			modelName = mn
			if p, ok := r.providerMap[providerID]; ok {
				provider = p
			}
		} else if r.selector != nil {
			selectedModel = r.selector.Select(messages)
			if selectedModel != "" {
				providerID, mn := ai.ParseModelID(selectedModel)
				modelName = mn
				// Look up provider from map
				if p, ok := r.providerMap[providerID]; ok {
					provider = p
				}
			}
		}

		// Fall back to first provider if selector didn't find one
		if provider == nil && len(r.providers) > 0 {
			provider = r.providers[0]
		}

		if provider == nil {
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: fmt.Errorf("no provider available")}
			return
		}

		// Build chat request
		chatReq := &ai.ChatRequest{
			Messages: messages,
			Tools:    r.tools.List(),
			System:   systemPrompt,
			Model:    modelName,
		}

		// Auto-enable thinking mode for reasoning tasks when model supports it
		if r.selector != nil && selectedModel != "" {
			taskType := r.selector.ClassifyTask(messages)
			if taskType == ai.TaskTypeReasoning && r.selector.SupportsThinking(selectedModel) {
				chatReq.EnableThinking = true
			}
		}

		// Stream to AI provider
		fmt.Printf("[Runner] Calling provider.Stream: provider=%s model=%s\n", provider.ID(), chatReq.Model)
		events, err := provider.Stream(ctx, chatReq)
		fmt.Printf("[Runner] provider.Stream returned: events=%v err=%v\n", events != nil, err)

		if err != nil {
			if ai.IsContextOverflow(err) && !compactionAttempted {
				compactionAttempted = true
				// Compact session and retry
				summary := r.generateSummary(ctx, messages)
				if compactErr := r.sessions.Compact(sessionID, summary); compactErr == nil {
					continue // Retry with compacted session
				}
			}
			if ai.IsRateLimitOrAuth(err) {
				// Mark model as failed and try again with a different one
				if r.selector != nil && selectedModel != "" {
					r.selector.MarkFailed(selectedModel)
				}
				continue
			}
			resultCh <- ai.StreamEvent{Type: ai.EventTypeError, Error: err}
			return
		}

		// Process streaming events
		hasToolCalls := false
		var assistantContent strings.Builder
		var toolCalls []session.ToolCall

		for event := range events {
			// Forward event to caller
			resultCh <- event

			switch event.Type {
			case ai.EventTypeText:
				assistantContent.WriteString(event.Text)

			case ai.EventTypeToolCall:
				hasToolCalls = true
				toolCalls = append(toolCalls, session.ToolCall{
					ID:    event.ToolCall.ID,
					Name:  event.ToolCall.Name,
					Input: event.ToolCall.Input,
				})

			case ai.EventTypeError:
				return
			}
		}

		// Save assistant message
		if assistantContent.Len() > 0 || len(toolCalls) > 0 {
			var toolCallsJSON json.RawMessage
			if len(toolCalls) > 0 {
				toolCallsJSON, _ = json.Marshal(toolCalls)
			}

			r.sessions.AppendMessage(sessionID, session.Message{
				SessionID: sessionID,
				Role:      "assistant",
				Content:   assistantContent.String(),
				ToolCalls: toolCallsJSON,
			})
		}

		// Execute tool calls
		if hasToolCalls {
			var toolResults []session.ToolResult

			for _, tc := range toolCalls {
				result := r.tools.Execute(ctx, &ai.ToolCall{
					ID:    tc.ID,
					Name:  tc.Name,
					Input: tc.Input,
				})

				// Send tool result event
				resultCh <- ai.StreamEvent{
					Type: ai.EventTypeToolResult,
					Text: result.Content,
				}

				toolResults = append(toolResults, session.ToolResult{
					ToolCallID: tc.ID,
					Content:    result.Content,
					IsError:    result.IsError,
				})
			}

			// Save tool results
			toolResultsJSON, _ := json.Marshal(toolResults)
			r.sessions.AppendMessage(sessionID, session.Message{
				SessionID:   sessionID,
				Role:        "tool",
				ToolResults: toolResultsJSON,
			})

			// Continue to next iteration for more tool calls
			continue
		}

		// No tool calls - LLM decided task is complete
		// Run memory extraction in background
		go r.extractAndStoreMemories(sessionID)
		resultCh <- ai.StreamEvent{Type: ai.EventTypeDone}
		return
	}

	// Exhausted iterations
	resultCh <- ai.StreamEvent{
		Type:  ai.EventTypeError,
		Error: fmt.Errorf("reached maximum iterations (%d)", maxIterations),
	}
}

// generateSummary creates a summary of the conversation for compaction
func (r *Runner) generateSummary(_ context.Context, messages []session.Message) string {
	// Simple summary: just note that conversation was compacted
	var summary strings.Builder
	summary.WriteString("[Previous conversation summary]\n")

	// Extract key points from messages
	for _, msg := range messages {
		if msg.Role == "user" && msg.Content != "" {
			summary.WriteString("- User request: ")
			content := msg.Content
			if len(content) > 200 {
				content = content[:200] + "..."
			}
			summary.WriteString(content)
			summary.WriteString("\n")
		}
	}

	return summary.String()
}

// Chat is a convenience method for one-shot chat without tool use
func (r *Runner) Chat(ctx context.Context, prompt string) (string, error) {
	if len(r.providers) == 0 {
		return "", fmt.Errorf("no providers configured")
	}

	provider := r.providers[0]
	events, err := provider.Stream(ctx, &ai.ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: prompt},
		},
	})
	if err != nil {
		return "", err
	}

	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
		if event.Type == ai.EventTypeError {
			return result.String(), event.Error
		}
	}

	return result.String(), nil
}

// extractAndStoreMemories runs in background to extract facts from a completed conversation
func (r *Runner) extractAndStoreMemories(sessionID string) {
	if !r.autoExtract || r.memoryTool == nil || len(r.providers) == 0 {
		return
	}

	// Use background context (conversation may have ended)
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	// Get recent messages from session
	messages, err := r.sessions.GetMessages(sessionID, 50) // Last 50 messages
	if err != nil || len(messages) < 2 {
		return // Not enough conversation to extract from
	}

	// Create extractor and extract facts
	extractor := memory.NewExtractor(r.providers[0])
	facts, err := extractor.Extract(ctx, messages)
	if err != nil {
		fmt.Printf("[runner] Memory extraction failed: %v\n", err)
		return
	}

	if facts.IsEmpty() {
		return
	}

	// Store extracted facts
	entries := facts.FormatForStorage()
	stored := 0
	for _, entry := range entries {
		if err := r.memoryTool.StoreEntry(entry.Layer, entry.Namespace, entry.Key, entry.Value, entry.Tags); err != nil {
			fmt.Printf("[runner] Failed to store memory %s: %v\n", entry.Key, err)
		} else {
			stored++
		}
	}

	if stored > 0 {
		fmt.Printf("[runner] Auto-extracted %d memories from conversation\n", stored)
	}
}

// detectUserModelSwitch checks the last user message for model switch requests
// Returns the matched model ID or empty string if no switch requested
func (r *Runner) detectUserModelSwitch(messages []session.Message) string {
	if r.fuzzyMatcher == nil {
		return ""
	}

	// Get the last user message
	var lastUserMessage string
	for i := len(messages) - 1; i >= 0; i-- {
		if messages[i].Role == "user" && messages[i].Content != "" {
			lastUserMessage = messages[i].Content
			break
		}
	}

	if lastUserMessage == "" {
		return ""
	}

	// Check for model switch patterns like "use claude", "switch to opus"
	modelRequest := ai.ParseModelRequest(lastUserMessage)
	if modelRequest == "" {
		return ""
	}

	// Use fuzzy matcher to resolve the model name
	return r.fuzzyMatcher.Match(modelRequest)
}
