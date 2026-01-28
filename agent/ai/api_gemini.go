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
	"time"

	"gobot/agent/session"
)

// GeminiProvider implements the Provider interface for Google Gemini
type GeminiProvider struct {
	apiKey string
	model  string
	client *http.Client
}

// GeminiContent represents content in Gemini format
type GeminiContent struct {
	Role  string       `json:"role"`
	Parts []GeminiPart `json:"parts"`
}

// GeminiPart represents a part of content
type GeminiPart struct {
	Text string `json:"text,omitempty"`
}

// GeminiRequest represents a request to Gemini
type GeminiRequest struct {
	Contents         []GeminiContent    `json:"contents"`
	SystemInstruction *GeminiContent    `json:"systemInstruction,omitempty"`
	GenerationConfig *GeminiGenConfig   `json:"generationConfig,omitempty"`
	Tools            []GeminiTool       `json:"tools,omitempty"`
}

// GeminiGenConfig represents generation configuration
type GeminiGenConfig struct {
	Temperature     float64 `json:"temperature,omitempty"`
	MaxOutputTokens int     `json:"maxOutputTokens,omitempty"`
}

// GeminiTool represents a tool definition for Gemini
type GeminiTool struct {
	FunctionDeclarations []GeminiFunctionDecl `json:"functionDeclarations"`
}

// GeminiFunctionDecl represents a function declaration
type GeminiFunctionDecl struct {
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Parameters  json.RawMessage `json:"parameters"`
}

