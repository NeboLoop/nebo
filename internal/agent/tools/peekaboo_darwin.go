//go:build darwin && !ios

package tools

import (
	"encoding/json"
	"fmt"
	"image"
	"image/png"
	"os"
	"os/exec"
	"strconv"
	"strings"
)

// PeekabooBackend provides an enhanced macOS backend using the Peekaboo CLI
// (https://github.com/steipete/Peekaboo). When installed, it offers higher-fidelity
// accessibility data via native Swift APIs compared to AppleScript.
// Auto-detected at startup; graceful fallback when not installed.
type PeekabooBackend struct {
	available bool
	path      string
}

// NewPeekabooBackend creates a Peekaboo backend, auto-detecting CLI availability.
func NewPeekabooBackend() *PeekabooBackend {
	path, err := exec.LookPath("peekaboo")
	return &PeekabooBackend{
		available: err == nil,
		path:      path,
	}
}

// Available returns true if the Peekaboo CLI is installed.
func (p *PeekabooBackend) Available() bool {
	return p.available
}

// See captures an annotated screenshot with element IDs using Peekaboo.
// Returns a Snapshot ready for storage, or an error if Peekaboo fails.
func (p *PeekabooBackend) See(app string) (*Snapshot, error) {
	if !p.available {
		return nil, fmt.Errorf("peekaboo CLI not installed")
	}

	args := []string{"see"}
	if app != "" {
		args = append(args, "--app", app)
	}
	args = append(args, "--json-output", "--annotate")

	out, err := exec.Command(p.path, args...).Output()
	if err != nil {
		return nil, fmt.Errorf("peekaboo see failed: %v", err)
	}

	return p.parseSeeOutput(out, app)
}

// peekabooSeeOutput represents Peekaboo's JSON output from the see command
type peekabooSeeOutput struct {
	App            string             `json:"app"`
	WindowTitle    string             `json:"windowTitle"`
	ScreenshotPath string             `json:"screenshotPath"`
	Elements       []peekabooElement  `json:"elements"`
}

type peekabooElement struct {
	ID          string  `json:"id"`
	Role        string  `json:"role"`
	Label       string  `json:"label"`
	Value       string  `json:"value"`
	X           float64 `json:"x"`
	Y           float64 `json:"y"`
	Width       float64 `json:"width"`
	Height      float64 `json:"height"`
	Actionable  bool    `json:"actionable"`
}

func (p *PeekabooBackend) parseSeeOutput(data []byte, app string) (*Snapshot, error) {
	var output peekabooSeeOutput
	if err := json.Unmarshal(data, &output); err != nil {
		return nil, fmt.Errorf("failed to parse peekaboo output: %v", err)
	}

	// Load the annotated screenshot
	var rawPNG []byte
	var annotatedPNG []byte
	if output.ScreenshotPath != "" {
		imgData, err := os.ReadFile(output.ScreenshotPath)
		if err == nil {
			annotatedPNG = imgData
			rawPNG = imgData // Peekaboo provides annotated, so both are the same
		}
		// Clean up temp file
		os.Remove(output.ScreenshotPath)
	}

	// Convert Peekaboo elements to Nebo elements
	elements := make(map[string]*Element)
	var elementOrder []string

	for _, pe := range output.Elements {
		elem := &Element{
			ID:         pe.ID,
			Role:       pe.Role,
			Label:      pe.Label,
			Value:      pe.Value,
			Bounds:     Rect{X: int(pe.X), Y: int(pe.Y), Width: int(pe.Width), Height: int(pe.Height)},
			Actionable: pe.Actionable,
		}
		elements[pe.ID] = elem
		elementOrder = append(elementOrder, pe.ID)
	}

	snap := &Snapshot{
		App:          output.App,
		WindowTitle:  output.WindowTitle,
		RawPNG:       rawPNG,
		AnnotatedPNG: annotatedPNG,
		Elements:     elements,
		ElementOrder: elementOrder,
	}

	return snap, nil
}

