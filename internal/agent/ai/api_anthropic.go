package ai

import (
	"context"
	"encoding/json"
	"fmt"
	"os"

	"github.com/anthropics/anthropic-sdk-go"
	"github.com/anthropics/anthropic-sdk-go/option"
	"github.com/anthropics/anthropic-sdk-go/packages/ssestream"

	"github.com/nebolabs/nebo/internal/agent/session"
)

const defaultMaxTokens = 8192

// debugAI enables verbose AI request/response logging
var debugAI = os.Getenv("NEBO_DEBUG_AI") != ""

func logDebug(format string, args ...interface{}) {
	if debugAI {
		fmt.Printf("[AI DEBUG] "+format+"\n", args...)
	}
}

// AnthropicProvider implements the Anthropic Claude API using the official SDK
type AnthropicProvider struct {
	client anthropic.Client
	model  string
}

// NewAnthropicProvider creates a new Anthropic provider
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewAnthropicProvider(apiKey, model string) *AnthropicProvider {
	client := anthropic.NewClient(option.WithAPIKey(apiKey))
	return &AnthropicProvider{
		client: client,
		model:  model,
	}
}

// ID returns the provider identifier
func (p *AnthropicProvider) ID() string {
	return "anthropic"
}

// ProfileID returns empty - use ProfiledProvider wrapper for profile tracking
func (p *AnthropicProvider) ProfileID() string {
	return ""
}

// HandlesTools returns false - the runner executes tools for API providers
func (p *AnthropicProvider) HandlesTools() bool {
	return false
}

// Stream sends a request and returns streaming events
func (p *AnthropicProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	// Build messages
	messages, err := p.buildMessages(req.Messages)
	if err != nil {
		return nil, fmt.Errorf("failed to build messages: %w", err)
	}

	// Use request model override if provided, otherwise use provider default
	model := p.model
	if req.Model != "" {
		model = req.Model
	}

	// Build params
	params := anthropic.MessageNewParams{
		Model:     anthropic.Model(model),
		MaxTokens: int64(defaultMaxTokens),
		Messages:  messages,
	}

	if req.MaxTokens > 0 {
		params.MaxTokens = int64(req.MaxTokens)
	}

	if req.System != "" {
		params.System = []anthropic.TextBlockParam{
			{Text: req.System},
		}
	}

	// Add tools if provided
	if len(req.Tools) > 0 {
		tools := make([]anthropic.ToolUnionParam, 0, len(req.Tools))
		for _, tool := range req.Tools {
			// Parse the JSON schema
			var schema map[string]interface{}
			if err := json.Unmarshal([]byte(tool.InputSchema), &schema); err != nil {
				fmt.Printf("[Anthropic] Failed to parse tool schema for %s: %v\n", tool.Name, err)
				continue
			}

			toolParam := anthropic.ToolParam{
				Name:        tool.Name,
				Description: anthropic.String(tool.Description),
				InputSchema: anthropic.ToolInputSchemaParam{
					Properties: schema["properties"],
				},
			}

			// Add required if present
			if required, ok := schema["required"].([]interface{}); ok {
				reqStrings := make([]string, len(required))
				for i, r := range required {
					reqStrings[i] = r.(string)
				}
				toolParam.InputSchema.Required = reqStrings
			}

			tools = append(tools, anthropic.ToolUnionParam{OfTool: &toolParam})
		}
		params.Tools = tools
	}

	// Enable extended thinking mode for reasoning tasks
	if req.EnableThinking {
		params.Thinking = anthropic.ThinkingConfigParamOfEnabled(10000)
		if req.MaxTokens <= 0 {
			params.MaxTokens = 16384
		}
	}

	fmt.Printf("[Anthropic] Sending request: model=%s messages=%d tools=%d\n",
		model, len(messages), len(req.Tools))

	// Create streaming request
	stream := p.client.Messages.NewStreaming(ctx, params)

	events := make(chan StreamEvent, 100)
	go p.handleStream(stream, events)

	return events, nil
}

