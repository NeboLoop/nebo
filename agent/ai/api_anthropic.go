package ai

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"

	"gobot/agent/session"
)

const (
	anthropicAPIURL     = "https://api.anthropic.com/v1/messages"
	anthropicAPIVersion = "2023-06-01"
	defaultMaxTokens    = 8192
)

// debugAI enables verbose AI request/response logging
var debugAI = os.Getenv("GOBOT_DEBUG_AI") != ""

func logDebug(format string, args ...interface{}) {
	if debugAI {
		fmt.Printf("[AI DEBUG] "+format+"\n", args...)
	}
}

// AnthropicProvider implements the Anthropic Claude API
type AnthropicProvider struct {
	apiKey string
	model  string
	client *http.Client
}

// NewAnthropicProvider creates a new Anthropic provider
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewAnthropicProvider(apiKey, model string) *AnthropicProvider {
	return &AnthropicProvider{
		apiKey: apiKey,
		model:  model,
		client: &http.Client{},
	}
}

// ID returns the provider identifier
func (p *AnthropicProvider) ID() string {
	return "anthropic"
}

// Stream sends a request and returns streaming events
func (p *AnthropicProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	// Convert to Anthropic format
	anthropicReq := p.buildRequest(req)

	body, err := json.Marshal(anthropicReq)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	// Always log key request info for debugging
	fmt.Printf("[Anthropic] Sending request to %s\n", anthropicAPIURL)
	fmt.Printf("[Anthropic] Model: %v, Messages: %d, HasTools: %v\n",
		anthropicReq["model"], len(req.Messages), len(req.Tools) > 0)

	httpReq, err := http.NewRequestWithContext(ctx, "POST", anthropicAPIURL, bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("x-api-key", p.apiKey)
	httpReq.Header.Set("anthropic-version", anthropicAPIVersion)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		fmt.Printf("[Anthropic] Request error: %v\n", err)
		return nil, fmt.Errorf("request failed: %w", err)
	}
	fmt.Printf("[Anthropic] Response: %d %s\n", resp.StatusCode, resp.Status)

	if resp.StatusCode != http.StatusOK {
		defer resp.Body.Close()
		body, _ := io.ReadAll(resp.Body)
		fmt.Printf("[Anthropic] Error body: %s\n", string(body))
		return nil, parseAnthropicError(resp.StatusCode, body)
	}

	events := make(chan StreamEvent, 100)
	go p.streamResponse(ctx, resp, events)

	return events, nil
}

// buildRequest converts ChatRequest to Anthropic API format
func (p *AnthropicProvider) buildRequest(req *ChatRequest) map[string]interface{} {
	messages := make([]map[string]interface{}, 0, len(req.Messages))

	for _, msg := range req.Messages {
		anthropicMsg := p.convertMessage(msg)
		if anthropicMsg != nil {
			messages = append(messages, anthropicMsg)
		}
	}

	// Use request model override if provided, otherwise use provider default
	model := p.model
	if req.Model != "" {
		model = req.Model
	}

	result := map[string]interface{}{
		"model":      model,
		"messages":   messages,
		"max_tokens": defaultMaxTokens,
		"stream":     true,
	}

	if req.MaxTokens > 0 {
		result["max_tokens"] = req.MaxTokens
	}

	if req.System != "" {
		result["system"] = req.System
	}

	if len(req.Tools) > 0 {
		tools := make([]map[string]interface{}, 0, len(req.Tools))
		for _, tool := range req.Tools {
			tools = append(tools, map[string]interface{}{
				"name":         tool.Name,
				"description":  tool.Description,
				"input_schema": json.RawMessage(tool.InputSchema),
			})
		}
		result["tools"] = tools
	}

	// Enable extended thinking mode for reasoning tasks
	if req.EnableThinking {
		result["thinking"] = map[string]interface{}{
			"type":         "enabled",
			"budget_tokens": 10000, // Allow up to 10K tokens for thinking
		}
		// Increase max tokens to accommodate thinking output
		if req.MaxTokens <= 0 {
			result["max_tokens"] = 16384
		}
	}

	return result
}

// convertMessage converts a session message to Anthropic format
func (p *AnthropicProvider) convertMessage(msg session.Message) map[string]interface{} {
	switch msg.Role {
	case "user":
		return map[string]interface{}{
			"role":    "user",
			"content": msg.Content,
		}
	case "assistant":
		content := make([]interface{}, 0)

		// Add text content if present
		if msg.Content != "" {
			content = append(content, map[string]interface{}{
				"type": "text",
				"text": msg.Content,
			})
		}

		// Add tool calls if present
		if len(msg.ToolCalls) > 0 {
			var toolCalls []session.ToolCall
			if err := json.Unmarshal(msg.ToolCalls, &toolCalls); err == nil {
				for _, tc := range toolCalls {
					content = append(content, map[string]interface{}{
						"type":  "tool_use",
						"id":    tc.ID,
						"name":  tc.Name,
						"input": json.RawMessage(tc.Input),
					})
				}
			}
		}

		if len(content) == 0 {
			return nil
		}

		return map[string]interface{}{
			"role":    "assistant",
			"content": content,
		}
	case "tool":
		// Tool results
		if len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
				content := make([]interface{}, 0, len(results))
				for _, r := range results {
					content = append(content, map[string]interface{}{
						"type":        "tool_result",
						"tool_use_id": r.ToolCallID,
						"content":     r.Content,
						"is_error":    r.IsError,
					})
				}
				return map[string]interface{}{
					"role":    "user",
					"content": content,
				}
			}
		}
		return nil
	case "system":
		// System messages are handled separately in Anthropic API
		return nil
	}
	return nil
}

