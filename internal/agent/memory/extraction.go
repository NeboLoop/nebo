package memory

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/session"
)

// ExtractedFacts contains facts extracted from a conversation
type ExtractedFacts struct {
	Preferences []Fact `json:"preferences"` // User preferences and behaviors
	Entities    []Fact `json:"entities"`    // People, places, things mentioned
	Decisions   []Fact `json:"decisions"`   // Decisions made during conversation
	Styles      []Fact `json:"styles"`      // Communication/personality style observations
	Artifacts   []Fact `json:"artifacts"`   // Content the agent produced that the user may reference later
}

// Fact represents a single extracted fact
type Fact struct {
	Key      string   `json:"key"`      // Unique key for storage
	Value    string   `json:"value"`    // The fact content
	Category string   `json:"category"` // Category (preference, entity, decision)
	Tags     []string `json:"tags"`     // Additional tags
}

// UnmarshalJSON handles both string and non-string values for flexible LLM parsing
func (f *Fact) UnmarshalJSON(data []byte) error {
	// Use an alias to avoid infinite recursion
	type FactAlias struct {
		Key      string          `json:"key"`
		Value    json.RawMessage `json:"value"`
		Category string          `json:"category"`
		Tags     []string        `json:"tags"`
	}

	var alias FactAlias
	if err := json.Unmarshal(data, &alias); err != nil {
		return err
	}

	f.Key = alias.Key
	f.Category = alias.Category
	f.Tags = alias.Tags

	// Try to unmarshal Value as string first
	var strVal string
	if err := json.Unmarshal(alias.Value, &strVal); err == nil {
		f.Value = strVal
		return nil
	}

	// If not a string, convert the raw JSON to string representation
	f.Value = strings.Trim(string(alias.Value), "\"")
	return nil
}

// ExtractFactsPrompt is the prompt used to extract facts from messages
const ExtractFactsPrompt = `Analyze the following conversation and extract durable facts that should be remembered long-term.

Return a JSON object with five arrays:
1. "preferences" - User preferences and learned behaviors (e.g., code style, favorite tools, communication preferences)
2. "entities" - Information about people, places, projects mentioned (format key as "type/name", e.g., "person/sarah", "project/nebo")
3. "decisions" - Important decisions made during this conversation
4. "styles" - Observations about how the user communicates or how they want the assistant to behave. These are emergent personality signals — things like humor preferences, directness level, topic engagement patterns, emotional tone, or pacing. Key format: "style/trait-name" (e.g., "style/humor-dry", "style/prefers-terse-responses", "style/engages-deeply-on-architecture"). Only include clear, repeated signals — not one-off moments.
5. "artifacts" - Important content the assistant produced that the user may reference later. This includes: copy/text written for the user (headlines, taglines, marketing copy, emails), plans or strategies outlined, specific recommendations given, code architecture decisions explained, or any creative output the user accepted or built on. Key format: "artifact/description" (e.g., "artifact/landing-page-hero-copy", "artifact/launch-strategy-summary"). Store the VERBATIM text or a precise summary — not a vague description.

Each fact should have:
- "key": A unique, descriptive key for retrieval (use path-like format: "category/name")
- "value": The actual information to remember
- "category": One of "preference", "entity", "decision", "style", "artifact"
- "tags": Relevant tags for searching

Skip:
- Greetings and casual chat
- Temporary or time-sensitive information
- Technical details that are already in code
- Information that can be easily looked up

Only include facts that would be valuable to remember in future conversations.

Conversation to analyze:
%s

Respond ONLY with valid JSON, no other text.`

// Extractor extracts facts from conversations
type Extractor struct {
	provider ai.Provider
}

// NewExtractor creates a new fact extractor
func NewExtractor(provider ai.Provider) *Extractor {
	return &Extractor{provider: provider}
}