// GeminiStreamResponse represents a streaming response
type GeminiStreamResponse struct {
	Candidates []struct {
		Content struct {
			Parts []struct {
				Text         string `json:"text,omitempty"`
				FunctionCall *struct {
					Name string          `json:"name"`
					Args json.RawMessage `json:"args"`
				} `json:"functionCall,omitempty"`
			} `json:"parts"`
		} `json:"content"`
		FinishReason string `json:"finishReason"`
	} `json:"candidates"`
	Error *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// NewGeminiProvider creates a new Gemini provider
func NewGeminiProvider(apiKey, model string) *GeminiProvider {
	if model == "" {
		model = "gemini-1.5-flash" // Default model
	}
	return &GeminiProvider{
		apiKey: apiKey,
		model:  model,
		client: &http.Client{
			Timeout: 5 * time.Minute,
		},
	}
}

// ID returns the provider identifier
func (p *GeminiProvider) ID() string {
	return fmt.Sprintf("gemini-%s", p.model)
}

// Stream sends a request to Gemini and streams the response
func (p *GeminiProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	resultCh := make(chan StreamEvent, 100)

	go func() {
		defer close(resultCh)

		// Convert messages to Gemini format
		contents := make([]GeminiContent, 0, len(req.Messages))

		for _, msg := range req.Messages {
			var role string
			switch msg.Role {
			case "user":
				role = "user"
			case "assistant":
				role = "model"
			case "system":
				// System messages handled separately in Gemini
				continue
			case "tool":
				// Include tool results as user messages
				if len(msg.ToolResults) > 0 {
					var results []session.ToolResult
					json.Unmarshal(msg.ToolResults, &results)
					for _, r := range results {
						contents = append(contents, GeminiContent{
							Role: "user",
							Parts: []GeminiPart{{
								Text: fmt.Sprintf("[Tool Result: %s]\n%s", r.ToolCallID, r.Content),
							}},
						})
					}
				}
				continue
			default:
				continue
			}

			content := msg.Content
			if content == "" && len(msg.ToolCalls) > 0 {
				// Include tool calls in assistant message
				var calls []session.ToolCall
				json.Unmarshal(msg.ToolCalls, &calls)
				var parts []string
				for _, c := range calls {
					parts = append(parts, fmt.Sprintf("[Using tool: %s]", c.Name))
				}
				content = strings.Join(parts, "\n")
			}

			if content != "" {
				contents = append(contents, GeminiContent{
					Role:  role,
					Parts: []GeminiPart{{Text: content}},
				})
			}
		}

		// Gemini requires alternating user/model turns
		contents = p.normalizeContents(contents)

		if len(contents) == 0 {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("no valid messages to send"),
			}
			return
		}

		// Build request
		geminiReq := GeminiRequest{
			Contents: contents,
		}

		// Add system instruction if present
		if req.System != "" {
			geminiReq.SystemInstruction = &GeminiContent{
				Parts: []GeminiPart{{Text: req.System}},
			}
		}

		// Add generation config
		if req.Temperature > 0 || req.MaxTokens > 0 {
			geminiReq.GenerationConfig = &GeminiGenConfig{}
			if req.Temperature > 0 {
				geminiReq.GenerationConfig.Temperature = req.Temperature
			}
			if req.MaxTokens > 0 {
				geminiReq.GenerationConfig.MaxOutputTokens = req.MaxTokens
			}
		}

		// Add tools if present
		if len(req.Tools) > 0 {
			funcs := make([]GeminiFunctionDecl, 0, len(req.Tools))
			for _, tool := range req.Tools {
				funcs = append(funcs, GeminiFunctionDecl{
					Name:        tool.Name,
					Description: tool.Description,
					Parameters:  tool.InputSchema,
				})
			}
			geminiReq.Tools = []GeminiTool{{FunctionDeclarations: funcs}}
		}

		body, err := json.Marshal(geminiReq)
		if err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("failed to marshal request: %w", err),
			}
			return
		}

		// Build URL with streaming
		url := fmt.Sprintf(
			"https://generativelanguage.googleapis.com/v1beta/models/%s:streamGenerateContent?alt=sse&key=%s",
			p.model, p.apiKey,
		)

		httpReq, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(body))
		if err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("failed to create request: %w", err),
			}
			return
		}
		httpReq.Header.Set("Content-Type", "application/json")

		resp, err := p.client.Do(httpReq)
		if err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("request failed: %w", err),
			}
			return
		}
		defer resp.Body.Close()

		if resp.StatusCode != http.StatusOK {
			body, _ := io.ReadAll(resp.Body)
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("Gemini error (%d): %s", resp.StatusCode, string(body)),
			}
			return
		}

		// Parse SSE stream
		scanner := bufio.NewScanner(resp.Body)
		scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

		toolCallCounter := 0

		for scanner.Scan() {
			select {
			case <-ctx.Done():
				return
			default:
			}

			line := scanner.Text()

			// SSE format: "data: {...}"
			if !strings.HasPrefix(line, "data: ") {
				continue
			}

			data := strings.TrimPrefix(line, "data: ")
			if data == "" {
				continue
			}

			var chunk GeminiStreamResponse
			if err := json.Unmarshal([]byte(data), &chunk); err != nil {
				continue
			}

			if chunk.Error != nil {
				resultCh <- StreamEvent{
					Type:  EventTypeError,
					Error: fmt.Errorf("Gemini API error: %s", chunk.Error.Message),
				}
				return
			}

			for _, candidate := range chunk.Candidates {
				for _, part := range candidate.Content.Parts {
					if part.Text != "" {
						resultCh <- StreamEvent{
							Type: EventTypeText,
							Text: part.Text,
						}
					}

					if part.FunctionCall != nil {
						toolCallCounter++
						resultCh <- StreamEvent{
							Type: EventTypeToolCall,
							ToolCall: &ToolCall{
								ID:    fmt.Sprintf("gemini-call-%d", toolCallCounter),
								Name:  part.FunctionCall.Name,
								Input: part.FunctionCall.Args,
							},
						}
					}
				}

				if candidate.FinishReason == "STOP" || candidate.FinishReason == "MAX_TOKENS" {
					resultCh <- StreamEvent{Type: EventTypeDone}
					return
				}
			}
		}

		if err := scanner.Err(); err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("stream read error: %w", err),
			}
		}
	}()

	return resultCh, nil
}

// normalizeContents ensures proper alternating turns for Gemini
func (p *GeminiProvider) normalizeContents(contents []GeminiContent) []GeminiContent {
	if len(contents) == 0 {
		return contents
	}

	normalized := make([]GeminiContent, 0, len(contents))
	var lastRole string

	for _, c := range contents {
		// Gemini requires starting with user
		if len(normalized) == 0 && c.Role != "user" {
			// Prepend a minimal user message
			normalized = append(normalized, GeminiContent{
				Role:  "user",
				Parts: []GeminiPart{{Text: "Continue."}},
			})
		}

		// Merge consecutive same-role messages
		if c.Role == lastRole && len(normalized) > 0 {
			last := &normalized[len(normalized)-1]
			for _, part := range c.Parts {
				last.Parts = append(last.Parts, part)
			}
		} else {
			normalized = append(normalized, c)
			lastRole = c.Role
		}
	}

	// Gemini requires ending with user
	if len(normalized) > 0 && normalized[len(normalized)-1].Role != "user" {
		// This is fine for generation, model will respond
	}

	return normalized
}
