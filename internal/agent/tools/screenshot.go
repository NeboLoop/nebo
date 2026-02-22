package tools

import (
	"bytes"
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
	"github.com/neboloop/nebo/internal/defaults"
)

// ScreenshotTool captures screenshots of the screen or specific displays.
// The "see" action adds annotated element overlays for the desktop automation workflow.
type ScreenshotTool struct{}

type screenshotInput struct {
	Action     string `json:"action"`      // "capture" (default) or "see"
	Display    int    `json:"display"`      // Display number (0 = primary, -1 = all)
	Output     string `json:"output"`       // Output path (optional, returns base64 if empty)
	Format     string `json:"format"`       // Output format: "file", "base64", "both"
	App        string `json:"app"`          // App name for window capture (see action)
	Window     string `json:"window"`       // Window target: "frontmost" or index (see action)
	SnapshotID string `json:"snapshot_id"`  // Retrieve a previous snapshot by ID
}

func NewScreenshotTool() *ScreenshotTool {
	return &ScreenshotTool{}
}

func (t *ScreenshotTool) Name() string {
	return "screenshot"
}

func (t *ScreenshotTool) Description() string {
	return `Capture screenshots or see annotated UI elements for desktop automation.

Actions:
- capture: Take a full-screen or window screenshot. Returns base64 or file path.
- see: Capture a window + overlay numbered element IDs on interactive UI elements.
  Returns annotated image + element list (e.g., B1=button "Save", T2=textfield "Search").
  Use desktop(action: "click", element: "B1") to interact with elements by ID.

Use "see" when you need to find and interact with specific UI elements.
Use "capture" when you just need a visual of the screen.`
}

func (t *ScreenshotTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["capture", "see"],
				"description": "capture: take a screenshot. see: capture + annotate UI elements with IDs for interaction. Default: capture",
				"default": "capture"
			},
			"display": {
				"type": "integer",
				"description": "Display number to capture (0 = primary display, -1 = all displays combined). Default: 0",
				"default": 0
			},
			"output": {
				"type": "string",
				"description": "File path to save the screenshot. If empty, saves to data dir."
			},
			"format": {
				"type": "string",
				"enum": ["file", "base64", "both"],
				"description": "Output format: 'file' saves to disk, 'base64' returns encoded image, 'both' does both. Default: file",
				"default": "file"
			},
			"app": {
				"type": "string",
				"description": "App name to capture (window-level). Omit for full screen. Used with 'see' action."
			},
			"window": {
				"type": "string",
				"description": "Window target: 'frontmost' (default) or a 1-based index. Used with 'see' action.",
				"default": "frontmost"
			},
			"snapshot_id": {
				"type": "string",
				"description": "Retrieve a previous snapshot by ID instead of capturing a new one."
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

	if params.Action == "" {
		params.Action = "capture"
	}

	switch params.Action {
	case "capture":
		return t.executeCapture(params)
	case "see":
		return t.executeSee(ctx, params)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s. Use 'capture' or 'see'.", params.Action),
			IsError: true,
		}, nil
	}
}

func (t *ScreenshotTool) executeCapture(params screenshotInput) (*ToolResult, error) {
	if params.Format == "" {
		params.Format = "file"
	}

	numDisplays := screenshot.NumActiveDisplays()
	if numDisplays == 0 {
		return &ToolResult{Content: "No active displays found", IsError: true}, nil
	}

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
		bounds := screenshot.GetDisplayBounds(0)
		for i := 1; i < numDisplays; i++ {
			b := screenshot.GetDisplayBounds(i)
			bounds = bounds.Union(b)
		}
		img, err = screenshot.CaptureRect(bounds)
	} else {
		bounds := screenshot.GetDisplayBounds(displayNum)
		img, err = screenshot.CaptureRect(bounds)
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to capture screenshot: %v", err),
			IsError: true,
		}, nil
	}

	return t.formatOutput(img, params)
}

func (t *ScreenshotTool) executeSee(ctx context.Context, params screenshotInput) (*ToolResult, error) {
	// If retrieving a previous snapshot
	if params.SnapshotID != "" {
		snap := GetSnapshotStore().Get(params.SnapshotID)
		if snap == nil {
			return &ToolResult{
				Content: fmt.Sprintf("Snapshot %q not found or expired.", params.SnapshotID),
				IsError: true,
			}, nil
		}
		return t.formatSnapshotResult(snap)
	}

	// Capture the screenshot
	var capturedImg image.Image
	var windowBounds Rect
	var windowTitle string

	if params.App != "" {
		// Window-level capture
		windowIndex := 1
		if params.Window != "" && params.Window != "frontmost" {
			if idx, err := parseInt(params.Window); err == nil && idx > 0 {
				windowIndex = idx
			}
		}

		img, bounds, err := CaptureAppWindow(params.App, windowIndex)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to capture %s window: %v", params.App, err),
				IsError: true,
			}, nil
		}
		capturedImg = img
		windowBounds = bounds
		windowTitle = params.App
	} else {
		// Full display capture
		numDisplays := screenshot.NumActiveDisplays()
		if numDisplays == 0 {
			return &ToolResult{Content: "No active displays found", IsError: true}, nil
		}
		displayNum := params.Display
		if displayNum < 0 {
			displayNum = 0
		}
		bounds := screenshot.GetDisplayBounds(displayNum)
		img, err := screenshot.CaptureRect(bounds)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to capture display: %v", err),
				IsError: true,
			}, nil
		}
		capturedImg = img
		windowBounds = Rect{X: bounds.Min.X, Y: bounds.Min.Y, Width: bounds.Dx(), Height: bounds.Dy()}
		windowTitle = fmt.Sprintf("Display %d", displayNum)
	}

	// Get UI tree with element bounds from accessibility
	rawElements := getUITreeWithBounds(params.App, windowBounds)

	// Assign element IDs
	elements := AssignElementIDs(rawElements)

	// Render annotations on the screenshot
	annotatedImg, err := RenderAnnotations(capturedImg, elements)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to render annotations: %v", err),
			IsError: true,
		}, nil
	}

	// Encode images
	rawPNG := encodeImagePNG(capturedImg)
	annotatedPNG := encodeImagePNG(annotatedImg)

	// Build element map
	elemMap := make(map[string]*Element, len(elements))
	elemOrder := make([]string, len(elements))
	for i, elem := range elements {
		elemMap[elem.ID] = elem
		elemOrder[i] = elem.ID
	}

	// Store snapshot
	snapID := fmt.Sprintf("snap-%s", time.Now().Format("20060102-150405"))
	snap := &Snapshot{
		ID:           snapID,
		CreatedAt:    time.Now(),
		App:          params.App,
		WindowTitle:  windowTitle,
		RawPNG:       rawPNG,
		AnnotatedPNG: annotatedPNG,
		Elements:     elemMap,
		ElementOrder: elemOrder,
	}
	GetSnapshotStore().Put(snap)

	return t.formatSnapshotResult(snap)
}

