package ai

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/google/generative-ai-go/genai"
	"google.golang.org/api/iterator"
	"google.golang.org/api/option"

	"nebo/agent/session"
)

// GeminiProvider implements the Provider interface for Google Gemini using the official SDK
type GeminiProvider struct {
	apiKey string
	model  string
}

// NewGeminiProvider creates a new Gemini provider
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewGeminiProvider(apiKey, model string) *GeminiProvider {
	return &GeminiProvider{
		apiKey: apiKey,
		model:  model,
	}
}

// ID returns the provider identifier
// Must match the key used in models.yaml ("google")
func (p *GeminiProvider) ID() string {
	return "google"
}

// Stream sends a request to Gemini and streams the response
func (p *GeminiProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	// Create client
	client, err := genai.NewClient(ctx, option.WithAPIKey(p.apiKey))
	if err != nil {
		return nil, fmt.Errorf("failed to create Gemini client: %w", err)
	}

	// Use request model override if provided, otherwise use provider default
	modelName := p.model
	if req.Model != "" {
		modelName = req.Model
	}

	// Create model
	model := client.GenerativeModel(modelName)

	// Set system instruction if present
	if req.System != "" {
		model.SystemInstruction = &genai.Content{
			Parts: []genai.Part{genai.Text(req.System)},
		}
	}

	// Set generation config
	if req.Temperature > 0 {
		temp := float32(req.Temperature)
		model.Temperature = &temp
	}
	if req.MaxTokens > 0 {
		maxTokens := int32(req.MaxTokens)
		model.MaxOutputTokens = &maxTokens
	}

	// Add tools if present
	if len(req.Tools) > 0 {
		funcs := make([]*genai.FunctionDeclaration, 0, len(req.Tools))
		for _, tool := range req.Tools {
			schema := p.convertJSONSchemaToGenAI(tool.InputSchema)
			funcs = append(funcs, &genai.FunctionDeclaration{
				Name:        tool.Name,
				Description: tool.Description,
				Parameters:  schema,
			})
		}
		model.Tools = []*genai.Tool{{FunctionDeclarations: funcs}}
	}

	// Build history and get current message
	history, currentParts := p.buildHistory(req.Messages)

	// Start chat session
	cs := model.StartChat()
	cs.History = history

	fmt.Printf("[Gemini] Sending request: model=%s history=%d tools=%d\n",
		modelName, len(history), len(req.Tools))

	// Create streaming request
	iter := cs.SendMessageStream(ctx, currentParts...)

	events := make(chan StreamEvent, 100)
	go p.handleStream(client, iter, events)

	return events, nil
}

// buildHistory converts session messages to Gemini format
// Returns history (all messages except last user message) and current parts
func (p *GeminiProvider) buildHistory(msgs []session.Message) ([]*genai.Content, []genai.Part) {
	// First pass: collect all tool_call IDs that have responses
	respondedToolIDs := make(map[string]bool)
	for _, msg := range msgs {
		if msg.Role == "tool" && len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
				for _, r := range results {
					respondedToolIDs[r.ToolCallID] = true
				}
			}
		}
	}

	var contents []*genai.Content

	for _, msg := range msgs {
		switch msg.Role {
		case "user":
			contents = append(contents, &genai.Content{
				Role:  "user",
				Parts: []genai.Part{genai.Text(msg.Content)},
			})

		case "assistant":
			var parts []genai.Part

			// Add text content if present
			if msg.Content != "" {
				parts = append(parts, genai.Text(msg.Content))
			}

			// Add function calls if present (only those with responses)
			if len(msg.ToolCalls) > 0 {
				var toolCalls []session.ToolCall
				if err := json.Unmarshal(msg.ToolCalls, &toolCalls); err == nil {
					for _, tc := range toolCalls {
						// Only include tool calls that have responses
						if !respondedToolIDs[tc.ID] {
							fmt.Printf("[Gemini] Skipping tool_call without response: %s\n", tc.ID)
							continue
						}

						var args map[string]any
						if err := json.Unmarshal(tc.Input, &args); err != nil {
							args = map[string]any{}
						}
						parts = append(parts, genai.FunctionCall{
							Name: tc.Name,
							Args: args,
						})
					}
				}
			}

			if len(parts) > 0 {
				contents = append(contents, &genai.Content{
					Role:  "model",
					Parts: parts,
				})
			}

		case "tool":
			// Tool results as function responses
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					var parts []genai.Part
					for _, r := range results {
						// Extract the tool name from the ID if we stored it
						// Tool IDs are like "gemini-call-1", but we need the function name
						// We'll use the content as the response
						parts = append(parts, genai.FunctionResponse{
							Name: p.extractToolName(r.ToolCallID, msgs),
							Response: map[string]any{
								"result": r.Content,
							},
						})
					}
					if len(parts) > 0 {
						contents = append(contents, &genai.Content{
							Role:  "user",
							Parts: parts,
						})
					}
				}
			}

		case "system":
			// System messages handled via model.SystemInstruction
			continue
		}
	}

	// Normalize: ensure alternating turns
	contents = p.normalizeContents(contents)

	// Split into history and current message
	if len(contents) == 0 {
		return nil, []genai.Part{genai.Text("Hello")}
	}

	// Last message should be for sending (if it's user)
	if len(contents) > 0 {
		last := contents[len(contents)-1]
		if last.Role == "user" {
			return contents[:len(contents)-1], last.Parts
		}
	}

	// If last message is model, send empty to continue
	return contents, []genai.Part{genai.Text("Continue.")}
}