// Extract extracts facts from a conversation
func (e *Extractor) Extract(ctx context.Context, messages []session.Message) (*ExtractedFacts, error) {
	if len(messages) == 0 {
		return &ExtractedFacts{}, nil
	}

	// Build conversation text
	var conv strings.Builder
	for _, msg := range messages {
		if msg.Content != "" {
			conv.WriteString(fmt.Sprintf("[%s]: %s\n\n", msg.Role, msg.Content))
		}
	}

	// Get AI to extract facts
	prompt := fmt.Sprintf(ExtractFactsPrompt, conv.String())

	events, err := e.provider.Stream(ctx, &ai.ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: prompt},
		},
	})
	if err != nil {
		return nil, fmt.Errorf("failed to stream: %w", err)
	}

	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
		if event.Type == ai.EventTypeError {
			return nil, event.Error
		}
	}

	// Parse the response
	responseText := strings.TrimSpace(result.String())

	// Empty response = no facts to extract (common for trivial conversations)
	if responseText == "" {
		return &ExtractedFacts{}, nil
	}

	// Strip markdown code fences if present (```json ... ``` or ``` ... ```)
	if strings.HasPrefix(responseText, "```") {
		// Find the end of the opening fence line
		if idx := strings.Index(responseText, "\n"); idx != -1 {
			responseText = responseText[idx+1:]
		}
		// Remove closing fence
		if idx := strings.LastIndex(responseText, "```"); idx != -1 {
			responseText = responseText[:idx]
		}
		responseText = strings.TrimSpace(responseText)
	}

	// Strip any remaining backticks (inline code formatting)
	responseText = strings.Trim(responseText, "`")
	responseText = strings.TrimSpace(responseText)

	// Try to extract FIRST JSON object from the response
	// (CLI provider may emit duplicates from both assistant and result messages)
	jsonStart := strings.Index(responseText, "{")
	if jsonStart < 0 {
		// No JSON object in response — LLM returned prose instead of JSON
		// (e.g., "No facts to extract from this conversation")
		return &ExtractedFacts{}, nil
	}

	// Find matching closing brace for the first object
	braceCount := 0
	jsonEnd := -1
	for i := jsonStart; i < len(responseText); i++ {
		if responseText[i] == '{' {
			braceCount++
		} else if responseText[i] == '}' {
			braceCount--
			if braceCount == 0 {
				jsonEnd = i
				break
			}
		}
	}
	if jsonEnd > jsonStart {
		responseText = responseText[jsonStart : jsonEnd+1]
	} else {
		// Unbalanced braces — malformed JSON
		return &ExtractedFacts{}, nil
	}

	// Final cleanup - remove any backticks that might be embedded
	responseText = strings.ReplaceAll(responseText, "```json", "")
	responseText = strings.ReplaceAll(responseText, "```", "")
	responseText = strings.ReplaceAll(responseText, "`", "")
	responseText = strings.TrimSpace(responseText)

	var facts ExtractedFacts
	if err := json.Unmarshal([]byte(responseText), &facts); err != nil {
		return nil, fmt.Errorf("failed to parse extracted facts: %w", err)
	}

	return &facts, nil
}

// FormatForStorage returns facts formatted for the memory tool
func (f *ExtractedFacts) FormatForStorage() []MemoryEntry {
	var entries []MemoryEntry
	today := time.Now().Format("2006-01-02")

	for _, pref := range f.Preferences {
		entries = append(entries, MemoryEntry{
			Layer:     "tacit",
			Namespace: "preferences",
			Key:       pref.Key,
			Value:     pref.Value,
			Tags:      append(pref.Tags, "preference"),
		})
	}

	for _, entity := range f.Entities {
		entries = append(entries, MemoryEntry{
			Layer:     "entity",
			Namespace: "default",
			Key:       entity.Key,
			Value:     entity.Value,
			Tags:      append(entity.Tags, "entity"),
		})
	}

	for _, decision := range f.Decisions {
		entries = append(entries, MemoryEntry{
			Layer:     "daily",
			Namespace: today,
			Key:       decision.Key,
			Value:     decision.Value,
			Tags:      append(decision.Tags, "decision"),
		})
	}

	for _, style := range f.Styles {
		entries = append(entries, MemoryEntry{
			Layer:     "tacit",
			Namespace: "personality",
			Key:       style.Key,
			Value:     style.Value,
			Tags:      append(style.Tags, "style"),
			IsStyle:   true,
		})
	}

	for _, artifact := range f.Artifacts {
		entries = append(entries, MemoryEntry{
			Layer:     "tacit",
			Namespace: "artifacts",
			Key:       artifact.Key,
			Value:     artifact.Value,
			Tags:      append(artifact.Tags, "artifact"),
		})
	}

	return entries
}

// MemoryEntry represents an entry ready for storage
type MemoryEntry struct {
	Layer     string
	Namespace string
	Key       string
	Value     string
	Tags      []string
	IsStyle   bool // Style observations use reinforcement tracking instead of overwrite
}

// IsEmpty returns true if no facts were extracted
func (f *ExtractedFacts) IsEmpty() bool {
	return len(f.Preferences) == 0 && len(f.Entities) == 0 && len(f.Decisions) == 0 && len(f.Styles) == 0 && len(f.Artifacts) == 0
}

// TotalCount returns the total number of facts
func (f *ExtractedFacts) TotalCount() int {
	return len(f.Preferences) + len(f.Entities) + len(f.Decisions) + len(f.Styles) + len(f.Artifacts)
}
