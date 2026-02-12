package advisors

import (
	"context"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/session"
)

// MaxAdvisors is the maximum number of advisors that can run concurrently.
// More voices = noise, not intelligence.
const MaxAdvisors = 5

// AdvisorTimeout is the maximum time to wait for all advisors to respond.
const AdvisorTimeout = 30 * time.Second

// Runner executes advisors in parallel and collects their responses.
// Advisors never talk to each other - they talk TO the main agent.
type Runner struct {
	loader   *Loader
	provider ai.Provider
}

// NewRunner creates an advisor runner
func NewRunner(loader *Loader, provider ai.Provider) *Runner {
	return &Runner{
		loader:   loader,
		provider: provider,
	}
}

// SetProvider updates the provider used for advisor execution
func (r *Runner) SetProvider(provider ai.Provider) {
	r.provider = provider
}

// Deliberate runs all enabled advisors in parallel on the given task.
// Returns a slice of responses (may be partial if some advisors timeout/fail).
// The task should be a clear description of what the main agent is about to do.
func (r *Runner) Deliberate(ctx context.Context, task string, recentMessages []session.Message) ([]Response, error) {
	advisors := r.loader.List()
	if len(advisors) == 0 {
		return nil, nil // No advisors configured
	}

	// Cap at MaxAdvisors
	if len(advisors) > MaxAdvisors {
		advisors = advisors[:MaxAdvisors]
	}

	fmt.Printf("[advisors] Starting deliberation with %d advisors for task: %s\n",
		len(advisors), truncateTask(task, 100))

	// Use max per-advisor timeout as the overall deadline
	maxTimeout := AdvisorTimeout
	for _, adv := range advisors {
		if adv.TimeoutSeconds > 0 {
			t := time.Duration(adv.TimeoutSeconds) * time.Second
			if t > maxTimeout {
				maxTimeout = t
			}
		}
	}
	ctx, cancel := context.WithTimeout(ctx, maxTimeout)
	defer cancel()

	// Run advisors in parallel
	var wg sync.WaitGroup
	responses := make([]Response, len(advisors))
	errors := make([]error, len(advisors))

	for i, advisor := range advisors {
		wg.Add(1)
		go func(idx int, adv *Advisor) {
			defer wg.Done()
			// Per-advisor timeout
			advCtx := ctx
			if adv.TimeoutSeconds > 0 {
				var advCancel context.CancelFunc
				advCtx, advCancel = context.WithTimeout(ctx, time.Duration(adv.TimeoutSeconds)*time.Second)
				defer advCancel()
			}
			resp, err := r.runAdvisor(advCtx, adv, task, recentMessages)
			if err != nil {
				errors[idx] = err
				fmt.Printf("[advisors] Advisor %s failed: %v\n", adv.Name, err)
			} else {
				responses[idx] = resp
				fmt.Printf("[advisors] Advisor %s responded (confidence: %d)\n",
					adv.Name, resp.Confidence)
			}
		}(i, advisor)
	}

	// Wait for all advisors to complete
	wg.Wait()

	// Collect successful responses
	var results []Response
	for i, resp := range responses {
		if errors[i] == nil && resp.Critique != "" {
			results = append(results, resp)
		}
	}

	fmt.Printf("[advisors] Deliberation complete: %d/%d advisors responded\n",
		len(results), len(advisors))

	return results, nil
}

// runAdvisor executes a single advisor and parses its response
func (r *Runner) runAdvisor(ctx context.Context, advisor *Advisor, task string, recentMessages []session.Message) (Response, error) {
	resp := Response{
		AdvisorName: advisor.Name,
		Role:        advisor.Role,
	}

	// Build the prompt for this advisor
	systemPrompt := advisor.BuildSystemPrompt(task)

	// Build messages: include recent context + the task
	messages := buildAdvisorMessages(recentMessages, task)

	// Call the provider
	events, err := r.provider.Stream(ctx, &ai.ChatRequest{
		System:   systemPrompt,
		Messages: messages,
	})
	if err != nil {
		return resp, fmt.Errorf("stream error: %w", err)
	}

	// Collect response
	var content strings.Builder
	for event := range events {
		switch event.Type {
		case ai.EventTypeText:
			content.WriteString(event.Text)
		case ai.EventTypeError:
			return resp, event.Error
		}
	}

	// Parse the response into structured format
	resp.Critique = content.String()
	resp.Confidence = extractConfidence(content.String())
	resp.Risks = extractSection(content.String(), "Risks")
	resp.Suggestion = extractSection(content.String(), "Suggestion")

	return resp, nil
}