func (t *ScreenshotTool) formatSnapshotResult(snap *Snapshot) (*ToolResult, error) {
	// Save annotated image to files dir for web serving
	dataDir, _ := defaults.DataDir()
	filesDir := filepath.Join(dataDir, "files")
	os.MkdirAll(filesDir, 0755)
	fileName := fmt.Sprintf("screenshot_see_%s.png", time.Now().Format("20060102_150405"))
	filePath := filepath.Join(filesDir, fileName)

	if err := os.WriteFile(filePath, snap.AnnotatedPNG, 0644); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to save annotated screenshot: %v", err),
			IsError: true,
		}, nil
	}

	// Build element list
	elements := make([]*Element, 0, len(snap.ElementOrder))
	for _, id := range snap.ElementOrder {
		if elem, ok := snap.Elements[id]; ok {
			elements = append(elements, elem)
		}
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Snapshot %s captured for %s\n", snap.ID, snap.WindowTitle))
	sb.WriteString(fmt.Sprintf("Saved to: %s\n\n", filePath))
	sb.WriteString(FormatElementList(elements))
	sb.WriteString("\nUse desktop(action: \"click\", element: \"B1\") to interact with elements.")

	return &ToolResult{
		Content:  sb.String(),
		ImageURL: fmt.Sprintf("/api/v1/files/%s", fileName),
	}, nil
}

func (t *ScreenshotTool) formatOutput(img image.Image, params screenshotInput) (*ToolResult, error) {
	bounds := img.Bounds()
	var result strings.Builder
	result.WriteString(fmt.Sprintf("Screenshot captured: %dx%d pixels\n", bounds.Dx(), bounds.Dy()))

	// Always save to file so the web UI can display it via ImageURL.
	// The Content field stays text-only — no markdown image links.
	filePath, fileName, err := t.saveImageToFile(img, params.Output)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to save screenshot: %v", err),
			IsError: true,
		}, nil
	}
	result.WriteString(fmt.Sprintf("Saved to: %s\n", filePath))
	imageURL := fmt.Sprintf("/api/v1/files/%s", fileName)

	// For base64/both formats, also include the encoded data for programmatic use
	if params.Format == "base64" || params.Format == "both" {
		b64, err := imageToBase64(img)
		if err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Failed to encode screenshot: %v", err),
				IsError: true,
			}, nil
		}
		result.WriteString(fmt.Sprintf("Base64 length: %d chars\n", len(b64)))
		result.WriteString(fmt.Sprintf("data:image/png;base64,%s", b64))
	}

	return &ToolResult{Content: result.String(), ImageURL: imageURL}, nil
}

// saveImageToFile saves any image.Image and returns (fullPath, fileName, error).
func (t *ScreenshotTool) saveImageToFile(img image.Image, outputPath string) (string, string, error) {
	var fileName string
	if outputPath == "" {
		dataDir, _ := defaults.DataDir()
		filesDir := filepath.Join(dataDir, "files")
		os.MkdirAll(filesDir, 0755)
		fileName = fmt.Sprintf("screenshot_%s.png", time.Now().Format("20060102_150405"))
		outputPath = filepath.Join(filesDir, fileName)
	} else {
		fileName = filepath.Base(outputPath)
	}

	dir := filepath.Dir(outputPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", "", fmt.Errorf("failed to create directory: %w", err)
	}

	file, err := os.Create(outputPath)
	if err != nil {
		return "", "", fmt.Errorf("failed to create file: %w", err)
	}
	defer file.Close()

	if err := png.Encode(file, img); err != nil {
		return "", "", fmt.Errorf("failed to encode PNG: %w", err)
	}

	return outputPath, fileName, nil
}

func encodeImagePNG(img image.Image) []byte {
	var buf bytes.Buffer
	png.Encode(&buf, img)
	return buf.Bytes()
}

func imageToBase64(img image.Image) (string, error) {
	var buf strings.Builder
	encoder := base64.NewEncoder(base64.StdEncoding, &buf)
	if err := png.Encode(encoder, img); err != nil {
		return "", fmt.Errorf("failed to encode PNG: %w", err)
	}
	encoder.Close()
	return buf.String(), nil
}

func parseInt(s string) (int, error) {
	var n int
	_, err := fmt.Sscanf(s, "%d", &n)
	return n, err
}
