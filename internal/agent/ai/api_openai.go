package ai

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strconv"
	"sync"
	"time"

	"github.com/openai/openai-go"
	"github.com/openai/openai-go/option"
	"github.com/openai/openai-go/packages/ssestream"
	"github.com/openai/openai-go/shared"

	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/agenthub"
)

// RateLimitInfo holds parsed rate-limit headers from a provider response.
type RateLimitInfo struct {
	LimitTokens     int64
	RemainingTokens int64
	ResetAt         time.Time
	UpdatedAt       time.Time
}

// OpenAIProvider implements the OpenAI API using the official SDK
type OpenAIProvider struct {
	client     openai.Client
	model      string
	providerID string // custom ID override (e.g. "janus" for NeboLoop)
	botID      string // X-Bot-ID header for Janus per-bot billing

	rateLimitMu sync.RWMutex
	rateLimit   *RateLimitInfo
}

// NewOpenAIProvider creates a new OpenAI provider.
// Optional baseURL overrides the API endpoint for OpenAI-compatible services.
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewOpenAIProvider(apiKey, model string, baseURL ...string) *OpenAIProvider {
	p := &OpenAIProvider{
		model: model,
	}
	opts := []option.RequestOption{
		option.WithAPIKey(apiKey),
		option.WithMiddleware(p.captureRateLimitHeaders),
	}
	if len(baseURL) > 0 && baseURL[0] != "" {
		opts = append(opts, option.WithBaseURL(baseURL[0]))
	}
	p.client = openai.NewClient(opts...)
	return p
}

// captureRateLimitHeaders intercepts HTTP responses to extract X-RateLimit-* headers.
func (p *OpenAIProvider) captureRateLimitHeaders(req *http.Request, next option.MiddlewareNext) (*http.Response, error) {
	resp, err := next(req)
	if err != nil || resp == nil {
		return resp, err
	}
	limitStr := resp.Header.Get("X-RateLimit-Limit-Tokens")
	if limitStr == "" {
		return resp, err
	}
	limit, _ := strconv.ParseInt(limitStr, 10, 64)
	remaining, _ := strconv.ParseInt(resp.Header.Get("X-RateLimit-Remaining-Tokens"), 10, 64)
	var resetAt time.Time
	if resetStr := resp.Header.Get("X-RateLimit-Reset"); resetStr != "" {
		resetAt, _ = time.Parse(time.RFC3339, resetStr)
	}
	if limit > 0 {
		info := &RateLimitInfo{
			LimitTokens:     limit,
			RemainingTokens: remaining,
			ResetAt:         resetAt,
			UpdatedAt:       time.Now(),
		}
		p.rateLimitMu.Lock()
		p.rateLimit = info
		p.rateLimitMu.Unlock()
		fmt.Printf("[OpenAI] Rate limit: %d/%d tokens, reset %s\n", remaining, limit, resetAt.Format(time.RFC3339))
	}
	return resp, err
}

// GetRateLimit returns the latest rate-limit info, or nil if not yet received.
func (p *OpenAIProvider) GetRateLimit() *RateLimitInfo {
	p.rateLimitMu.RLock()
	defer p.rateLimitMu.RUnlock()
	return p.rateLimit
}

// SetProviderID overrides the provider ID (default "openai").
// Used for OpenAI-compatible providers like Janus/NeboLoop.
func (p *OpenAIProvider) SetProviderID(id string) {
	p.providerID = id
}

// SetBotID sets the bot ID sent as X-Bot-ID header on every request.
// Required for Janus per-bot billing.
func (p *OpenAIProvider) SetBotID(botID string) {
	p.botID = botID
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

	// Create streaming request (with per-request options like X-Bot-ID for Janus)
	var reqOpts []option.RequestOption
	if p.botID != "" {
		reqOpts = append(reqOpts, option.WithHeader("X-Bot-ID", p.botID))
	}
	if p.providerID == "janus" {
		if lane := agenthub.GetLane(ctx); lane != "" {
			reqOpts = append(reqOpts, option.WithHeader("X-Lane", lane))
		}
	}
	stream := p.client.Chat.Completions.NewStreaming(ctx, params, reqOpts...)

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
	skippedOrphans := 0
	skippedEmpty := 0

	// Add system message if provided
	if req.System != "" {
		result = append(result, openai.SystemMessage(req.System))
	}

	for _, msg := range req.Messages {
		switch msg.Role {
		case "user":
			if msg.Content == "" {
				skippedEmpty++
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
						if !respondedToolIDs[tc.ID] {
							skippedOrphans++
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
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
					for _, r := range results {
						if respondedToolIDs[r.ToolCallID] {
							result = append(result, openai.ToolMessage(r.Content, r.ToolCallID))
						}
					}
				}
			}

		case "system":
			if msg.Content == "" {
				skippedEmpty++
				continue
			}
			result = append(result, openai.SystemMessage(msg.Content))
		}
	}

	if skippedOrphans > 0 || skippedEmpty > 0 {
		fmt.Printf("[OpenAI] Cleaned history: stripped %d orphaned tool calls, %d empty messages\n", skippedOrphans, skippedEmpty)
	}

	return result, nil
}

// handleStream processes the streaming response
func (p *OpenAIProvider) handleStream(stream *ssestream.Stream[openai.ChatCompletionChunk], events chan<- StreamEvent) {
	defer close(events)
	defer stream.Close()

	acc := openai.ChatCompletionAccumulator{}
	chunkCount := 0
	textChunks := 0
	finishedClean := false // true when we saw finish_reason and broke early
	emittedToolCalls := make(map[string]bool)
	seenToolName := make(map[int64]bool) // track which tool indices already have a name

	for stream.Next() {
		chunk := stream.Current()
		chunkCount++

		// Prevent tool name duplication: some gateways (e.g. Janus) send the
		// tool name in every chunk. The SDK accumulator concatenates names, so
		// "agent" x4 becomes "agentagentagentagent". Clear repeated names.
		if len(chunk.Choices) > 0 {
			for i := range chunk.Choices[0].Delta.ToolCalls {
				idx := chunk.Choices[0].Delta.ToolCalls[i].Index
				if chunk.Choices[0].Delta.ToolCalls[i].Function.Name != "" {
					if seenToolName[idx] {
						chunk.Choices[0].Delta.ToolCalls[i].Function.Name = ""
					} else {
						seenToolName[idx] = true
					}
				}
			}
		}

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

		// Check for terminal finish reason — stop waiting for more chunks.
		// Some gateways (e.g., Janus) don't send the SSE [DONE] sentinel after
		// finish_reason, causing stream.Next() to block until TCP timeout (~120s).
		if len(chunk.Choices) > 0 && chunk.Choices[0].FinishReason != "" {
			fmt.Printf("[OpenAI] Stream finish_reason=%s (after %d text chunks)\n",
				chunk.Choices[0].FinishReason, textChunks)
			finishedClean = true
			break
		}
	}

	if err := stream.Err(); err != nil && !finishedClean {
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
