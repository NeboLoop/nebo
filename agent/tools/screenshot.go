package tools

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"image"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/kbinani/screenshot"
)

// ScreenshotTool captures screenshots of the screen or specific displays
type ScreenshotTool struct{}

type screenshotInput struct {
	Display  int    `json:"display"`  // Display number (0 = primary, -1 = all)
	Output   string `json:"output"`   // Output path (optional, returns base64 if empty)
	Format   string `json:"format"`   // Output format: "file", "base64", "both"
}

func NewScreenshotTool() *ScreenshotTool {
	return &ScreenshotTool{}
}

func (t *ScreenshotTool) Name() string {
	return "screenshot"
}

func (t *ScreenshotTool) Description() string {
	return "Capture a screenshot of the screen or a specific display. Returns the image as base64 or saves to file."
}

func (t *ScreenshotTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"display": {
				"type": "integer",
				"description": "Display number to capture (0 = primary display, -1 = all displays combined). Default: 0",
				"default": 0
			},
			"output": {
				"type": "string",
				"description": "File path to save the screenshot. If empty, returns base64 encoded image."
			},
			"format": {
				"type": "string",
				"enum": ["file", "base64", "both"],
				"description": "Output format: 'file' saves to disk, 'base64' returns encoded image, 'both' does both. Default: base64",
				"default": "base64"
			}
		}
	}`)
}

func (t *ScreenshotTool) RequiresApproval() bool {
	return false
}

func (t *ScreenshotTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params screenshotInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to parse input: %v", err),
			IsError: true,
		}, nil
	}

	// Defaults
	if params.Format == "" {
		params.Format = "base64"
	}

	// Get number of displays
	numDisplays := screenshot.NumActiveDisplays()
	if numDisplays == 0 {
		return &ToolResult{
			Content: "No active displays found",
			IsError: true,
		}, nil
	}

	// Determine which display to capture
	displayNum := params.Display
	if displayNum < -1 || displayNum >= numDisplays {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid display number %d. Available displays: 0-%d (or -1 for all)", displayNum, numDisplays-1),
			IsError: true,
		}, nil
	}

	var img *image.RGBA
	var err error

	if displayNum == -1 {
		// Capture all displays combined
		bounds := screenshot.GetDisplayBounds(0)
		for i := 1; i < numDisplays; i++ {
			b := screenshot.GetDisplayBounds(i)
			bounds = bounds.Union(b)
		}
		img, err = screenshot.CaptureRect(bounds)
	} else {
		// Capture specific display
		bounds := screenshot.GetDisplayBounds(displayNum)
		img, err = screenshot.CaptureRect(bounds)
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to capture screenshot: %v", err),
			IsError: true,
		}, nil
	}

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Screenshot captured: %dx%d pixels\n", img.Bounds().Dx(), img.Bounds().Dy()))

	// Handle output based on format
	switch params.Format {
	case "file":
		filePath, err := t.saveToFile(img, params.Output)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to save screenshot: %v", err),
				IsError: true,
			}, nil
		}
		result.WriteString(fmt.Sprintf("Saved to: %s", filePath))

	case "base64":
		b64, err := t.toBase64(img)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to encode screenshot: %v", err),
				IsError: true,
			}, nil
		}
		result.WriteString(fmt.Sprintf("Base64 image (data:image/png;base64,%s)", b64))

	case "both":
		filePath, err := t.saveToFile(img, params.Output)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to save screenshot: %v", err),
				IsError: true,
			}, nil
		}
		b64, err := t.toBase64(img)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to encode screenshot: %v", err),
				IsError: true,
			}, nil
		}
		result.WriteString(fmt.Sprintf("Saved to: %s\n", filePath))
		result.WriteString(fmt.Sprintf("Base64 image (data:image/png;base64,%s)", b64))
	}

	return &ToolResult{
		Content: result.String(),
		IsError: false,
	}, nil
}

func (t *ScreenshotTool) saveToFile(img *image.RGBA, outputPath string) (string, error) {
	if outputPath == "" {
		// Generate default path
		homeDir, _ := os.UserHomeDir()
		screenshotsDir := filepath.Join(homeDir, ".nebo", "screenshots")
		os.MkdirAll(screenshotsDir, 0755)
		outputPath = filepath.Join(screenshotsDir, fmt.Sprintf("screenshot_%s.png", time.Now().Format("20060102_150405")))
	}

	// Ensure directory exists
	dir := filepath.Dir(outputPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", fmt.Errorf("failed to create directory: %w", err)
	}

	file, err := os.Create(outputPath)
	if err != nil {
		return "", fmt.Errorf("failed to create file: %w", err)
	}
	defer file.Close()

	if err := png.Encode(file, img); err != nil {
		return "", fmt.Errorf("failed to encode PNG: %w", err)
	}

	return outputPath, nil
}

func (t *ScreenshotTool) toBase64(img *image.RGBA) (string, error) {
	var buf strings.Builder
	encoder := base64.NewEncoder(base64.StdEncoding, &buf)

	if err := png.Encode(encoder, img); err != nil {
		return "", fmt.Errorf("failed to encode PNG: %w", err)
	}
	encoder.Close()

	return buf.String(), nil
}
