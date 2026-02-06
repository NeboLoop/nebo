//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// DesktopTool provides Linux mouse and keyboard control via xdotool or ydotool.
type DesktopTool struct {
	backend string // "xdotool", "ydotool", or ""
}

func NewDesktopTool() *DesktopTool {
	t := &DesktopTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *DesktopTool) detectBackend() string {
	// xdotool works with X11
	if _, err := exec.LookPath("xdotool"); err == nil {
		return "xdotool"
	}
	// ydotool works with Wayland
	if _, err := exec.LookPath("ydotool"); err == nil {
		return "ydotool"
	}
	return ""
}

func (t *DesktopTool) Name() string { return "desktop" }

func (t *DesktopTool) Description() string {
	switch t.backend {
	case "xdotool":
		return "Control Desktop (using xdotool) - mouse clicks, keyboard input, window management for X11."
	case "ydotool":
		return "Control Desktop (using ydotool) - mouse clicks, keyboard input for Wayland."
	default:
		return "Control Desktop - requires xdotool (X11) or ydotool (Wayland) to be installed."
	}
}

func (t *DesktopTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["click", "double_click", "right_click", "type", "hotkey", "scroll", "move", "drag", "get_mouse_pos", "get_active_window"],
				"description": "Action to perform"
			},
			"x": {"type": "integer", "description": "X coordinate"},
			"y": {"type": "integer", "description": "Y coordinate"},
			"text": {"type": "string", "description": "Text to type"},
			"keys": {"type": "string", "description": "Keyboard shortcut (e.g., 'ctrl+c', 'Return', 'alt+Tab')"},
			"direction": {"type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction"},
			"amount": {"type": "integer", "description": "Scroll amount (default: 3)"},
			"to_x": {"type": "integer", "description": "Destination X for drag"},
			"to_y": {"type": "integer", "description": "Destination Y for drag"},
			"delay": {"type": "integer", "description": "Delay between keystrokes in ms (default: 12)"}
		},
		"required": ["action"]
	}`)
}

func (t *DesktopTool) RequiresApproval() bool { return true }

type desktopInputLinux struct {
	Action    string `json:"action"`
	X         int    `json:"x"`
	Y         int    `json:"y"`
	Text      string `json:"text"`
	Keys      string `json:"keys"`
	Direction string `json:"direction"`
	Amount    int    `json:"amount"`
	ToX       int    `json:"to_x"`
	ToY       int    `json:"to_y"`
	Delay     int    `json:"delay"`
}

func (t *DesktopTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No desktop control backend available. Please install one of:\n" +
				"  - xdotool: sudo apt install xdotool (for X11)\n" +
				"  - ydotool: sudo apt install ydotool (for Wayland)\n" +
				"For ydotool on Wayland, you may need to start ydotoold service.",
			IsError: true,
		}, nil
	}

	var p desktopInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch t.backend {
	case "xdotool":
		return t.executeXdotool(ctx, p)
	case "ydotool":
		return t.executeYdotool(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// xdotool implementation (X11)
// ============================================================================

func (t *DesktopTool) executeXdotool(ctx context.Context, p desktopInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "click":
		return t.xdotoolClick(ctx, p.X, p.Y, 1)
	case "double_click":
		return t.xdotoolClick(ctx, p.X, p.Y, 2)
	case "right_click":
		return t.xdotoolRightClick(ctx, p.X, p.Y)
	case "type":
		return t.xdotoolType(ctx, p.Text, p.Delay)
	case "hotkey":
		return t.xdotoolHotkey(ctx, p.Keys)
	case "scroll":
		return t.xdotoolScroll(ctx, p.Direction, p.Amount)
	case "move":
		return t.xdotoolMove(ctx, p.X, p.Y)
	case "drag":
		return t.xdotoolDrag(ctx, p.X, p.Y, p.ToX, p.ToY)
	case "get_mouse_pos":
		return t.xdotoolGetMousePos(ctx)
	case "get_active_window":
		return t.xdotoolGetActiveWindow(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *DesktopTool) xdotoolClick(ctx context.Context, x, y, count int) (*ToolResult, error) {
	args := []string{}
	if x != 0 || y != 0 {
		args = append(args, "mousemove", strconv.Itoa(x), strconv.Itoa(y))
	}
	args = append(args, "click", "--repeat", strconv.Itoa(count), "1")

	cmd := exec.CommandContext(ctx, "xdotool", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Click failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	if count > 1 {
		return &ToolResult{Content: fmt.Sprintf("Double-clicked at (%d, %d)", x, y)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Clicked at (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) xdotoolRightClick(ctx context.Context, x, y int) (*ToolResult, error) {
	args := []string{}
	if x != 0 || y != 0 {
		args = append(args, "mousemove", strconv.Itoa(x), strconv.Itoa(y))
	}
	args = append(args, "click", "3")

	cmd := exec.CommandContext(ctx, "xdotool", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Right-click failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Right-clicked at (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) xdotoolType(ctx context.Context, text string, delay int) (*ToolResult, error) {
	if text == "" {
		return &ToolResult{Content: "Text is required for type action", IsError: true}, nil
	}
	if delay <= 0 {
		delay = 12
	}

	cmd := exec.CommandContext(ctx, "xdotool", "type", "--delay", strconv.Itoa(delay), text)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Type failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Typed: %s", text)}, nil
}

func (t *DesktopTool) xdotoolHotkey(ctx context.Context, keys string) (*ToolResult, error) {
	if keys == "" {
		return &ToolResult{Content: "Keys are required for hotkey action", IsError: true}, nil
	}

	// Convert common key names to xdotool format
	keys = strings.ReplaceAll(keys, "cmd", "super")
	keys = strings.ReplaceAll(keys, "option", "alt")

	cmd := exec.CommandContext(ctx, "xdotool", "key", keys)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Hotkey failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Pressed: %s", keys)}, nil
}

func (t *DesktopTool) xdotoolScroll(ctx context.Context, direction string, amount int) (*ToolResult, error) {
	if amount <= 0 {
		amount = 3
	}

	button := "4" // up
	switch direction {
	case "down":
		button = "5"
	case "left":
		button = "6"
	case "right":
		button = "7"
	}

	cmd := exec.CommandContext(ctx, "xdotool", "click", "--repeat", strconv.Itoa(amount), button)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Scroll failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Scrolled %s %d clicks", direction, amount)}, nil
}

func (t *DesktopTool) xdotoolMove(ctx context.Context, x, y int) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "xdotool", "mousemove", strconv.Itoa(x), strconv.Itoa(y))
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Move failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Moved mouse to (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) xdotoolDrag(ctx context.Context, fromX, fromY, toX, toY int) (*ToolResult, error) {
	// Move to start, press button, move to end, release button
	cmd := exec.CommandContext(ctx, "xdotool", "mousemove", strconv.Itoa(fromX), strconv.Itoa(fromY),
		"mousedown", "1", "mousemove", strconv.Itoa(toX), strconv.Itoa(toY), "mouseup", "1")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Drag failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Dragged from (%d, %d) to (%d, %d)", fromX, fromY, toX, toY)}, nil
}

func (t *DesktopTool) xdotoolGetMousePos(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "xdotool", "getmouselocation", "--shell")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get mouse position: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *DesktopTool) xdotoolGetActiveWindow(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "xdotool", "getactivewindow", "getwindowname")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get active window: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Active window: %s", strings.TrimSpace(string(out)))}, nil
}

// ============================================================================
// ydotool implementation (Wayland)
// ============================================================================

func (t *DesktopTool) executeYdotool(ctx context.Context, p desktopInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "click":
		return t.ydotoolClick(ctx, p.X, p.Y, 1)
	case "double_click":
		return t.ydotoolClick(ctx, p.X, p.Y, 2)
	case "right_click":
		return t.ydotoolRightClick(ctx, p.X, p.Y)
	case "type":
		return t.ydotoolType(ctx, p.Text, p.Delay)
	case "hotkey":
		return t.ydotoolHotkey(ctx, p.Keys)
	case "scroll":
		return t.ydotoolScroll(ctx, p.Direction, p.Amount)
	case "move":
		return t.ydotoolMove(ctx, p.X, p.Y)
	case "drag":
		return &ToolResult{Content: "Drag is not well supported in ydotool. Use xdotool with X11 instead.", IsError: true}, nil
	case "get_mouse_pos", "get_active_window":
		return &ToolResult{Content: "This action is not supported with ydotool on Wayland.", IsError: true}, nil
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *DesktopTool) ydotoolClick(ctx context.Context, x, y, count int) (*ToolResult, error) {
	if x != 0 || y != 0 {
		cmd := exec.CommandContext(ctx, "ydotool", "mousemove", "--absolute", "-x", strconv.Itoa(x), "-y", strconv.Itoa(y))
		cmd.Run()
	}

	args := []string{"click", "0xC0"}
	for i := 0; i < count; i++ {
		cmd := exec.CommandContext(ctx, "ydotool", args...)
		if _, err := cmd.CombinedOutput(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Click failed: %v", err), IsError: true}, nil
		}
	}

	if count > 1 {
		return &ToolResult{Content: fmt.Sprintf("Double-clicked at (%d, %d)", x, y)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Clicked at (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) ydotoolRightClick(ctx context.Context, x, y int) (*ToolResult, error) {
	if x != 0 || y != 0 {
		cmd := exec.CommandContext(ctx, "ydotool", "mousemove", "--absolute", "-x", strconv.Itoa(x), "-y", strconv.Itoa(y))
		cmd.Run()
	}

	cmd := exec.CommandContext(ctx, "ydotool", "click", "0xC1")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Right-click failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Right-clicked at (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) ydotoolType(ctx context.Context, text string, delay int) (*ToolResult, error) {
	if text == "" {
		return &ToolResult{Content: "Text is required for type action", IsError: true}, nil
	}

	args := []string{"type", text}
	if delay > 0 {
		args = []string{"type", "--key-delay", strconv.Itoa(delay), text}
	}

	cmd := exec.CommandContext(ctx, "ydotool", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Type failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Typed: %s", text)}, nil
}

func (t *DesktopTool) ydotoolHotkey(ctx context.Context, keys string) (*ToolResult, error) {
	if keys == "" {
		return &ToolResult{Content: "Keys are required for hotkey action", IsError: true}, nil
	}

	cmd := exec.CommandContext(ctx, "ydotool", "key", keys)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Hotkey failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Pressed: %s", keys)}, nil
}

func (t *DesktopTool) ydotoolScroll(ctx context.Context, direction string, amount int) (*ToolResult, error) {
	if amount <= 0 {
		amount = 3
	}

	dy := amount
	dx := 0
	switch direction {
	case "up":
		dy = -amount
	case "left":
		dx = -amount
		dy = 0
	case "right":
		dx = amount
		dy = 0
	}

	cmd := exec.CommandContext(ctx, "ydotool", "mousemove", "-w", "-x", strconv.Itoa(dx), "-y", strconv.Itoa(dy))
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Scroll failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Scrolled %s %d", direction, amount)}, nil
}

func (t *DesktopTool) ydotoolMove(ctx context.Context, x, y int) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "ydotool", "mousemove", "--absolute", "-x", strconv.Itoa(x), "-y", strconv.Itoa(y))
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Move failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Moved mouse to (%d, %d)", x, y)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewDesktopTool(),
		Platforms: []string{PlatformLinux},
		Category:  "automation",
	})
}