// buildMessages converts session messages to Anthropic format
func (p *AnthropicProvider) buildMessages(msgs []session.Message) ([]anthropic.MessageParam, error) {
	// First pass: collect all tool_call IDs and tool_result IDs
	// This allows us to filter orphaned entries on both sides
	allToolCallIDs := make(map[string]bool)
	respondedToolIDs := make(map[string]bool)
	for _, msg := range msgs {
		if msg.Role == "assistant" && len(msg.ToolCalls) > 0 {
			var toolCalls []session.ToolCall
			if err := json.Unmarshal(msg.ToolCalls, &toolCalls); err == nil {
				for _, tc := range toolCalls {
					allToolCallIDs[tc.ID] = true
				}
			}
		}
		if msg.Role == "tool" && len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
				for _, r := range results {
					respondedToolIDs[r.ToolCallID] = true
				}
			}
		}
	}

	var result []anthropic.MessageParam

	for _, msg := range msgs {
		switch msg.Role {
		case "user":
			// Skip empty user messages to avoid "text content blocks must be non-empty" error
			if msg.Content == "" {
				continue
			}
			result = append(result, anthropic.NewUserMessage(
				anthropic.NewTextBlock(msg.Content),
			))

		case "assistant":
			var blocks []anthropic.ContentBlockParamUnion

			// Add text content if present
			if msg.Content != "" {
				blocks = append(blocks, anthropic.NewTextBlock(msg.Content))
			}

			// Add tool calls if present (only those with responses)
			if len(msg.ToolCalls) > 0 {
				var toolCalls []session.ToolCall
				if err := json.Unmarshal(msg.ToolCalls, &toolCalls); err == nil {
					for _, tc := range toolCalls {
						// Only include tool calls that have responses
						if !respondedToolIDs[tc.ID] {
							fmt.Printf("[Anthropic] Skipping tool_use without response: %s\n", tc.ID)
							continue
						}

						var input map[string]interface{}
						if err := json.Unmarshal(tc.Input, &input); err != nil {
							input = map[string]interface{}{}
						}
						blocks = append(blocks, anthropic.ContentBlockParamUnion{
							OfToolUse: &anthropic.ToolUseBlockParam{
								ID:    tc.ID,
								Name:  tc.Name,
								Input: input,
							},
						})
					}
				}
			}

			if len(blocks) > 0 {
				result = append(result, anthropic.MessageParam{
					Role:    anthropic.MessageParamRoleAssistant,
					Content: blocks,
				})
			}

		case "tool":
			// Tool results - create user message with tool result blocks
			// Only include results that have matching tool_calls (filter orphaned results)
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					var blocks []anthropic.ContentBlockParamUnion
					for _, r := range results {
						// Skip orphaned tool results (no matching tool_call)
						if !allToolCallIDs[r.ToolCallID] {
							fmt.Printf("[Anthropic] Skipping orphaned tool_result: %s\n", r.ToolCallID)
							continue
						}
						// Also skip results for tool_calls we're not including (those without responses)
						if !respondedToolIDs[r.ToolCallID] {
							continue
						}
						blocks = append(blocks, anthropic.NewToolResultBlock(
							r.ToolCallID,
							r.Content,
							r.IsError,
						))
					}
					if len(blocks) > 0 {
						result = append(result, anthropic.NewUserMessage(blocks...))
					}
				}
			}

		case "system":
			// System messages handled separately via params.System
			continue
		}
	}

	return result, nil
}

// handleStream processes the streaming response
func (p *AnthropicProvider) handleStream(stream *ssestream.Stream[anthropic.MessageStreamEventUnion], events chan<- StreamEvent) {
	defer close(events)

	var currentToolID string
	var currentToolName string
	var inputBuffer string

	for stream.Next() {
		event := stream.Current()

		switch event.Type {
		case "content_block_start":
			cb := event.AsContentBlockStart()
			block := cb.ContentBlock.AsAny()
			if toolUse, ok := block.(anthropic.ToolUseBlock); ok {
				currentToolID = toolUse.ID
				currentToolName = toolUse.Name
				inputBuffer = ""
			}

		case "content_block_delta":
			delta := event.AsContentBlockDelta()
			switch d := delta.Delta.AsAny().(type) {
			case anthropic.TextDelta:
				events <- StreamEvent{
					Type: EventTypeText,
					Text: d.Text,
				}
			case anthropic.InputJSONDelta:
				inputBuffer += d.PartialJSON
			case anthropic.ThinkingDelta:
				events <- StreamEvent{
					Type: EventTypeThinking,
					Text: d.Thinking,
				}
			}

		case "content_block_stop":
			if currentToolID != "" {
				events <- StreamEvent{
					Type: EventTypeToolCall,
					ToolCall: &ToolCall{
						ID:    currentToolID,
						Name:  currentToolName,
						Input: json.RawMessage(inputBuffer),
					},
				}
				currentToolID = ""
				currentToolName = ""
				inputBuffer = ""
			}

		case "message_stop":
			events <- StreamEvent{Type: EventTypeDone}
			return

		case "error":
			// Try to extract error info from the raw JSON
			events <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("stream error: %s", event.RawJSON()),
			}
			return
		}
	}

	if err := stream.Err(); err != nil {
		fmt.Printf("[Anthropic] Stream error: %v\n", err)
		events <- StreamEvent{
			Type:  EventTypeError,
			Error: err,
		}
		return
	}

	events <- StreamEvent{Type: EventTypeDone}
}
