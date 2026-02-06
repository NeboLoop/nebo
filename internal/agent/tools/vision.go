package tools

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

// VisionTool analyzes images using AI vision capabilities
type VisionTool struct {
	apiKey   string
	model    string
	endpoint string
}

type visionInput struct {
	Image       string `json:"image"`       // File path or base64 data or URL
	Prompt      string `json:"prompt"`      // What to analyze/ask about the image
	MaxTokens   int    `json:"max_tokens"`  // Max response tokens (default: 1024)
}

// VisionConfig configures the vision tool
type VisionConfig struct {
	APIKey   string
	Model    string // Should be set from models.yaml config
	Endpoint string // Default: https://api.anthropic.com/v1/messages
}

// NewVisionTool creates a vision tool
// Model should be provided from models.yaml config - do NOT hardcode model IDs
func NewVisionTool(cfg VisionConfig) *VisionTool {
	if cfg.Endpoint == "" {
		cfg.Endpoint = "https://api.anthropic.com/v1/messages"
	}
	return &VisionTool{
		apiKey:   cfg.APIKey,
		model:    cfg.Model,
		endpoint: cfg.Endpoint,
	}
}

func (t *VisionTool) Name() string {
	return "vision"
}

func (t *VisionTool) Description() string {
	return "Analyze an image using AI vision. Can describe images, read text, identify objects, answer questions about image content. Accepts file paths, URLs, or base64 encoded images."
}

func (t *VisionTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"image": {
				"type": "string",
				"description": "Image source: file path (e.g., '/path/to/image.png'), URL (e.g., 'https://example.com/image.jpg'), or base64 data (e.g., 'data:image/png;base64,...')"
			},
			"prompt": {
				"type": "string",
				"description": "What to analyze or ask about the image. Default: 'Describe this image in detail.'",
				"default": "Describe this image in detail."
			},
			"max_tokens": {
				"type": "integer",
				"description": "Maximum tokens in response. Default: 1024",
				"default": 1024
			}
		},
		"required": ["image"]
	}`)
}

func (t *VisionTool) RequiresApproval() bool {
	return false
}

func (t *VisionTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params visionInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to parse input: %v", err),
			IsError: true,
		}, nil
	}

	if params.Image == "" {
		return &ToolResult{
			Content: "Image parameter is required",
			IsError: true,
		}, nil
	}

	if params.Prompt == "" {
		params.Prompt = "Describe this image in detail."
	}

	if params.MaxTokens == 0 {
		params.MaxTokens = 1024
	}

	// Load and encode the image
	imageData, mediaType, err := t.loadImage(params.Image)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to load image: %v", err),
			IsError: true,
		}, nil
	}

	// Call vision API
	response, err := t.callVisionAPI(ctx, imageData, mediaType, params.Prompt, params.MaxTokens)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Vision API error: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: response,
		IsError: false,
	}, nil
}

func (t *VisionTool) loadImage(source string) (string, string, error) {
	// Check if it's already base64 data
	if strings.HasPrefix(source, "data:image/") {
		// Parse data URL
		parts := strings.SplitN(source, ",", 2)
		if len(parts) != 2 {
			return "", "", fmt.Errorf("invalid data URL format")
		}
		// Extract media type
		mediaType := strings.TrimPrefix(strings.Split(parts[0], ";")[0], "data:")
		return parts[1], mediaType, nil
	}

	// Check if it's a URL
	if strings.HasPrefix(source, "http://") || strings.HasPrefix(source, "https://") {
		return t.loadFromURL(source)
	}

	// Treat as file path
	return t.loadFromFile(source)
}

func (t *VisionTool) loadFromFile(path string) (string, string, error) {
	// Expand ~ to home directory
	if strings.HasPrefix(path, "~/") {
		homeDir, _ := os.UserHomeDir()
		path = filepath.Join(homeDir, path[2:])
	}

	data, err := os.ReadFile(path)
	if err != nil {
		return "", "", fmt.Errorf("failed to read file: %w", err)
	}

	// Determine media type from extension
	mediaType := t.mediaTypeFromExt(filepath.Ext(path))
	if mediaType == "" {
		return "", "", fmt.Errorf("unsupported image format: %s", filepath.Ext(path))
	}

	return base64.StdEncoding.EncodeToString(data), mediaType, nil
}

func (t *VisionTool) loadFromURL(url string) (string, string, error) {
	resp, err := http.Get(url)
	if err != nil {
		return "", "", fmt.Errorf("failed to fetch URL: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", "", fmt.Errorf("HTTP error: %s", resp.Status)
	}

	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", "", fmt.Errorf("failed to read response: %w", err)
	}

	// Get media type from Content-Type header or URL
	mediaType := resp.Header.Get("Content-Type")
	if mediaType == "" || !strings.HasPrefix(mediaType, "image/") {
		mediaType = t.mediaTypeFromExt(filepath.Ext(url))
	}
	if mediaType == "" {
		mediaType = "image/jpeg" // Default assumption
	}

	return base64.StdEncoding.EncodeToString(data), mediaType, nil
}

func (t *VisionTool) mediaTypeFromExt(ext string) string {
	switch strings.ToLower(ext) {
	case ".png":
		return "image/png"
	case ".jpg", ".jpeg":
		return "image/jpeg"
	case ".gif":
		return "image/gif"
	case ".webp":
		return "image/webp"
	default:
		return ""
	}
}

func (t *VisionTool) callVisionAPI(ctx context.Context, imageData, mediaType, prompt string, maxTokens int) (string, error) {
	if t.apiKey == "" {
		return "", fmt.Errorf("API key not configured for vision tool")
	}

	// Anthropic Messages API format with vision
	requestBody := map[string]interface{}{
		"model":      t.model,
		"max_tokens": maxTokens,
		"messages": []map[string]interface{}{
			{
				"role": "user",
				"content": []map[string]interface{}{
					{
						"type": "image",
						"source": map[string]string{
							"type":         "base64",
							"media_type":   mediaType,
							"data":         imageData,
						},
					},
					{
						"type": "text",
						"text": prompt,
					},
				},
			},
		},
	}

	jsonBody, err := json.Marshal(requestBody)
	if err != nil {
		return "", fmt.Errorf("failed to marshal request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, "POST", t.endpoint, strings.NewReader(string(jsonBody)))
	if err != nil {
		return "", fmt.Errorf("failed to create request: %w", err)
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("x-api-key", t.apiKey)
	req.Header.Set("anthropic-version", "2023-06-01")

	client := &http.Client{}
	resp, err := client.Do(req)
	if err != nil {
		return "", fmt.Errorf("API request failed: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("failed to read response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("API error (%d): %s", resp.StatusCode, string(body))
	}

	// Parse Anthropic response
	var result struct {
		Content []struct {
			Type string `json:"type"`
			Text string `json:"text"`
		} `json:"content"`
		Error struct {
			Message string `json:"message"`
		} `json:"error"`
	}

	if err := json.Unmarshal(body, &result); err != nil {
		return "", fmt.Errorf("failed to parse response: %w", err)
	}

	if result.Error.Message != "" {
		return "", fmt.Errorf("API error: %s", result.Error.Message)
	}

	// Extract text from response
	var texts []string
	for _, content := range result.Content {
		if content.Type == "text" {
			texts = append(texts, content.Text)
		}
	}

	if len(texts) == 0 {
		return "", fmt.Errorf("no text content in response")
	}

	return strings.Join(texts, "\n"), nil
}