// streamResponse reads SSE events and sends them to the channel
func (p *AnthropicProvider) streamResponse(ctx context.Context, resp *http.Response, events chan<- StreamEvent) {
	defer close(events)
	defer resp.Body.Close()

	logDebug("Anthropic starting stream response parsing...")
	reader := bufio.NewReader(resp.Body)
	var currentToolCall *ToolCall
	var inputBuffer strings.Builder
	eventCount := 0

	for {
		select {
		case <-ctx.Done():
			logDebug("Anthropic stream: context cancelled")
			events <- StreamEvent{Type: EventTypeError, Error: ctx.Err()}
			return
		default:
		}

		line, err := reader.ReadString('\n')
		if err != nil {
			if err == io.EOF {
				logDebug("Anthropic stream: EOF after %d events", eventCount)
				break
			}
			logDebug("Anthropic stream read error: %v", err)
			events <- StreamEvent{Type: EventTypeError, Error: err}
			return
		}

		line = strings.TrimSpace(line)
		if line == "" || !strings.HasPrefix(line, "data: ") {
			continue
		}

		data := strings.TrimPrefix(line, "data: ")
		if data == "[DONE]" {
			logDebug("Anthropic stream: received [DONE]")
			break
		}

		var event anthropicStreamEvent
		if err := json.Unmarshal([]byte(data), &event); err != nil {
			logDebug("Anthropic stream: failed to parse event: %v", err)
			continue
		}

		eventCount++
		logDebug("Anthropic stream event #%d: type=%s", eventCount, event.Type)

		switch event.Type {
		case "content_block_start":
			if event.ContentBlock.Type == "tool_use" {
				currentToolCall = &ToolCall{
					ID:   event.ContentBlock.ID,
					Name: event.ContentBlock.Name,
				}
				inputBuffer.Reset()
			}

		case "content_block_delta":
			if event.Delta.Type == "text_delta" {
				events <- StreamEvent{
					Type: EventTypeText,
					Text: event.Delta.Text,
				}
			} else if event.Delta.Type == "input_json_delta" {
				inputBuffer.WriteString(event.Delta.PartialJSON)
			} else if event.Delta.Type == "thinking_delta" {
				events <- StreamEvent{
					Type: EventTypeThinking,
					Text: event.Delta.Thinking,
				}
			}

		case "content_block_stop":
			if currentToolCall != nil {
				currentToolCall.Input = json.RawMessage(inputBuffer.String())
				events <- StreamEvent{
					Type:     EventTypeToolCall,
					ToolCall: currentToolCall,
				}
				currentToolCall = nil
			}

		case "message_stop":
			events <- StreamEvent{Type: EventTypeDone}
			return

		case "error":
			events <- StreamEvent{
				Type:  EventTypeError,
				Error: &ProviderError{Message: event.Error.Message, Type: event.Error.Type},
			}
			return
		}
	}

	events <- StreamEvent{Type: EventTypeDone}
}

// anthropicStreamEvent represents a streaming event from Anthropic
type anthropicStreamEvent struct {
	Type         string `json:"type"`
	Index        int    `json:"index,omitempty"`
	ContentBlock struct {
		Type  string          `json:"type,omitempty"`
		ID    string          `json:"id,omitempty"`
		Name  string          `json:"name,omitempty"`
		Input json.RawMessage `json:"input,omitempty"`
	} `json:"content_block,omitempty"`
	Delta struct {
		Type        string `json:"type,omitempty"`
		Text        string `json:"text,omitempty"`
		PartialJSON string `json:"partial_json,omitempty"`
		Thinking    string `json:"thinking,omitempty"`
	} `json:"delta,omitempty"`
	Error struct {
		Type    string `json:"type"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// parseAnthropicError parses an error response from the Anthropic API
func parseAnthropicError(statusCode int, body []byte) error {
	var errResp struct {
		Error struct {
			Type    string `json:"type"`
			Message string `json:"message"`
		} `json:"error"`
	}

	if err := json.Unmarshal(body, &errResp); err != nil {
		return &ProviderError{
			Code:    fmt.Sprintf("%d", statusCode),
			Message: string(body),
		}
	}

	code := errResp.Error.Type
	if statusCode == 429 {
		code = "rate_limit_exceeded"
	} else if statusCode == 401 {
		code = "authentication_error"
	}

	return &ProviderError{
		Code:    code,
		Type:    errResp.Error.Type,
		Message: errResp.Error.Message,
	}
}
