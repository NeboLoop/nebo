package ai

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/ollama/ollama/api"

	"github.com/neboloop/nebo/internal/agent/session"
)

// OllamaProvider implements the Provider interface for Ollama (local models) using the official SDK
type OllamaProvider struct {
	client *api.Client
	model  string
}

// NewOllamaProvider creates a new Ollama provider
func NewOllamaProvider(baseURL, model string) *OllamaProvider {
	if baseURL == "" {
		baseURL = "http://localhost:11434"
	}
	if model == "" {
		model = "qwen3:4b" // Default model
	}

	parsedURL, err := url.Parse(baseURL)
	if err != nil {
		parsedURL, _ = url.Parse("http://localhost:11434")
	}

	httpClient := &http.Client{
		Timeout: 5 * time.Minute, // Longer timeout for local inference
	}

	return &OllamaProvider{
		client: api.NewClient(parsedURL, httpClient),
		model:  model,
	}
}

// ID returns the provider identifier
// Must match the key used in models.yaml ("ollama")
func (p *OllamaProvider) ID() string {
	return "ollama"
}

// ProfileID returns empty - use ProfiledProvider wrapper for profile tracking
func (p *OllamaProvider) ProfileID() string {
	return ""
}

// HandlesTools returns false - the runner executes tools for API providers
func (p *OllamaProvider) HandlesTools() bool {
	return false
}

// Stream sends a request to Ollama and streams the response
func (p *OllamaProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	resultCh := make(chan StreamEvent, 100)

	// Build messages
	messages := p.buildMessages(req)

	// Use request model override if provided, otherwise use provider default
	model := p.model
	if req.Model != "" {
		model = req.Model
	}

	// Build request
	chatReq := &api.ChatRequest{
		Model:    model,
		Messages: messages,
	}

	// Enable streaming
	stream := true
	chatReq.Stream = &stream

	// Set options
	if req.Temperature > 0 || req.MaxTokens > 0 {
		chatReq.Options = make(map[string]any)
		if req.Temperature > 0 {
			chatReq.Options["temperature"] = req.Temperature
		}
		if req.MaxTokens > 0 {
			chatReq.Options["num_predict"] = req.MaxTokens
		}
	}

	// Add tools if present
	if len(req.Tools) > 0 {
		chatReq.Tools = p.buildTools(req.Tools)
	}

	fmt.Printf("[Ollama] Sending request: model=%s messages=%d tools=%d\n",
		model, len(messages), len(req.Tools))

	go func() {
		defer close(resultCh)

		toolCallCounter := 0

		err := p.client.Chat(ctx, chatReq, func(resp api.ChatResponse) error {
			// Stream text content
			if resp.Message.Content != "" {
				resultCh <- StreamEvent{
					Type: EventTypeText,
					Text: resp.Message.Content,
				}
			}

			// Handle tool calls
			if len(resp.Message.ToolCalls) > 0 {
				for _, tc := range resp.Message.ToolCalls {
					toolCallCounter++
					argsJSON, _ := json.Marshal(tc.Function.Arguments.ToMap())
					resultCh <- StreamEvent{
						Type: EventTypeToolCall,
						ToolCall: &ToolCall{
							ID:    fmt.Sprintf("ollama-call-%d", toolCallCounter),
							Name:  tc.Function.Name,
							Input: argsJSON,
						},
					}
				}
			}

			// Check if done
			if resp.Done {
				resultCh <- StreamEvent{Type: EventTypeDone}
			}

			return nil
		})

		if err != nil {
			fmt.Printf("[Ollama] Stream error: %v\n", err)
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: err,
			}
		}
	}()

	return resultCh, nil
}

// buildMessages converts session messages to Ollama format
func (p *OllamaProvider) buildMessages(req *ChatRequest) []api.Message {
	messages := make([]api.Message, 0, len(req.Messages)+1)

	// Add system message if present
	if req.System != "" {
		messages = append(messages, api.Message{
			Role:    "system",
			Content: req.System,
		})
	}

	// First pass: collect all tool_call IDs that have responses
	respondedToolIDs := make(map[string]bool)
	for _, msg := range req.Messages {
		if msg.Role == "tool" && len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
				for _, r := range results {
					respondedToolIDs[r.ToolCallID] = true
				}
			}
		}
	}

	// Convert session messages
	for _, msg := range req.Messages {
		switch msg.Role {
		case "user":
			messages = append(messages, api.Message{
				Role:    "user",
				Content: msg.Content,
			})

		case "assistant":
			assistantMsg := api.Message{
				Role:    "assistant",
				Content: msg.Content,
			}

			// Add tool calls if present (only those with responses)
			if len(msg.ToolCalls) > 0 {
				var toolCalls []session.ToolCall
				if err := json.Unmarshal(msg.ToolCalls, &toolCalls); err == nil {
					for _, tc := range toolCalls {
						// Only include tool calls that have responses
						if !respondedToolIDs[tc.ID] {
							fmt.Printf("[Ollama] Skipping tool_call without response: %s\n", tc.ID)
							continue
						}

						args := api.NewToolCallFunctionArguments()
						var argsMap map[string]any
						if err := json.Unmarshal(tc.Input, &argsMap); err == nil {
							for k, v := range argsMap {
								args.Set(k, v)
							}
						}

						assistantMsg.ToolCalls = append(assistantMsg.ToolCalls, api.ToolCall{
							ID: tc.ID,
							Function: api.ToolCallFunction{
								Name:      tc.Name,
								Arguments: args,
							},
						})
					}
				}
			}

			// Only add if has content or tool calls
			if assistantMsg.Content != "" || len(assistantMsg.ToolCalls) > 0 {
				messages = append(messages, assistantMsg)
			}

		case "system":
			messages = append(messages, api.Message{
				Role:    "system",
				Content: msg.Content,
			})

		case "tool":
			// Tool results
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					for _, r := range results {
						// Find the tool name for this result
						toolName := p.findToolName(r.ToolCallID, req.Messages)
						messages = append(messages, api.Message{
							Role:       "tool",
							Content:    r.Content,
							ToolCallID: r.ToolCallID,
							ToolName:   toolName,
						})
					}
				}
			}
		}
	}

	return messages
}

