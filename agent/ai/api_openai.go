package ai

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"

	"gobot/agent/session"
)

const (
	openaiAPIURL = "https://api.openai.com/v1/chat/completions"
)

// OpenAIProvider implements the OpenAI API
type OpenAIProvider struct {
	apiKey string
	model  string
	client *http.Client
}

// NewOpenAIProvider creates a new OpenAI provider
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewOpenAIProvider(apiKey, model string) *OpenAIProvider {
	return &OpenAIProvider{
		apiKey: apiKey,
		model:  model,
		client: &http.Client{},
	}
}

// ID returns the provider identifier
func (p *OpenAIProvider) ID() string {
	return "openai"
}

// Stream sends a request and returns streaming events
func (p *OpenAIProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	// Convert to OpenAI format
	openaiReq := p.buildRequest(req)

	body, err := json.Marshal(openaiReq)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal request: %w", err)
	}

	httpReq, err := http.NewRequestWithContext(ctx, "POST", openaiAPIURL, bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Authorization", "Bearer "+p.apiKey)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		return nil, fmt.Errorf("request failed: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		defer resp.Body.Close()
		body, _ := io.ReadAll(resp.Body)
		return nil, parseOpenAIError(resp.StatusCode, body)
	}

	events := make(chan StreamEvent, 100)
	go p.streamResponse(ctx, resp, events)

	return events, nil
}

// buildRequest converts ChatRequest to OpenAI API format
func (p *OpenAIProvider) buildRequest(req *ChatRequest) map[string]any {
	messages := make([]map[string]any, 0, len(req.Messages)+1)

	// Add system message if provided
	if req.System != "" {
		messages = append(messages, map[string]any{
			"role":    "system",
			"content": req.System,
		})
	}

	for _, msg := range req.Messages {
		openaiMsg := p.convertMessage(msg)
		if openaiMsg != nil {
			messages = append(messages, openaiMsg)
		}
	}

	// Use request model override if provided, otherwise use provider default
	model := p.model
	if req.Model != "" {
		model = req.Model
	}

	result := map[string]any{
		"model":    model,
		"messages": messages,
		"stream":   true,
	}

	if req.MaxTokens > 0 {
		result["max_completion_tokens"] = req.MaxTokens
	}

	if len(req.Tools) > 0 {
		tools := make([]map[string]any, 0, len(req.Tools))
		for _, tool := range req.Tools {
			tools = append(tools, map[string]any{
				"type": "function",
				"function": map[string]any{
					"name":        tool.Name,
					"description": tool.Description,
					"parameters":  json.RawMessage(tool.InputSchema),
				},
			})
		}
		result["tools"] = tools
	}

	return result
}

// convertMessage converts a session message to OpenAI format
func (p *OpenAIProvider) convertMessage(msg session.Message) map[string]any {
	switch msg.Role {
	case "user":
		return map[string]any{
			"role":    "user",
			"content": msg.Content,
		}
	case "assistant":
		result := map[string]any{
			"role": "assistant",
		}

		if msg.Content != "" {
			result["content"] = msg.Content
		}

		// Add tool calls if present
		if len(msg.ToolCalls) > 0 {
			var toolCalls []session.ToolCall
			if err := json.Unmarshal(msg.ToolCalls, &toolCalls); err == nil {
				openaiToolCalls := make([]map[string]any, 0, len(toolCalls))
				for _, tc := range toolCalls {
					openaiToolCalls = append(openaiToolCalls, map[string]any{
						"id":   tc.ID,
						"type": "function",
						"function": map[string]any{
							"name":      tc.Name,
							"arguments": string(tc.Input),
						},
					})
				}
				result["tool_calls"] = openaiToolCalls
			}
		}

		return result
	case "tool":
		// Tool results
		if len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
				// OpenAI expects separate messages for each tool result
				if len(results) > 0 {
					return map[string]any{
						"role":         "tool",
						"tool_call_id": results[0].ToolCallID,
						"content":      results[0].Content,
					}
				}
			}
		}
		return nil
	case "system":
		return map[string]any{
			"role":    "system",
			"content": msg.Content,
		}
	}
	return nil
}

// streamResponse reads SSE events and sends them to the channel
func (p *OpenAIProvider) streamResponse(ctx context.Context, resp *http.Response, events chan<- StreamEvent) {
	defer close(events)
	defer resp.Body.Close()

	reader := bufio.NewReader(resp.Body)
	var currentToolCall *ToolCall
	var argsBuffer strings.Builder

	for {
		select {
		case <-ctx.Done():
			events <- StreamEvent{Type: EventTypeError, Error: ctx.Err()}
			return
		default:
		}

		line, err := reader.ReadString('\n')
		if err != nil {
			if err == io.EOF {
				break
			}
			events <- StreamEvent{Type: EventTypeError, Error: err}
			return
		}

		line = strings.TrimSpace(line)
		if line == "" || !strings.HasPrefix(line, "data: ") {
			continue
		}

		data := strings.TrimPrefix(line, "data: ")
		if data == "[DONE]" {
			break
		}

		var chunk openaiStreamChunk
		if err := json.Unmarshal([]byte(data), &chunk); err != nil {
			continue
		}

		if len(chunk.Choices) == 0 {
			continue
		}

		choice := chunk.Choices[0]
		delta := choice.Delta

		// Handle content
		if delta.Content != "" {
			events <- StreamEvent{
				Type: EventTypeText,
				Text: delta.Content,
			}
		}

		// Handle tool calls
		if len(delta.ToolCalls) > 0 {
			for _, tc := range delta.ToolCalls {
				if tc.ID != "" {
					// New tool call starting
					if currentToolCall != nil {
						// Finish previous tool call
						currentToolCall.Input = json.RawMessage(argsBuffer.String())
						events <- StreamEvent{
							Type:     EventTypeToolCall,
							ToolCall: currentToolCall,
						}
					}
					currentToolCall = &ToolCall{
						ID:   tc.ID,
						Name: tc.Function.Name,
					}
					argsBuffer.Reset()
				}
				if tc.Function.Arguments != "" {
					argsBuffer.WriteString(tc.Function.Arguments)
				}
			}
		}

		// Handle finish reason
		if choice.FinishReason == "tool_calls" && currentToolCall != nil {
			currentToolCall.Input = json.RawMessage(argsBuffer.String())
			events <- StreamEvent{
				Type:     EventTypeToolCall,
				ToolCall: currentToolCall,
			}
			currentToolCall = nil
		}
	}

	events <- StreamEvent{Type: EventTypeDone}
}

// openaiStreamChunk represents a streaming chunk from OpenAI
type openaiStreamChunk struct {
	Choices []struct {
		Delta struct {
			Content   string `json:"content,omitempty"`
			ToolCalls []struct {
				ID       string `json:"id,omitempty"`
				Function struct {
					Name      string `json:"name,omitempty"`
					Arguments string `json:"arguments,omitempty"`
				} `json:"function,omitempty"`
			} `json:"tool_calls,omitempty"`
		} `json:"delta"`
		FinishReason string `json:"finish_reason,omitempty"`
	} `json:"choices"`
}

// parseOpenAIError parses an error response from the OpenAI API
func parseOpenAIError(statusCode int, body []byte) error {
	var errResp struct {
		Error struct {
			Type    string `json:"type"`
			Message string `json:"message"`
			Code    string `json:"code"`
		} `json:"error"`
	}

	if err := json.Unmarshal(body, &errResp); err != nil {
		return &ProviderError{
			Code:    fmt.Sprintf("%d", statusCode),
			Message: string(body),
		}
	}

	code := errResp.Error.Code
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
