package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/nebolabs/nebo/internal/agent/advisors"
	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/embeddings"
	"github.com/nebolabs/nebo/internal/agent/session"
)

// AdvisorsTool allows the main agent to consult internal advisors for deliberation.
// Advisors run concurrently and provide concise counsel that the agent synthesizes.
type AdvisorsTool struct {
	loader        *advisors.Loader
	provider      ai.Provider
	providerMap   map[string]ai.Provider
	sessions      *session.Manager
	searcher      *embeddings.HybridSearcher
	currentUserID string
}

// AdvisorsInput defines the input for the advisors tool
type AdvisorsInput struct {
	// Task describes what the agent is deliberating on
	Task string `json:"task"`

	// SessionID is the current session for context (optional)
	SessionID string `json:"session_id,omitempty"`

	// Specific advisor names to consult (optional, defaults to all enabled)
	Advisors []string `json:"advisors,omitempty"`
}

// NewAdvisorsTool creates a new advisors tool
func NewAdvisorsTool(loader *advisors.Loader) *AdvisorsTool {
	return &AdvisorsTool{
		loader: loader,
	}
}

// SetProvider sets the provider used for advisor execution
func (t *AdvisorsTool) SetProvider(provider ai.Provider) {
	t.provider = provider
}

// SetProviderMap sets the provider map for selecting execution provider
func (t *AdvisorsTool) SetProviderMap(providerMap map[string]ai.Provider) {
	t.providerMap = providerMap
}

// SetSessionManager sets the session manager for context retrieval
func (t *AdvisorsTool) SetSessionManager(sessions *session.Manager) {
	t.sessions = sessions
}

// SetSearcher sets the hybrid searcher for memory-enabled advisors
func (t *AdvisorsTool) SetSearcher(searcher *embeddings.HybridSearcher) {
	t.searcher = searcher
}

// SetCurrentUser sets the user ID for user-scoped memory queries
func (t *AdvisorsTool) SetCurrentUser(userID string) {
	t.currentUserID = userID
}

// Name returns the tool name
func (t *AdvisorsTool) Name() string {
	return "advisors"
}

// Description returns a brief description
func (t *AdvisorsTool) Description() string {
	return "Consult internal advisors for deliberation on complex decisions"
}

// RequiresApproval returns false - advisors are internal deliberation
func (t *AdvisorsTool) RequiresApproval() bool {
	return false
}

// Schema returns the JSON schema for this tool
func (t *AdvisorsTool) Schema() json.RawMessage {
	// List available advisors in description
	var advisorList string
	if t.loader != nil {
		names := make([]string, 0)
		for _, adv := range t.loader.ListAll() {
			status := "enabled"
			if !adv.Enabled {
				status = "disabled"
			}
			names = append(names, fmt.Sprintf("%s (%s, %s)", adv.Name, adv.Role, status))
		}
		if len(names) > 0 {
			advisorList = " Available: " + strings.Join(names, ", ")
		}
	}

	schema := map[string]any{
		"type": "object",
		"properties": map[string]any{
			"task": map[string]any{
				"type":        "string",
				"description": "Description of what you're deliberating on. Be specific about the decision or problem.",
			},
			"session_id": map[string]any{
				"type":        "string",
				"description": "Current session ID for context (optional)",
			},
			"advisors": map[string]any{
				"type":        "array",
				"items":       map[string]any{"type": "string"},
				"description": "Specific advisor names to consult (optional, defaults to all enabled)." + advisorList,
			},
		},
		"required": []string{"task"},
	}

	data, _ := json.Marshal(schema)
	return data
}

// Execute runs the advisors tool
func (t *AdvisorsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var req AdvisorsInput
	if err := json.Unmarshal(input, &req); err != nil {
		return &ToolResult{Content: fmt.Sprintf("invalid input: %v", err), IsError: true}, nil
	}

	if req.Task == "" {
		return &ToolResult{Content: "task is required", IsError: true}, nil
	}

	if t.loader == nil {
		return &ToolResult{Content: "advisors not configured", IsError: true}, nil
	}

	if t.provider == nil {
		return &ToolResult{Content: "no provider configured for advisors", IsError: true}, nil
	}

	// Get advisors to consult
	var advisorsToRun []*advisors.Advisor
	if len(req.Advisors) > 0 {
		// Use specified advisors
		for _, name := range req.Advisors {
			if adv, ok := t.loader.Get(name); ok && adv.Enabled {
				advisorsToRun = append(advisorsToRun, adv)
			}
		}
	} else {
		// Use all enabled advisors
		advisorsToRun = t.loader.List()
	}

	if len(advisorsToRun) == 0 {
		return &ToolResult{Content: "no advisors available (check advisors/ directory in Nebo data folder)", IsError: true}, nil
	}

	// Cap at 5 advisors max
	if len(advisorsToRun) > 5 {
		advisorsToRun = advisorsToRun[:5]
	}

	// Get recent context if session provided
	var recentMessages []session.Message
	if req.SessionID != "" && t.sessions != nil {
		msgs, err := t.sessions.GetMessages(req.SessionID, 10)
		if err == nil {
			recentMessages = msgs
		}
	}

	fmt.Printf("[advisors] Consulting %d advisors on: %s\n", len(advisorsToRun), truncateForLog(req.Task, 80))

	// Run advisors concurrently with timeout
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	responses := t.runAdvisorsConcurrently(ctx, advisorsToRun, req.Task, recentMessages)

	// Format responses for the agent
	result := t.formatResponses(responses)

	return &ToolResult{
		Content: result,
		IsError: false,
	}, nil
}

