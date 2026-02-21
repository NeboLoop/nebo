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
	return "Control desktop: mouse clicks, keyboard input, scrolling, cursor movement. Use element IDs from screenshot(action: see) for precise targeting."
}

func (t *DesktopTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["click", "double_click", "right_click", "type", "hotkey", "scroll", "move", "drag", "paste"],
				"description": "Action to perform"
			},
			"x": {"type": "integer", "description": "X coordinate"},
			"y": {"type": "integer", "description": "Y coordinate"},
			"text": {"type": "string", "description": "Text to type or paste"},
			"keys": {"type": "string", "description": "Keyboard shortcut (e.g., 'cmd+c', 'return')"},
			"direction": {"type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction"},
			"amount": {"type": "integer", "description": "Scroll amount (default: 3)"},
			"to_x": {"type": "integer", "description": "Destination X for drag"},
			"to_y": {"type": "integer", "description": "Destination Y for drag"},
			"element": {"type": "string", "description": "Element ID from screenshot see action (e.g., B3, T2). Replaces x/y."},
			"snapshot_id": {"type": "string", "description": "Snapshot to look up element in. Default: most recent."}
		},
		"required": ["action"]
	}`)
}

func (t *DesktopTool) RequiresApproval() bool { return true }

type desktopInput struct {
	Action     string `json:"action"`
	X          int    `json:"x"`
	Y          int    `json:"y"`
	Text       string `json:"text"`
	Keys       string `json:"keys"`
	Direction  string `json:"direction"`
	Amount     int    `json:"amount"`
	ToX        int    `json:"to_x"`
	ToY        int    `json:"to_y"`
	Element    string `json:"element"`
	SnapshotID string `json:"snapshot_id"`
}

func (t *DesktopTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p desktopInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Resolve element ID to x/y coordinates
	if p.Element != "" {
		elem, _, err := GetSnapshotStore().LookupElement(p.Element, p.SnapshotID)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Element lookup failed: %v", err), IsError: true}, nil
		}
		cx, cy := elem.Bounds.Center()
		p.X = cx
		p.Y = cy
	}

	switch p.Action {
	case "click":
		return t.click(p.X, p.Y, "left", 1)
	case "double_click":
		return t.click(p.X, p.Y, "left", 2)
	case "right_click":
		return t.click(p.X, p.Y, "right", 1)
	case "type":
		if p.Element != "" {
			// Click element first to focus, then type
			if result, _ := t.click(p.X, p.Y, "left", 1); result.IsError {
				return result, nil
			}
		}
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
	case "paste":
		return t.paste(p.Text, p.X, p.Y, p.Element != "")
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

func (t *DesktopTool) paste(text string, x, y int, hasElement bool) (*ToolResult, error) {
	if text == "" {
		return &ToolResult{Content: "Text is required for paste action", IsError: true}, nil
	}

	// Save current clipboard, set new content, paste, restore
	script := fmt.Sprintf(`
-- Save clipboard
set oldClip to ""
try
	set oldClip to the clipboard as text
end try

-- Set new clipboard content
set the clipboard to "%s"
delay 0.05

-- Paste
tell application "System Events" to keystroke "v" using {command down}
delay 0.1

-- Restore clipboard
try
	set the clipboard to oldClip
end try
`, strings.ReplaceAll(strings.ReplaceAll(text, `\`, `\\`), `"`, `\"`))

	// Click element first to focus if targeting an element
	if hasElement {
		if result, _ := t.click(x, y, "left", 1); result.IsError {
			return result, nil
		}
	}

	if _, err := exec.Command("osascript", "-e", script).CombinedOutput(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Paste failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Pasted text into element at (%d, %d)", x, y)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewDesktopTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "automation",
	})
}
