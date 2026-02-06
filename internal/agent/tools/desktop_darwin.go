//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// DesktopTool provides mouse and keyboard control for macOS.
// Prefers cliclick (brew install cliclick) but falls back to AppleScript.
type DesktopTool struct {
	useCliclick bool
}

func NewDesktopTool() *DesktopTool {
	_, err := exec.LookPath("cliclick")
	return &DesktopTool{useCliclick: err == nil}
}

func (t *DesktopTool) Name() string { return "desktop" }

func (t *DesktopTool) Description() string {
	return "Control desktop: mouse clicks, keyboard input, scrolling, cursor movement. Best with cliclick (brew install cliclick)."
}

func (t *DesktopTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["click", "double_click", "right_click", "type", "hotkey", "scroll", "move", "drag"],
				"description": "Action to perform"
			},
			"x": {"type": "integer", "description": "X coordinate"},
			"y": {"type": "integer", "description": "Y coordinate"},
			"text": {"type": "string", "description": "Text to type"},
			"keys": {"type": "string", "description": "Keyboard shortcut (e.g., 'cmd+c', 'return')"},
			"direction": {"type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction"},
			"amount": {"type": "integer", "description": "Scroll amount (default: 3)"},
			"to_x": {"type": "integer", "description": "Destination X for drag"},
			"to_y": {"type": "integer", "description": "Destination Y for drag"}
		},
		"required": ["action"]
	}`)
}

func (t *DesktopTool) RequiresApproval() bool { return true }

type desktopInput struct {
	Action    string `json:"action"`
	X         int    `json:"x"`
	Y         int    `json:"y"`
	Text      string `json:"text"`
	Keys      string `json:"keys"`
	Direction string `json:"direction"`
	Amount    int    `json:"amount"`
	ToX       int    `json:"to_x"`
	ToY       int    `json:"to_y"`
}

func (t *DesktopTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p desktopInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "click":
		return t.click(p.X, p.Y, "left", 1)
	case "double_click":
		return t.click(p.X, p.Y, "left", 2)
	case "right_click":
		return t.click(p.X, p.Y, "right", 1)
	case "type":
		return t.typeText(p.Text)
	case "hotkey":
		return t.hotkey(p.Keys)
	case "scroll":
		amount := p.Amount
		if amount == 0 {
			amount = 3
		}
		return t.scroll(p.Direction, amount)
	case "move":
		return t.moveCursor(p.X, p.Y)
	case "drag":
		return t.drag(p.X, p.Y, p.ToX, p.ToY)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *DesktopTool) click(x, y int, button string, count int) (*ToolResult, error) {
	if t.useCliclick {
		var cmd string
		switch button {
		case "right":
			cmd = "rc"
		default:
			if count == 2 {
				cmd = "dc"
			} else {
				cmd = "c"
			}
		}
		if _, err := exec.Command("cliclick", fmt.Sprintf("%s:%d,%d", cmd, x, y)).CombinedOutput(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("cliclick failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Clicked at (%d, %d)", x, y)}, nil
	}
	// AppleScript fallback
	script := fmt.Sprintf(`tell application "System Events" to click at {%d, %d}`, x, y)
	if _, err := exec.Command("osascript", "-e", script).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Click failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Clicked at (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) typeText(text string) (*ToolResult, error) {
	if t.useCliclick {
		if _, err := exec.Command("cliclick", "t:"+text).CombinedOutput(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Type failed: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Typed: %q", text)}, nil
	}
	escaped := strings.ReplaceAll(text, `\`, `\\`)
	escaped = strings.ReplaceAll(escaped, `"`, `\"`)
	script := fmt.Sprintf(`tell application "System Events" to keystroke "%s"`, escaped)
	if _, err := exec.Command("osascript", "-e", script).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Type failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Typed: %q", text)}, nil
}

func (t *DesktopTool) hotkey(keys string) (*ToolResult, error) {
	parts := strings.Split(strings.ToLower(keys), "+")
	var modifiers []string
	var key string
	for _, p := range parts {
		switch p {
		case "cmd", "command":
			modifiers = append(modifiers, "command down")
		case "ctrl", "control":
			modifiers = append(modifiers, "control down")
		case "alt", "option":
			modifiers = append(modifiers, "option down")
		case "shift":
			modifiers = append(modifiers, "shift down")
		default:
			key = p
		}
	}
	var script string
	if len(modifiers) > 0 {
		script = fmt.Sprintf(`tell application "System Events" to keystroke "%s" using {%s}`, key, strings.Join(modifiers, ", "))
	} else {
		script = fmt.Sprintf(`tell application "System Events" to keystroke "%s"`, key)
	}
	if _, err := exec.Command("osascript", "-e", script).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Hotkey failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Pressed: %s", keys)}, nil
}

func (t *DesktopTool) scroll(direction string, amount int) (*ToolResult, error) {
	if !t.useCliclick {
		return &ToolResult{Content: "Scroll requires cliclick (brew install cliclick)", IsError: true}, nil
	}
	var deltaX, deltaY int
	switch direction {
	case "up":
		deltaY = amount
	case "down":
		deltaY = -amount
	case "left":
		deltaX = amount
	case "right":
		deltaX = -amount
	default:
		return &ToolResult{Content: fmt.Sprintf("Invalid direction: %s", direction), IsError: true}, nil
	}
	if _, err := exec.Command("cliclick", fmt.Sprintf("scroll:%d,%d", deltaX, deltaY)).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Scroll failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Scrolled %s by %d", direction, amount)}, nil
}

func (t *DesktopTool) moveCursor(x, y int) (*ToolResult, error) {
	if !t.useCliclick {
		return &ToolResult{Content: "Move requires cliclick (brew install cliclick)", IsError: true}, nil
	}
	if _, err := exec.Command("cliclick", fmt.Sprintf("m:%d,%d", x, y)).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Move failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Moved to (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) drag(fromX, fromY, toX, toY int) (*ToolResult, error) {
	if !t.useCliclick {
		return &ToolResult{Content: "Drag requires cliclick (brew install cliclick)", IsError: true}, nil
	}
	if _, err := exec.Command("cliclick", fmt.Sprintf("dd:%d,%d", fromX, fromY), fmt.Sprintf("du:%d,%d", toX, toY)).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Drag failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Dragged from (%d,%d) to (%d,%d)", fromX, fromY, toX, toY)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewDesktopTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "automation",
	})
}
