package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// WebTool fetches content from URLs
type WebTool struct {
	client *http.Client
}

// NewWebTool creates a new web tool
func NewWebTool() *WebTool {
	return &WebTool{
		client: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

// Name returns the tool name
func (t *WebTool) Name() string {
	return "web"
}

// Description returns the tool description
func (t *WebTool) Description() string {
	return `Fetch content from a URL. Returns the raw content of the page.
Use this to access web pages, APIs, or download files.
Note: This is a simple HTTP GET request - for complex interactions, use the browser tool.`
}

// Schema returns the JSON schema for the tool input
func (t *WebTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"url": {
				"type": "string",
				"description": "The URL to fetch"
			},
			"headers": {
				"type": "object",
				"description": "Optional headers to include in the request",
				"additionalProperties": {
					"type": "string"
				}
			},
			"method": {
				"type": "string",
				"description": "HTTP method (GET, POST, etc). Default: GET"
			},
			"body": {
				"type": "string",
				"description": "Request body for POST/PUT requests"
			}
		},
		"required": ["url"]
	}`)
}

// WebInput represents the tool input
type WebInput struct {
	URL     string            `json:"url"`
	Headers map[string]string `json:"headers"`
	Method  string            `json:"method"`
	Body    string            `json:"body"`
}

// Execute fetches the URL
func (t *WebTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in WebInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	if in.URL == "" {
		return &ToolResult{
			Content: "Error: url is required",
			IsError: true,
		}, nil
	}

	// Default to GET
	method := in.Method
	if method == "" {
		method = "GET"
	}

	// Create request
	var body io.Reader
	if in.Body != "" {
		body = strings.NewReader(in.Body)
	}

	req, err := http.NewRequestWithContext(ctx, method, in.URL, body)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error creating request: %v", err),
			IsError: true,
		}, nil
	}

	// Set default user agent
	req.Header.Set("User-Agent", "Nebo/1.0")

	// Set custom headers
	for k, v := range in.Headers {
		req.Header.Set(k, v)
	}

	// Make request
	resp, err := t.client.Do(req)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error fetching URL: %v", err),
			IsError: true,
		}, nil
	}
	defer resp.Body.Close()

	// Read response
	content, err := io.ReadAll(resp.Body)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error reading response: %v", err),
			IsError: true,
		}, nil
	}

	// Truncate very long responses
	const maxContent = 100000
	result := string(content)
	if len(result) > maxContent {
		result = result[:maxContent] + "\n... (content truncated)"
	}

	// Add status info
	header := fmt.Sprintf("HTTP %d %s\nContent-Type: %s\nContent-Length: %d\n\n",
		resp.StatusCode,
		resp.Status,
		resp.Header.Get("Content-Type"),
		len(content),
	)

	return &ToolResult{
		Content: header + result,
		IsError: resp.StatusCode >= 400,
	}, nil
}

// RequiresApproval returns false - reading web is safe
func (t *WebTool) RequiresApproval() bool {
	return false
}
