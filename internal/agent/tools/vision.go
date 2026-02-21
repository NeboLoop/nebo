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
	"sync"
)

// AnalyzeFunc is a callback that routes vision analysis through Nebo's provider system.
// It receives base64-encoded image data, the media type, and a prompt, and returns the analysis text.
type AnalyzeFunc func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error)

// VisionTool analyzes images using AI vision capabilities.
// Uses AnalyzeFunc for provider-agnostic vision — works with any configured AI provider.
type VisionTool struct {
	mu          sync.RWMutex
	analyzeFunc AnalyzeFunc
}

type visionInput struct {
	Image  string `json:"image"`  // File path or base64 data or URL
	Prompt string `json:"prompt"` // What to analyze/ask about the image
}

// VisionConfig configures the vision tool.
type VisionConfig struct {
	AnalyzeFunc AnalyzeFunc
}

// NewVisionTool creates a vision tool.
// Wire the AnalyzeFunc after provider loading via SetAnalyzeFunc.
func NewVisionTool(cfg VisionConfig) *VisionTool {
	return &VisionTool{
		analyzeFunc: cfg.AnalyzeFunc,
	}
}

// SetAnalyzeFunc sets the vision analysis callback (wired after provider loading).
func (t *VisionTool) SetAnalyzeFunc(fn AnalyzeFunc) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.analyzeFunc = fn
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

	t.mu.RLock()
	fn := t.analyzeFunc
	t.mu.RUnlock()

	if fn == nil {
		return &ToolResult{
			Content: "No vision provider configured. Add an AI provider with vision support in Settings > Providers.",
			IsError: true,
		}, nil
	}

	// Load and encode the image
	imageData, mediaType, err := t.loadImage(params.Image)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to load image: %v", err),
			IsError: true,
		}, nil
	}

	// Route through the configured provider
	response, err := fn(ctx, imageData, mediaType, params.Prompt)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Vision analysis error: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{Content: response}, nil
}

func (t *VisionTool) loadImage(source string) (string, string, error) {
	// Check if it's already base64 data
	if strings.HasPrefix(source, "data:image/") {
		parts := strings.SplitN(source, ",", 2)
		if len(parts) != 2 {
			return "", "", fmt.Errorf("invalid data URL format")
		}
		mediaType := strings.TrimPrefix(strings.Split(parts[0], ";")[0], "data:")
		return parts[1], mediaType, nil
	}

	// Check if it's a URL — encode as data URL and let the provider handle it
	if strings.HasPrefix(source, "http://") || strings.HasPrefix(source, "https://") {
		return t.loadFromURL(source)
	}

	// Treat as file path
	return t.loadFromFile(source)
}

func (t *VisionTool) loadFromFile(path string) (string, string, error) {
	if strings.HasPrefix(path, "~/") {
		homeDir, _ := os.UserHomeDir()
		path = filepath.Join(homeDir, path[2:])
	}

	data, err := os.ReadFile(path)
	if err != nil {
		return "", "", fmt.Errorf("failed to read file: %w", err)
	}

	mediaType := mediaTypeFromExt(filepath.Ext(path))
	if mediaType == "" {
		return "", "", fmt.Errorf("unsupported image format: %s", filepath.Ext(path))
	}

	return base64.StdEncoding.EncodeToString(data), mediaType, nil
}

func (t *VisionTool) loadFromURL(url string) (string, string, error) {
	// For URLs, pass through as-is — the provider can handle URL images directly.
	// We encode the URL info so the AnalyzeFunc can decide how to handle it.
	// For now, fetch and encode to base64 for maximum provider compatibility.
	resp, err := http.Get(url) //nolint:gosec // URL from user input, validated by caller
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

	mediaType := resp.Header.Get("Content-Type")
	if mediaType == "" || !strings.HasPrefix(mediaType, "image/") {
		mediaType = mediaTypeFromExt(filepath.Ext(url))
	}
	if mediaType == "" {
		mediaType = "image/jpeg"
	}

	return base64.StdEncoding.EncodeToString(data), mediaType, nil
}

func mediaTypeFromExt(ext string) string {
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
