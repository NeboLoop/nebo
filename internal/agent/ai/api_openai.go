package ai

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/openai/openai-go"
	"github.com/openai/openai-go/option"
	"github.com/openai/openai-go/packages/ssestream"
	"github.com/openai/openai-go/shared"

	"github.com/neboloop/nebo/internal/agent/session"
)

// OpenAIProvider implements the OpenAI API using the official SDK
type OpenAIProvider struct {
	client     openai.Client
	model      string
	providerID string // custom ID override (e.g. "janus" for NeboLoop)
}

// NewOpenAIProvider creates a new OpenAI provider.
// Optional baseURL overrides the API endpoint for OpenAI-compatible services.
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewOpenAIProvider(apiKey, model string, baseURL ...string) *OpenAIProvider {
	opts := []option.RequestOption{option.WithAPIKey(apiKey)}
	if len(baseURL) > 0 && baseURL[0] != "" {
		opts = append(opts, option.WithBaseURL(baseURL[0]))
	}
	client := openai.NewClient(opts...)
	return &OpenAIProvider{
		client: client,
		model:  model,
	}
}

// SetProviderID overrides the provider ID (default "openai").
// Used for OpenAI-compatible providers like Janus/NeboLoop.
func (p *OpenAIProvider) SetProviderID(id string) {
	p.providerID = id
}

// ID returns the provider identifier
func (p *OpenAIProvider) ID() string {
	if p.providerID != "" {
		return p.providerID
	}
	return "openai"
}

// ProfileID returns empty - use ProfiledProvider wrapper for profile tracking
func (p *OpenAIProvider) ProfileID() string {
	return ""
}

// HandlesTools returns false - the runner executes tools for API providers
func (p *OpenAIProvider) HandlesTools() bool {
	return false
}

// Stream sends a request and returns streaming events
func (p *OpenAIProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	// Build messages
	messages, err := p.buildMessages(req)
	if err != nil {
		return nil, fmt.Errorf("failed to build messages: %w", err)
	}

	// Use request model override if provided, otherwise use provider default
	model := p.model
	if req.Model != "" {
		model = req.Model
	}

	// Build params
	params := openai.ChatCompletionNewParams{
		Model:    shared.ChatModel(model),
		Messages: messages,
	}

	if req.MaxTokens > 0 {
		params.MaxCompletionTokens = openai.Int(int64(req.MaxTokens))
	}

	// Add tools if provided
	if len(req.Tools) > 0 {
		tools := make([]openai.ChatCompletionToolParam, 0, len(req.Tools))
		for _, tool := range req.Tools {
			var schema map[string]interface{}
			if err := json.Unmarshal([]byte(tool.InputSchema), &schema); err != nil {
				fmt.Printf("[OpenAI] Failed to parse tool schema for %s: %v\n", tool.Name, err)
				continue
			}

			tools = append(tools, openai.ChatCompletionToolParam{
				Function: shared.FunctionDefinitionParam{
					Name:        tool.Name,
					Description: openai.String(tool.Description),
					Parameters:  shared.FunctionParameters(schema),
				},
			})
		}
		params.Tools = tools
	}

	fmt.Printf("[OpenAI] Sending request: model=%s messages=%d tools=%d\n",
		model, len(messages), len(req.Tools))

	// Create streaming request
	stream := p.client.Chat.Completions.NewStreaming(ctx, params)

	events := make(chan StreamEvent, 100)
	go p.handleStream(stream, events)

	return events, nil
}