// runAdvisorsConcurrently executes all advisors in parallel and collects responses
func (t *AdvisorsTool) runAdvisorsConcurrently(ctx context.Context, advs []*advisors.Advisor, task string, recentMessages []session.Message) []advisorResponse {
	var wg sync.WaitGroup
	responses := make([]advisorResponse, len(advs))

	for i, adv := range advs {
		wg.Add(1)
		go func(idx int, advisor *advisors.Advisor) {
			defer wg.Done()
			resp := t.runSingleAdvisor(ctx, advisor, task, recentMessages)
			responses[idx] = resp
		}(i, adv)
	}

	wg.Wait()
	return responses
}

type advisorResponse struct {
	Name       string
	Role       string
	Counsel    string
	Confidence int
	Error      error
}

// runSingleAdvisor executes one advisor and returns its response
func (t *AdvisorsTool) runSingleAdvisor(ctx context.Context, adv *advisors.Advisor, task string, recentMessages []session.Message) advisorResponse {
	resp := advisorResponse{
		Name: adv.Name,
		Role: adv.Role,
	}

	// Build system prompt with advisor persona + conciseness instruction
	systemPrompt := fmt.Sprintf(`%s

---

## Your Task

Provide counsel on the following:

%s

---

## Response Requirements

BE CONCISE. Your counsel should be:
- 2-4 sentences maximum for your main point
- Direct and actionable
- No fluff, no hedging

Format:
**Counsel**: [Your main advice in 2-4 sentences]
**Confidence**: [1-10]
**Key Risk**: [One sentence, optional]`, adv.Persona, task)

	// Build context from memory (if enabled) and recent messages
	var contextSummary string

	// Inject persistent memory for advisors with memory_access: true
	if adv.MemoryAccess && t.searcher != nil {
		results, err := t.searcher.Search(ctx, task, embeddings.SearchOptions{
			Limit:  5,
			UserID: t.currentUserID,
		})
		if err == nil && len(results) > 0 {
			var sb strings.Builder
			sb.WriteString("Relevant memories:\n")
			for _, r := range results {
				value := r.Value
				if len(value) > 200 {
					value = value[:200] + "..."
				}
				sb.WriteString(fmt.Sprintf("- %s: %s (score: %.2f)\n", r.Key, value, r.Score))
			}
			sb.WriteString("\n")
			contextSummary = sb.String()
		}
	}

	if len(recentMessages) > 0 {
		var sb strings.Builder
		sb.WriteString("Recent context:\n")
		// Only last 3 messages for conciseness
		start := len(recentMessages) - 3
		if start < 0 {
			start = 0
		}
		for _, msg := range recentMessages[start:] {
			if msg.Role == "user" && msg.Content != "" {
				sb.WriteString(fmt.Sprintf("- User: %s\n", truncateForLog(msg.Content, 150)))
			}
		}
		contextSummary += sb.String()
	}

	messages := []session.Message{
		{
			Role:    "user",
			Content: contextSummary + "\nProvide your counsel now.",
		},
	}

	// Call provider
	events, err := t.provider.Stream(ctx, &ai.ChatRequest{
		System:   systemPrompt,
		Messages: messages,
	})
	if err != nil {
		resp.Error = err
		return resp
	}

	// Collect response
	var content strings.Builder
	for event := range events {
		switch event.Type {
		case ai.EventTypeText:
			content.WriteString(event.Text)
		case ai.EventTypeError:
			resp.Error = event.Error
			return resp
		}
	}

	resp.Counsel = content.String()
	resp.Confidence = extractConfidenceFromText(content.String())

	fmt.Printf("[advisors] %s responded (confidence: %d)\n", adv.Name, resp.Confidence)
	return resp
}

// formatResponses formats advisor responses for the agent
func (t *AdvisorsTool) formatResponses(responses []advisorResponse) string {
	var sb strings.Builder
	sb.WriteString("## Advisor Counsel\n\n")

	successCount := 0
	for _, resp := range responses {
		if resp.Error != nil {
			sb.WriteString(fmt.Sprintf("### %s (%s)\n", resp.Name, resp.Role))
			sb.WriteString(fmt.Sprintf("*Error: %v*\n\n", resp.Error))
			continue
		}

		if resp.Counsel == "" {
			continue
		}

		successCount++
		sb.WriteString(fmt.Sprintf("### %s (%s)\n", resp.Name, resp.Role))
		sb.WriteString(resp.Counsel)
		sb.WriteString("\n\n")
	}

	if successCount == 0 {
		return "No advisors provided counsel. Proceed with your own judgment."
	}

	sb.WriteString("---\n\n")
	sb.WriteString("*Synthesize these perspectives. You are the decision-maker.*")

	return sb.String()
}

// extractConfidenceFromText attempts to extract confidence score from response
func extractConfidenceFromText(text string) int {
	lines := strings.Split(text, "\n")
	for _, line := range lines {
		lower := strings.ToLower(line)
		if strings.Contains(lower, "confidence") {
			for _, word := range strings.Fields(line) {
				word = strings.Trim(word, "():,*/[]")
				if len(word) == 1 && word[0] >= '1' && word[0] <= '9' {
					return int(word[0] - '0')
				}
				if word == "10" {
					return 10
				}
			}
		}
	}
	return 5
}

// truncateForLog truncates text for logging
func truncateForLog(text string, maxLen int) string {
	text = strings.ReplaceAll(text, "\n", " ")
	if len(text) <= maxLen {
		return text
	}
	return text[:maxLen-3] + "..."
}
