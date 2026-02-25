//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
)

// WindowTool provides macOS window management via AppleScript.
type WindowTool struct{}

func NewWindowTool() *WindowTool { return &WindowTool{} }

func (t *WindowTool) Name() string { return "window" }

func (t *WindowTool) Description() string {
	return "Manage windows: list, focus, move, resize, minimize, maximize, or close windows by app name."
}

func (t *WindowTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "focus", "move", "resize", "minimize", "maximize", "close"],
				"description": "Action to perform"
			},
			"app": {"type": "string", "description": "Application name"},
			"title": {"type": "string", "description": "Window title (partial match)"},
			"x": {"type": "integer", "description": "X position for move"},
			"y": {"type": "integer", "description": "Y position for move"},
			"width": {"type": "integer", "description": "Width for resize"},
			"height": {"type": "integer", "description": "Height for resize"}
		},
		"required": ["action"]
	}`)
}

func (t *WindowTool) RequiresApproval() bool { return true }

type windowInput struct {
	Action string `json:"action"`
	App    string `json:"app"`
	Title  string `json:"title"`
	X      int    `json:"x"`
	Y      int    `json:"y"`
	Width  int    `json:"width"`
	Height int    `json:"height"`
}

func (t *WindowTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p windowInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "list":
		return t.listWindows()
	case "focus":
		return t.focusWindow(p.App, p.Title)
	case "move":
		return t.moveWindow(p.App, p.Title, p.X, p.Y)
	case "resize":
		return t.resizeWindow(p.App, p.Title, p.Width, p.Height)
	case "minimize":
		return t.minimizeWindow(p.App, p.Title)
	case "maximize":
		return t.maximizeWindow(p.App)
	case "close":
		return t.closeWindow(p.App, p.Title)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *WindowTool) listWindows() (*ToolResult, error) {
	script := `set windowList to ""
		tell application "System Events"
			set appList to (name of every process whose visible is true)
			repeat with appName in appList
				try
					tell process appName
						set winCount to count of windows
						if winCount > 0 then
							repeat with i from 1 to winCount
								set win to window i
								set winTitle to name of win
								set winPos to position of win
								set winSize to size of win
								set windowList to windowList & appName & "|||" & winTitle & "|||" & (item 1 of winPos) & "|||" & (item 2 of winPos) & "|||" & (item 1 of winSize) & "|||" & (item 2 of winSize) & "
"
							end repeat
						end if
					end tell
				end try
			end repeat
		end tell
		return windowList`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	lines := strings.Split(strings.TrimSpace(out), "\n")
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Found %d windows:\n\n", len(lines)))
	for _, line := range lines {
		if line == "" {
			continue
		}
		parts := strings.Split(line, "|||")
		if len(parts) >= 6 {
			title := parts[1]
			if len(title) > 50 {
				title = title[:47] + "..."
			}
			x, _ := strconv.Atoi(parts[2])
			y, _ := strconv.Atoi(parts[3])
			w, _ := strconv.Atoi(parts[4])
			h, _ := strconv.Atoi(parts[5])
			sb.WriteString(fmt.Sprintf("• %s\n  Title: %s\n  Position: (%d, %d), Size: %dx%d\n\n", parts[0], title, x, y, w, h))
		}
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *WindowTool) focusWindow(app, title string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	var script string
	if title != "" {
		script = fmt.Sprintf(`tell application "System Events"
			tell process "%s"
				set frontmost to true
				repeat with win in windows
					if name of win contains "%s" then
						perform action "AXRaise" of win
						return "focused"
					end if
				end repeat
			end tell
		end tell
		return "not found"`, app, title)
	} else {
		script = fmt.Sprintf(`tell application "%s" to activate`, app)
	}
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "not found" {
		return &ToolResult{Content: fmt.Sprintf("Window '%s' not found in %s", title, app), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Focused %s", app)}, nil
}

func (t *WindowTool) moveWindow(app, title string, x, y int) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			if (count of windows) > 0 then
				set position of window 1 to {%d, %d}
				return "moved"
			end if
		end tell
	end tell
	return "no windows"`, app, x, y)
	out, err := execAppleScript(script)
	if err != nil || out == "no windows" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Moved window to (%d, %d)", x, y)}, nil
}

func (t *WindowTool) resizeWindow(app, title string, width, height int) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	if width <= 0 || height <= 0 {
		return &ToolResult{Content: "Width and height must be positive", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			if (count of windows) > 0 then
				set size of window 1 to {%d, %d}
				return "resized"
			end if
		end tell
	end tell
	return "no windows"`, app, width, height)
	out, err := execAppleScript(script)
	if err != nil || out == "no windows" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Resized window to %dx%d", width, height)}, nil
}

func (t *WindowTool) minimizeWindow(app, title string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			if (count of windows) > 0 then
				set value of attribute "AXMinimized" of window 1 to true
				return "minimized"
			end if
		end tell
	end tell
	return "no windows"`, app)
	out, err := execAppleScript(script)
	if err != nil || out == "no windows" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: "Minimized window"}, nil
}

func (t *WindowTool) maximizeWindow(app string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	// Get screen size and maximize
	script := fmt.Sprintf(`tell application "Finder"
		set screenBounds to bounds of window of desktop
		set screenWidth to item 3 of screenBounds
		set screenHeight to (item 4 of screenBounds) - 25
	end tell
	tell application "System Events"
		tell process "%s"
			if (count of windows) > 0 then
				set position of window 1 to {0, 25}
				set size of window 1 to {screenWidth, screenHeight}
				return "maximized"
			end if
		end tell
	end tell
	return "no windows"`, app)
	out, err := execAppleScript(script)
	if err != nil || out == "no windows" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: "Maximized window"}, nil
}

func (t *WindowTool) closeWindow(app, title string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			if (count of windows) > 0 then
				click button 1 of window 1
				return "closed"
			end if
		end tell
	end tell
	return "no windows"`, app)
	out, err := execAppleScript(script)
	if err != nil || out == "no windows" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: "Closed window"}, nil
}