// extractToolName finds the tool name from a tool call ID by searching messages
func (p *GeminiProvider) extractToolName(toolCallID string, msgs []session.Message) string {
	for _, msg := range msgs {
		if msg.Role == "assistant" && len(msg.ToolCalls) > 0 {
			var calls []session.ToolCall
			if err := json.Unmarshal(msg.ToolCalls, &calls); err == nil {
				for _, c := range calls {
					if c.ID == toolCallID {
						return c.Name
					}
				}
			}
		}
	}
	return "unknown"
}

// normalizeContents ensures proper alternating turns for Gemini
func (p *GeminiProvider) normalizeContents(contents []*genai.Content) []*genai.Content {
	if len(contents) == 0 {
		return contents
	}

	normalized := make([]*genai.Content, 0, len(contents))
	var lastRole string

	for _, c := range contents {
		// Gemini requires starting with user
		if len(normalized) == 0 && c.Role != "user" {
			// Prepend a minimal user message
			normalized = append(normalized, &genai.Content{
				Role:  "user",
				Parts: []genai.Part{genai.Text("Continue.")},
			})
		}

		// Merge consecutive same-role messages
		if c.Role == lastRole && len(normalized) > 0 {
			last := normalized[len(normalized)-1]
			last.Parts = append(last.Parts, c.Parts...)
		} else {
			normalized = append(normalized, c)
			lastRole = c.Role
		}
	}

	return normalized
}

// handleStream processes the streaming response
func (p *GeminiProvider) handleStream(client *genai.Client, iter *genai.GenerateContentResponseIterator, events chan<- StreamEvent) {
	defer close(events)
	defer client.Close()

	toolCallCounter := 0

	for {
		resp, err := iter.Next()
		if err == iterator.Done {
			events <- StreamEvent{Type: EventTypeDone}
			return
		}
		if err != nil {
			fmt.Printf("[Gemini] Stream error: %v\n", err)
			events <- StreamEvent{
				Type:  EventTypeError,
				Error: err,
			}
			return
		}

		for _, candidate := range resp.Candidates {
			if candidate.Content == nil {
				continue
			}

			for _, part := range candidate.Content.Parts {
				switch v := part.(type) {
				case genai.Text:
					if string(v) != "" {
						events <- StreamEvent{
							Type: EventTypeText,
							Text: string(v),
						}
					}

				case genai.FunctionCall:
					toolCallCounter++
					argsJSON, _ := json.Marshal(v.Args)
					events <- StreamEvent{
						Type: EventTypeToolCall,
						ToolCall: &ToolCall{
							ID:    fmt.Sprintf("gemini-call-%d", toolCallCounter),
							Name:  v.Name,
							Input: argsJSON,
						},
					}
				}
			}

			// Check finish reason
			if candidate.FinishReason == genai.FinishReasonStop ||
				candidate.FinishReason == genai.FinishReasonMaxTokens {
				events <- StreamEvent{Type: EventTypeDone}
				return
			}
		}
	}
}

// convertJSONSchemaToGenAI converts JSON Schema to genai.Schema
func (p *GeminiProvider) convertJSONSchemaToGenAI(schemaJSON json.RawMessage) *genai.Schema {
	var raw map[string]any
	if err := json.Unmarshal(schemaJSON, &raw); err != nil {
		return &genai.Schema{Type: genai.TypeObject}
	}

	return p.convertSchemaObject(raw)
}

// convertSchemaObject recursively converts a JSON schema object to genai.Schema
func (p *GeminiProvider) convertSchemaObject(obj map[string]any) *genai.Schema {
	schema := &genai.Schema{}

	// Get type
	if typeStr, ok := obj["type"].(string); ok {
		switch strings.ToLower(typeStr) {
		case "string":
			schema.Type = genai.TypeString
		case "number":
			schema.Type = genai.TypeNumber
		case "integer":
			schema.Type = genai.TypeInteger
		case "boolean":
			schema.Type = genai.TypeBoolean
		case "array":
			schema.Type = genai.TypeArray
		case "object":
			schema.Type = genai.TypeObject
		default:
			schema.Type = genai.TypeString
		}
	} else {
		schema.Type = genai.TypeObject
	}

	// Get description
	if desc, ok := obj["description"].(string); ok {
		schema.Description = desc
	}

	// Get enum
	if enum, ok := obj["enum"].([]any); ok {
		for _, v := range enum {
			if s, ok := v.(string); ok {
				schema.Enum = append(schema.Enum, s)
			}
		}
	}

	// Get properties (for objects)
	if props, ok := obj["properties"].(map[string]any); ok {
		schema.Properties = make(map[string]*genai.Schema)
		for name, propRaw := range props {
			if propObj, ok := propRaw.(map[string]any); ok {
				schema.Properties[name] = p.convertSchemaObject(propObj)
			}
		}
	}

	// Get required
	if required, ok := obj["required"].([]any); ok {
		for _, v := range required {
			if s, ok := v.(string); ok {
				schema.Required = append(schema.Required, s)
			}
		}
	}

	// Get items (for arrays)
	if items, ok := obj["items"].(map[string]any); ok {
		schema.Items = p.convertSchemaObject(items)
	}

	return schema
}