// findToolName finds the tool name from a tool call ID by searching messages
func (p *OllamaProvider) findToolName(toolCallID string, msgs []session.Message) string {
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

// buildTools converts tool definitions to Ollama format
func (p *OllamaProvider) buildTools(tools []ToolDefinition) api.Tools {
	result := make(api.Tools, 0, len(tools))

	for _, tool := range tools {
		var schemaRaw map[string]any
		if err := json.Unmarshal([]byte(tool.InputSchema), &schemaRaw); err != nil {
			continue
		}

		params := api.ToolFunctionParameters{
			Type: "object",
		}

		// Extract properties
		if props, ok := schemaRaw["properties"].(map[string]any); ok {
			propsMap := api.NewToolPropertiesMap()
			for name, propRaw := range props {
				if propObj, ok := propRaw.(map[string]any); ok {
					prop := p.convertProperty(propObj)
					propsMap.Set(name, prop)
				}
			}
			params.Properties = propsMap
		}

		// Extract required
		if required, ok := schemaRaw["required"].([]any); ok {
			for _, r := range required {
				if s, ok := r.(string); ok {
					params.Required = append(params.Required, s)
				}
			}
		}

		result = append(result, api.Tool{
			Type: "function",
			Function: api.ToolFunction{
				Name:        tool.Name,
				Description: tool.Description,
				Parameters:  params,
			},
		})
	}

	return result
}

// convertProperty converts a JSON schema property to Ollama format
func (p *OllamaProvider) convertProperty(prop map[string]any) api.ToolProperty {
	result := api.ToolProperty{}

	// Get type
	if typeVal, ok := prop["type"].(string); ok {
		result.Type = api.PropertyType{typeVal}
	}

	// Get description
	if desc, ok := prop["description"].(string); ok {
		result.Description = desc
	}

	// Get enum
	if enum, ok := prop["enum"].([]any); ok {
		result.Enum = enum
	}

	// Get items (for arrays)
	if items, ok := prop["items"]; ok {
		result.Items = items
	}

	return result
}

// CheckOllamaAvailable checks if Ollama is running
func CheckOllamaAvailable(baseURL string) bool {
	if baseURL == "" {
		baseURL = "http://localhost:11434"
	}

	client := &http.Client{Timeout: 2 * time.Second}
	resp, err := client.Get(baseURL + "/api/tags")
	if err != nil {
		return false
	}
	defer resp.Body.Close()

	return resp.StatusCode == http.StatusOK
}

// EnsureOllamaModel checks if a model exists locally and pulls it if not.
// This is non-blocking for the caller — it logs progress but doesn't stream.
// Returns nil if the model is already present or was pulled successfully.
func EnsureOllamaModel(baseURL, model string) error {
	if baseURL == "" {
		baseURL = "http://localhost:11434"
	}
	if model == "" {
		return nil
	}

	// Check if already present
	models, err := ListOllamaModels(baseURL)
	if err != nil {
		return fmt.Errorf("cannot list Ollama models: %w", err)
	}
	for _, m := range models {
		// Ollama returns "qwen3:4b" or "qwen3-embedding:latest" — match prefix
		if m == model || strings.HasPrefix(m, model+":") || strings.TrimSuffix(m, ":latest") == model {
			return nil // Already present
		}
	}

	// Model not found — pull it
	fmt.Printf("[Ollama] Model %s not found locally, pulling...\n", model)

	parsedURL, err := url.Parse(baseURL)
	if err != nil {
		return err
	}

	client := api.NewClient(parsedURL, &http.Client{Timeout: 30 * time.Minute})
	pullReq := &api.PullRequest{Model: model}

	var lastPct string
	err = client.Pull(context.Background(), pullReq, func(resp api.ProgressResponse) error {
		if resp.Total > 0 {
			pct := fmt.Sprintf("%d%%", resp.Completed*100/resp.Total)
			if pct != lastPct {
				lastPct = pct
				fmt.Printf("[Ollama] Pulling %s: %s\n", model, pct)
			}
		} else if resp.Status != "" {
			fmt.Printf("[Ollama] %s: %s\n", model, resp.Status)
		}
		return nil
	})
	if err != nil {
		return fmt.Errorf("failed to pull %s: %w", model, err)
	}

	fmt.Printf("[Ollama] Model %s ready\n", model)
	return nil
}

// ListOllamaModels returns available models
func ListOllamaModels(baseURL string) ([]string, error) {
	if baseURL == "" {
		baseURL = "http://localhost:11434"
	}

	parsedURL, err := url.Parse(baseURL)
	if err != nil {
		return nil, err
	}

	client := api.NewClient(parsedURL, &http.Client{Timeout: 5 * time.Second})
	resp, err := client.List(context.Background())
	if err != nil {
		return nil, err
	}

	models := make([]string, 0, len(resp.Models))
	for _, m := range resp.Models {
		models = append(models, m.Name)
	}

	return models, nil
}
