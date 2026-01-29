package memory

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"gobot/agent/ai"
	"gobot/agent/session"
)

// ExtractedFacts contains facts extracted from a conversation
type ExtractedFacts struct {
	Preferences []Fact `json:"preferences"` // User preferences and behaviors
	Entities    []Fact `json:"entities"`    // People, places, things mentioned
	Decisions   []Fact `json:"decisions"`   // Decisions made during conversation
}

// Fact represents a single extracted fact
type Fact struct {
	Key      string   `json:"key"`      // Unique key for storage
	Value    string   `json:"value"`    // The fact content
	Category string   `json:"category"` // Category (preference, entity, decision)
	Tags     []string `json:"tags"`     // Additional tags
}

// ExtractFactsPrompt is the prompt used to extract facts from messages
const ExtractFactsPrompt = `Analyze the following conversation and extract durable facts that should be remembered long-term.

Return a JSON object with three arrays:
1. "preferences" - User preferences and learned behaviors (e.g., code style, favorite tools, communication preferences)
2. "entities" - Information about people, places, projects mentioned (format key as "type/name", e.g., "person/sarah", "project/gobot")
3. "decisions" - Important decisions made during this conversation

Each fact should have:
- "key": A unique, descriptive key for retrieval (use path-like format: "category/name")
- "value": The actual information to remember
- "category": One of "preference", "entity", "decision"
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
	// Try to extract JSON from the response (in case there's extra text)
	jsonStart := strings.Index(responseText, "{")
	jsonEnd := strings.LastIndex(responseText, "}")
	if jsonStart >= 0 && jsonEnd > jsonStart {
		responseText = responseText[jsonStart : jsonEnd+1]
	}

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

	return entries
}

// MemoryEntry represents an entry ready for storage
type MemoryEntry struct {
	Layer     string
	Namespace string
	Key       string
	Value     string
	Tags      []string
}

// IsEmpty returns true if no facts were extracted
func (f *ExtractedFacts) IsEmpty() bool {
	return len(f.Preferences) == 0 && len(f.Entities) == 0 && len(f.Decisions) == 0
}

// TotalCount returns the total number of facts
func (f *ExtractedFacts) TotalCount() int {
	return len(f.Preferences) + len(f.Entities) + len(f.Decisions)
}