// buildMessages converts session messages to OpenAI format
func (p *OpenAIProvider) buildMessages(req *ChatRequest) ([]openai.ChatCompletionMessageParamUnion, error) {
	// First pass: collect all tool_call_ids that have responses
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

	var result []openai.ChatCompletionMessageParamUnion

	// Add system message if provided
	if req.System != "" {
		result = append(result, openai.SystemMessage(req.System))
	}

	for i, msg := range req.Messages {
		switch msg.Role {
		case "user":
			if msg.Content == "" {
				fmt.Printf("[OpenAI] Skipping empty user message at index %d\n", i)
				continue
			}
			result = append(result, openai.UserMessage(msg.Content))

		case "assistant":
			// Build assistant message with optional tool calls
			var toolCalls []openai.ChatCompletionMessageToolCallParam

			if len(msg.ToolCalls) > 0 {
				var tcs []session.ToolCall
				if err := json.Unmarshal(msg.ToolCalls, &tcs); err == nil {
					for _, tc := range tcs {
						// Only include tool calls that have responses
						if !respondedToolIDs[tc.ID] {
							fmt.Printf("[OpenAI] Skipping tool_call without response: %s\n", tc.ID)
							continue
						}
						toolCalls = append(toolCalls, openai.ChatCompletionMessageToolCallParam{
							ID:   tc.ID,
							Type: "function",
							Function: openai.ChatCompletionMessageToolCallFunctionParam{
								Name:      tc.Name,
								Arguments: string(tc.Input),
							},
						})
					}
				}
			}

			// Only add message if it has content or tool calls
			if msg.Content != "" || len(toolCalls) > 0 {
				assistantMsg := openai.ChatCompletionAssistantMessageParam{
					Role: "assistant",
				}
				// Always set content — some gateways (e.g. Janus→Gemini) reject
				// assistant messages with null content even when tool_calls are present
				content := msg.Content
				if content == "" && len(toolCalls) > 0 {
					content = " "
				}
				if content != "" {
					assistantMsg.Content = openai.ChatCompletionAssistantMessageParamContentUnion{
						OfString: openai.String(content),
					}
				}
				if len(toolCalls) > 0 {
					assistantMsg.ToolCalls = toolCalls
				}
				result = append(result, openai.ChatCompletionMessageParamUnion{
					OfAssistant: &assistantMsg,
				})
			}

		case "tool":
			// Tool results
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					for _, r := range results {
						// Only include results for tool calls we kept
						if respondedToolIDs[r.ToolCallID] {
							result = append(result, openai.ToolMessage(r.Content, r.ToolCallID))
						}
					}
				}
			}

		case "system":
			if msg.Content == "" {
				fmt.Printf("[OpenAI] Skipping empty system message at index %d\n", i)
				continue
			}
			result = append(result, openai.SystemMessage(msg.Content))
		}
	}

	return result, nil
}

// handleStream processes the streaming response
func (p *OpenAIProvider) handleStream(stream *ssestream.Stream[openai.ChatCompletionChunk], events chan<- StreamEvent) {
	defer close(events)

	acc := openai.ChatCompletionAccumulator{}
	chunkCount := 0
	textChunks := 0
	emittedToolCalls := make(map[string]bool)

	for stream.Next() {
		chunk := stream.Current()
		chunkCount++
		acc.AddChunk(chunk)

		// Log first chunk for debugging (shows finish_reason, model echo, etc.)
		if chunkCount == 1 {
			if raw, err := json.Marshal(chunk); err == nil {
				fmt.Printf("[OpenAI] First chunk: %s\n", string(raw))
			}
		}

		// Check for finished tool calls (works when tool calls are streamed incrementally)
		if tool, ok := acc.JustFinishedToolCall(); ok {
			emittedToolCalls[tool.ID] = true
			events <- StreamEvent{
				Type: EventTypeToolCall,
				ToolCall: &ToolCall{
					ID:    tool.ID,
					Name:  tool.Name,
					Input: json.RawMessage(tool.Arguments),
				},
			}
		}

		// Stream text content
		if len(chunk.Choices) > 0 && chunk.Choices[0].Delta.Content != "" {
			textChunks++
			events <- StreamEvent{
				Type: EventTypeText,
				Text: chunk.Choices[0].Delta.Content,
			}
		}

		// Log non-text chunks that have a finish reason (e.g. "stop", "length", "content_filter")
		if len(chunk.Choices) > 0 && chunk.Choices[0].FinishReason != "" {
			fmt.Printf("[OpenAI] Stream finish_reason=%s (after %d text chunks)\n",
				chunk.Choices[0].FinishReason, textChunks)
		}
	}

	if err := stream.Err(); err != nil {
		fmt.Printf("[OpenAI] Stream error: %v\n", err)
		events <- StreamEvent{
			Type:  EventTypeError,
			Error: err,
		}
		return
	}

	// Fallback: emit any accumulated tool calls that JustFinishedToolCall() missed.
	// This happens when a gateway (e.g. Janus) sends complete tool calls in a single
	// chunk instead of streaming arguments incrementally.
	if len(acc.Choices) > 0 {
		for _, tc := range acc.Choices[0].Message.ToolCalls {
			if !emittedToolCalls[tc.ID] && tc.Function.Name != "" {
				fmt.Printf("[OpenAI] Fallback: emitting tool call %s (%s) from accumulator\n", tc.ID, tc.Function.Name)
				events <- StreamEvent{
					Type: EventTypeToolCall,
					ToolCall: &ToolCall{
						ID:    tc.ID,
						Name:  tc.Function.Name,
						Input: json.RawMessage(tc.Function.Arguments),
					},
				}
			}
		}
	}

	if chunkCount == 0 {
		fmt.Printf("[OpenAI] Warning: stream completed with 0 chunks (empty response from %s)\n", p.ID())
	} else if textChunks == 0 && len(emittedToolCalls) == 0 {
		fmt.Printf("[OpenAI] Warning: stream had %d chunks but 0 text content and 0 tool calls (provider: %s)\n", chunkCount, p.ID())
	}

	events <- StreamEvent{Type: EventTypeDone}
}
