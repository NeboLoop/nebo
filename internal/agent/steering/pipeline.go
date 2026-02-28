// Package steering provides mid-conversation steering for the agent.
//
// Steering messages are ephemeral — generated fresh each agentic loop iteration,
// injected into the message array sent to the LLM, but never persisted to SQLite
// and never shown to the user. They guide model behavior during long conversations.
package steering

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/session"
)

// Position controls where a steering message is injected in the conversation.
type Position int

const (
	// PositionEnd appends after all messages (most common).
	PositionEnd Position = iota
	// PositionAfterUser inserts after the last user message.
	PositionAfterUser
)

// Message is an ephemeral steering message. Never persisted. Never shown to the user.
type Message struct {
	Content  string
	Position Position
}

// WorkTask mirrors tools.WorkTask — kept here to avoid circular imports.
type WorkTask struct {
	ID      string `json:"id"`
	Subject string `json:"subject"`
	Status  string `json:"status"` // pending, in_progress, completed
}

// Context carries everything generators need to make decisions.
type Context struct {
	SessionID      string
	Messages       []session.Message
	UserPrompt     string            // Current user input (empty on tool-result turns)
	ActiveTask     string            // Pinned active task (from session metadata)
	Channel        string            // "web", "cli", "telegram", "discord", "slack"
	AgentName      string            // User-configured agent name
	Iteration      int               // Current agentic loop iteration (1-based)
	RunStartTime   time.Time         // When this Run() call started
	WorkTasks      []WorkTask        // In-memory work tracking tasks (from AgentDomainTool)
	JanusRateLimit *ai.RateLimitInfo // Latest Janus rate-limit info (may be nil)
}

// Generator produces zero or more steering messages for the current turn.
type Generator interface {
	Name() string
	Generate(ctx *Context) []Message
}

// Pipeline runs all registered generators and collects steering messages.
type Pipeline struct {
	generators []Generator
}

// New creates a pipeline with all default generators registered.
func New() *Pipeline {
	return &Pipeline{
		generators: []Generator{
			&identityGuard{},
			&channelAdapter{},
			&toolNudge{},
			&dateTimeRefresh{},
			&memoryNudge{},
			&objectiveTaskNudge{},
			&pendingTaskAction{},
			&taskProgress{},
			&janusQuotaWarning{},
		},
	}
}

// Generate runs all generators and returns collected steering messages.
// Each generator is wrapped in recover — a panicking generator never crashes the pipeline.
func (p *Pipeline) Generate(ctx *Context) []Message {
	var msgs []Message
	for _, g := range p.generators {
		result := p.safeGenerate(g, ctx)
		if len(result) > 0 {
			// Replace {agent_name} placeholder in all messages
			for i := range result {
				result[i].Content = strings.ReplaceAll(result[i].Content, "{agent_name}", ctx.AgentName)
			}
			msgs = append(msgs, result...)
		}
	}
	if len(msgs) > 0 {
		fmt.Printf("[Steering] Generated %d messages from %d generators\n", len(msgs), len(p.generators))
	}
	return msgs
}

// safeGenerate wraps a generator call with panic recovery.
func (p *Pipeline) safeGenerate(g Generator, ctx *Context) (result []Message) {
	defer func() {
		if r := recover(); r != nil {
			fmt.Printf("[Steering] Generator %q panicked: %v\n", g.Name(), r)
			result = nil
		}
	}()
	return g.Generate(ctx)
}

// Inject merges steering messages into the conversation message array.
// Steering messages become user-role messages with content wrapped in <steering> tags.
func Inject(messages []session.Message, steering []Message) []session.Message {
	if len(steering) == 0 {
		return messages
	}

	// Separate by position
	var endMsgs, afterUserMsgs []Message
	for _, m := range steering {
		switch m.Position {
		case PositionAfterUser:
			afterUserMsgs = append(afterUserMsgs, m)
		default:
			endMsgs = append(endMsgs, m)
		}
	}

	// Build result with injections
	result := make([]session.Message, 0, len(messages)+len(steering))

	if len(afterUserMsgs) > 0 {
		// Find last user message index
		lastUserIdx := -1
		for i := len(messages) - 1; i >= 0; i-- {
			if messages[i].Role == "user" {
				lastUserIdx = i
				break
			}
		}

		for i, msg := range messages {
			result = append(result, msg)
			if i == lastUserIdx {
				for _, sm := range afterUserMsgs {
					result = append(result, toSessionMessage(sm))
				}
			}
		}
	} else {
		result = append(result, messages...)
	}

	// Append end-position messages
	for _, sm := range endMsgs {
		result = append(result, toSessionMessage(sm))
	}

	return result
}

// toSessionMessage converts a steering Message to a session.Message.
func toSessionMessage(m Message) session.Message {
	return session.Message{
		Role:    "user",
		Content: m.Content,
	}
}

// --- Helpers for generators ---

// countAssistantTurns counts assistant messages in the conversation.
func countAssistantTurns(messages []session.Message) int {
	count := 0
	for _, m := range messages {
		if m.Role == "assistant" && m.Content != "" {
			count++
		}
	}
	return count
}

// countTurnsSinceToolUse counts assistant turns since the last tool call
// whose name contains the given prefix. The turn that made the call is NOT counted.
// Returns -1 if never used.
func countTurnsSinceToolUse(messages []session.Message, nameContains string) int {
	turns := 0
	for i := len(messages) - 1; i >= 0; i-- {
		m := messages[i]
		if m.Role == "assistant" {
			// Check if this assistant message has tool calls matching the prefix
			if len(m.ToolCalls) > 0 {
				var calls []struct {
					Name string `json:"name"`
				}
				if err := json.Unmarshal(m.ToolCalls, &calls); err == nil {
					for _, c := range calls {
						if strings.Contains(c.Name, nameContains) {
							return turns
						}
					}
				}
			}
			if m.Content != "" {
				turns++
			}
		}
	}
	return -1 // Never used
}

// countTurnsSinceAnyToolUse counts assistant turns since the last tool call of any kind.
// The assistant turn that made the tool call is NOT counted.
// Returns -1 if no tool calls found.
func countTurnsSinceAnyToolUse(messages []session.Message) int {
	turns := 0
	for i := len(messages) - 1; i >= 0; i-- {
		m := messages[i]
		if m.Role == "assistant" {
			if len(m.ToolCalls) > 0 {
				return turns
			}
			if m.Content != "" {
				turns++
			}
		}
	}
	return -1
}

// lastNUserMessagesContain checks if any of the last N user messages contain
// any of the given patterns (case-insensitive).
func lastNUserMessagesContain(messages []session.Message, n int, patterns []string) bool {
	found := 0
	for i := len(messages) - 1; i >= 0 && found < n; i-- {
		m := messages[i]
		if m.Role != "user" {
			continue
		}
		found++
		lower := strings.ToLower(m.Content)
		for _, p := range patterns {
			if strings.Contains(lower, strings.ToLower(p)) {
				return true
			}
		}
	}
	return false
}
