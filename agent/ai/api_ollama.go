package ai

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"gobot/agent/session"
)

// OllamaProvider implements the Provider interface for Ollama (local models)
type OllamaProvider struct {
	baseURL string
	model   string
	client  *http.Client
}

// OllamaMessage represents a message in Ollama format
type OllamaMessage struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// OllamaRequest represents a chat request to Ollama
type OllamaRequest struct {
	Model    string          `json:"model"`
	Messages []OllamaMessage `json:"messages"`
	Stream   bool            `json:"stream"`
	Options  *OllamaOptions  `json:"options,omitempty"`
}

// OllamaOptions represents model options
type OllamaOptions struct {
	Temperature float64 `json:"temperature,omitempty"`
	NumPredict  int     `json:"num_predict,omitempty"` // Max tokens
}

// OllamaStreamResponse represents a streaming response chunk
type OllamaStreamResponse struct {
	Model     string        `json:"model"`
	CreatedAt string        `json:"created_at"`
	Message   OllamaMessage `json:"message"`
	Done      bool          `json:"done"`
}

// NewOllamaProvider creates a new Ollama provider
func NewOllamaProvider(baseURL, model string) *OllamaProvider {
	if baseURL == "" {
		baseURL = "http://localhost:11434"
	}
	if model == "" {
		model = "llama3.2" // Default model
	}
	return &OllamaProvider{
		baseURL: baseURL,
		model:   model,
		client: &http.Client{
			Timeout: 5 * time.Minute, // Longer timeout for local inference
		},
	}
}

// ID returns the provider identifier
func (p *OllamaProvider) ID() string {
	return fmt.Sprintf("ollama-%s", p.model)
}

// Stream sends a request to Ollama and streams the response
func (p *OllamaProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	resultCh := make(chan StreamEvent, 100)

	go func() {
		defer close(resultCh)

		// Convert messages to Ollama format
		messages := make([]OllamaMessage, 0, len(req.Messages)+1)

		// Add system message if present
		if req.System != "" {
			messages = append(messages, OllamaMessage{
				Role:    "system",
				Content: req.System,
			})
		}

		// Convert session messages
		for _, msg := range req.Messages {
			switch msg.Role {
			case "user":
				messages = append(messages, OllamaMessage{
					Role:    "user",
					Content: msg.Content,
				})
			case "assistant":
				messages = append(messages, OllamaMessage{
					Role:    "assistant",
					Content: msg.Content,
				})
			case "system":
				messages = append(messages, OllamaMessage{
					Role:    "system",
					Content: msg.Content,
				})
			case "tool":
				// Ollama doesn't have native tool support, include as user message
				if len(msg.ToolResults) > 0 {
					var results []session.ToolResult
					json.Unmarshal(msg.ToolResults, &results)
					for _, r := range results {
						content := fmt.Sprintf("[Tool Result]\n%s", r.Content)
						messages = append(messages, OllamaMessage{
							Role:    "user",
							Content: content,
						})
					}
				}
			}
		}

		// Build request
		ollamaReq := OllamaRequest{
			Model:    p.model,
			Messages: messages,
			Stream:   true,
		}

		if req.Temperature > 0 {
			ollamaReq.Options = &OllamaOptions{
				Temperature: req.Temperature,
			}
		}
		if req.MaxTokens > 0 {
			if ollamaReq.Options == nil {
				ollamaReq.Options = &OllamaOptions{}
			}
			ollamaReq.Options.NumPredict = req.MaxTokens
		}

		body, err := json.Marshal(ollamaReq)
		if err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("failed to marshal request: %w", err),
			}
			return
		}

		// Make HTTP request
		httpReq, err := http.NewRequestWithContext(ctx, "POST", p.baseURL+"/api/chat", bytes.NewReader(body))
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
				Error: fmt.Errorf("Ollama error (%d): %s", resp.StatusCode, string(body)),
			}
			return
		}

		// Stream response
		scanner := bufio.NewScanner(resp.Body)
		scanner.Buffer(make([]byte, 1024*1024), 1024*1024)

		for scanner.Scan() {
			select {
			case <-ctx.Done():
				return
			default:
			}

			line := scanner.Bytes()
			if len(line) == 0 {
				continue
			}

			var chunk OllamaStreamResponse
			if err := json.Unmarshal(line, &chunk); err != nil {
				continue
			}

			if chunk.Message.Content != "" {
				resultCh <- StreamEvent{
					Type: EventTypeText,
					Text: chunk.Message.Content,
				}
			}

			if chunk.Done {
				resultCh <- StreamEvent{Type: EventTypeDone}
				return
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

// ListOllamaModels returns available models
func ListOllamaModels(baseURL string) ([]string, error) {
	if baseURL == "" {
		baseURL = "http://localhost:11434"
	}

	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get(baseURL + "/api/tags")
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var data struct {
		Models []struct {
			Name string `json:"name"`
		} `json:"models"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&data); err != nil {
		return nil, err
	}

	models := make([]string, 0, len(data.Models))
	for _, m := range data.Models {
		models = append(models, m.Name)
	}

	return models, nil
}