// buildAdvisorMessages creates the message list for an advisor call
func buildAdvisorMessages(recentMessages []session.Message, task string) []session.Message {
	// Include a summary of recent context (last 3-5 messages for brevity)
	maxContext := 5
	if len(recentMessages) > maxContext {
		recentMessages = recentMessages[len(recentMessages)-maxContext:]
	}

	// Build context summary
	var contextSummary strings.Builder
	contextSummary.WriteString("Recent conversation context:\n\n")

	for _, msg := range recentMessages {
		switch msg.Role {
		case "user":
			contextSummary.WriteString(fmt.Sprintf("User: %s\n\n", truncateTask(msg.Content, 200)))
		case "assistant":
			if msg.Content != "" {
				contextSummary.WriteString(fmt.Sprintf("Assistant: %s\n\n", truncateTask(msg.Content, 200)))
			}
		}
	}

	return []session.Message{
		{
			Role:    "user",
			Content: contextSummary.String() + "\n---\n\nThe main agent is about to respond. Please provide your critique.",
		},
	}
}

// FormatForInjection formats advisor responses for injection into the system prompt
func FormatForInjection(responses []Response) string {
	if len(responses) == 0 {
		return ""
	}

	var sb strings.Builder
	sb.WriteString("\n\n---\n## Internal Deliberation (Advisor Perspectives)\n\n")
	sb.WriteString("Before responding, consider these internal perspectives:\n\n")

	for _, resp := range responses {
		sb.WriteString(fmt.Sprintf("### %s (%s)\n", resp.AdvisorName, resp.Role))
		sb.WriteString(resp.Critique)
		sb.WriteString("\n\n")
	}

	sb.WriteString("---\n\n")
	sb.WriteString("Synthesize these perspectives but make your own decision. You are the authority.\n")

	return sb.String()
}

// extractConfidence attempts to extract a confidence score (1-10) from the response
func extractConfidence(text string) int {
	// Look for patterns like "Confidence: 7" or "7/10"
	lines := strings.Split(text, "\n")
	for _, line := range lines {
		lower := strings.ToLower(line)
		if strings.Contains(lower, "confidence") {
			// Try to find a number
			for _, word := range strings.Fields(line) {
				word = strings.Trim(word, "():,*/")
				if len(word) == 1 && word[0] >= '1' && word[0] <= '9' {
					return int(word[0] - '0')
				}
				if len(word) == 2 && word[0] == '1' && word[1] == '0' {
					return 10
				}
			}
		}
	}
	return 5 // Default confidence if not found
}

// extractSection attempts to extract a named section from the response
func extractSection(text, sectionName string) string {
	lines := strings.Split(text, "\n")
	inSection := false
	var result strings.Builder

	for _, line := range lines {
		lower := strings.ToLower(line)
		if strings.Contains(lower, strings.ToLower(sectionName)) && strings.Contains(line, ":") {
			inSection = true
			// Get content after the colon on the same line
			parts := strings.SplitN(line, ":", 2)
			if len(parts) > 1 {
				content := strings.TrimSpace(parts[1])
				if content != "" {
					return content
				}
			}
			continue
		}

		if inSection {
			// Stop at next section header or empty line after content
			if strings.HasPrefix(line, "#") || strings.HasPrefix(line, "**") {
				break
			}
			if line == "" && result.Len() > 0 {
				break
			}
			if line != "" {
				result.WriteString(line)
				result.WriteString(" ")
			}
		}
	}

	return strings.TrimSpace(result.String())
}

// truncateTask truncates a task description to a maximum length
func truncateTask(task string, maxLen int) string {
	if len(task) <= maxLen {
		return task
	}
	return task[:maxLen-3] + "..."
}