// Click performs a click on an element by ID using Peekaboo.
func (p *PeekabooBackend) Click(elementID, snapshotID string) error {
	if !p.available {
		return fmt.Errorf("peekaboo CLI not installed")
	}

	args := []string{"click", "--on", elementID}
	if snapshotID != "" {
		args = append(args, "--snapshot", snapshotID)
	}

	out, err := exec.Command(p.path, args...).CombinedOutput()
	if err != nil {
		return fmt.Errorf("peekaboo click failed: %s — %v", string(out), err)
	}

	return nil
}

// CaptureAppWindowPeekaboo captures a window using Peekaboo CLI for higher fidelity.
// Returns the captured image, window bounds, and any error.
func (p *PeekabooBackend) CaptureAppWindowPeekaboo(app string, windowIndex int) (image.Image, Rect, error) {
	if !p.available {
		return nil, Rect{}, fmt.Errorf("peekaboo CLI not installed")
	}

	args := []string{"screenshot"}
	if app != "" {
		args = append(args, "--app", app)
	}
	if windowIndex > 0 {
		args = append(args, "--window", strconv.Itoa(windowIndex))
	}
	args = append(args, "--json-output")

	out, err := exec.Command(p.path, args...).Output()
	if err != nil {
		return nil, Rect{}, fmt.Errorf("peekaboo screenshot failed: %v", err)
	}

	var result struct {
		Path   string `json:"path"`
		X      int    `json:"x"`
		Y      int    `json:"y"`
		Width  int    `json:"width"`
		Height int    `json:"height"`
	}
	if err := json.Unmarshal(out, &result); err != nil {
		return nil, Rect{}, fmt.Errorf("failed to parse peekaboo screenshot output: %v", err)
	}

	bounds := Rect{X: result.X, Y: result.Y, Width: result.Width, Height: result.Height}

	if result.Path == "" {
		return nil, bounds, fmt.Errorf("peekaboo returned no screenshot path")
	}

	f, err := os.Open(result.Path)
	if err != nil {
		return nil, bounds, fmt.Errorf("failed to open screenshot: %v", err)
	}
	defer f.Close()
	defer os.Remove(result.Path) // Clean up temp file

	img, err := png.Decode(f)
	if err != nil {
		return nil, bounds, fmt.Errorf("failed to decode screenshot: %v", err)
	}

	return img, bounds, nil
}

// GetAccessibilityTree retrieves the UI tree with bounds using Peekaboo.
func (p *PeekabooBackend) GetAccessibilityTree(app string) ([]RawElement, error) {
	if !p.available {
		return nil, fmt.Errorf("peekaboo CLI not installed")
	}

	args := []string{"accessibility"}
	if app != "" {
		args = append(args, "--app", app)
	}
	args = append(args, "--json-output")

	out, err := exec.Command(p.path, args...).Output()
	if err != nil {
		return nil, fmt.Errorf("peekaboo accessibility failed: %v", err)
	}

	var peekElements []peekabooElement
	if err := json.Unmarshal(out, &peekElements); err != nil {
		return nil, fmt.Errorf("failed to parse peekaboo accessibility output: %v", err)
	}

	// Convert to RawElement
	var elements []RawElement
	for _, pe := range peekElements {
		elements = append(elements, RawElement{
			Role:       strings.ToLower(pe.Role),
			Title:      pe.Label,
			Value:      pe.Value,
			Position:   Rect{X: int(pe.X), Y: int(pe.Y), Width: int(pe.Width), Height: int(pe.Height)},
			Actionable: pe.Actionable,
		})
	}

	return elements, nil
}

// peekabooBackendSingleton is the global Peekaboo backend instance.
// Initialized lazily on first access.
var peekabooBackendSingleton *PeekabooBackend

// GetPeekabooBackend returns the shared Peekaboo backend instance.
func GetPeekabooBackend() *PeekabooBackend {
	if peekabooBackendSingleton == nil {
		peekabooBackendSingleton = NewPeekabooBackend()
	}
	return peekabooBackendSingleton
}
