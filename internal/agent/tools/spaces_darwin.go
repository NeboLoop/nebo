//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SpacesTool manages macOS Spaces (virtual desktops).
// Uses AppleScript key events and System Events for space switching and window management.
type SpacesTool struct{}

// NewSpacesTool creates a new spaces tool
func NewSpacesTool() *SpacesTool {
	return &SpacesTool{}
}

func (t *SpacesTool) Name() string {
	return "spaces"
}

func (t *SpacesTool) Description() string {
	return "Manage macOS Spaces (virtual desktops): list spaces, switch between spaces, move windows to other spaces."
}

func (t *SpacesTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: list (show spaces info), switch (go to space), move_window (move frontmost window to space)",
				"enum": ["list", "switch", "move_window"]
			},
			"space": {
				"type": "integer",
				"description": "Space number to switch to or move window to (1-based)"
			},
			"direction": {
				"type": "string",
				"description": "Direction to switch: left, right (alternative to space number)",
				"enum": ["left", "right"]
			}
		},
		"required": ["action"]
	}`)
}

func (t *SpacesTool) RequiresApproval() bool {
	return true
}

type spacesInput struct {
	Action    string `json:"action"`
	Space     int    `json:"space"`
	Direction string `json:"direction"`
}

func (t *SpacesTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in spacesInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "list":
		result, err = t.listSpaces()
	case "switch":
		result, err = t.switchSpace(in.Space, in.Direction)
	case "move_window":
		result, err = t.moveWindow(in.Space)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

func (t *SpacesTool) listSpaces() (string, error) {
	// Use defaults to read the number of spaces configuration
	// Note: macOS doesn't expose a direct API for space count,
	// but we can read the spaces plist for info
	out, err := exec.Command("defaults", "read", "com.apple.spaces", "app-bindings").Output()
	appBindings := ""
	if err == nil {
		appBindings = strings.TrimSpace(string(out))
	}

	// Get current space info via AppleScript
	script := `
		tell application "System Events"
			set spaceInfo to ""

			-- Check if Mission Control has multiple spaces configured
			try
				set desktopCount to do shell script "defaults read com.apple.dock workspaces-count 2>/dev/null || echo 'unknown'"
				set spaceInfo to spaceInfo & "Configured desktops: " & desktopCount & return
			end try

			-- Get current frontmost app as space context
			set frontApp to name of first process whose frontmost is true
			set spaceInfo to spaceInfo & "Current space frontmost app: " & frontApp & return

			-- List all visible windows across spaces
			set visibleWindows to ""
			set procs to every process whose visible is true
			repeat with p in procs
				set pName to name of p
				try
					set winCount to count of windows of p
					if winCount > 0 then
						set visibleWindows to visibleWindows & "  " & pName & " (" & winCount & " windows)" & return
					end if
				end try
			end repeat

			set spaceInfo to spaceInfo & return & "Visible apps with windows:" & return & visibleWindows
			return spaceInfo
		end tell
	`

	out2, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to list spaces: %v", err)
	}

	result := strings.TrimSpace(string(out2))
	if appBindings != "" && appBindings != "(null)" {
		result += "\n\nApp-space bindings:\n" + appBindings
	}

	return result, nil
}

func (t *SpacesTool) switchSpace(space int, direction string) (string, error) {
	if direction != "" {
		// Switch by direction using Ctrl+Arrow
		var keyCode int
		switch direction {
		case "left":
			keyCode = 123 // left arrow
		case "right":
			keyCode = 124 // right arrow
		default:
			return "", fmt.Errorf("invalid direction: %s (use 'left' or 'right')", direction)
		}

		script := fmt.Sprintf(`
			tell application "System Events"
				key code %d using control down
			end tell
			return "Switched space %s"
		`, keyCode, direction)

		out, err := exec.Command("osascript", "-e", script).Output()
		if err != nil {
			return "", fmt.Errorf("failed to switch space: %v", err)
		}
		return strings.TrimSpace(string(out)), nil
	}

	if space < 1 || space > 16 {
		return "", fmt.Errorf("space number must be between 1 and 16")
	}

	// Switch to specific space using Ctrl+<number>
	// macOS uses Ctrl+1 through Ctrl+9 for spaces (must be enabled in System Preferences)
	if space <= 9 {
		// Key codes for 1-9: 18, 19, 20, 21, 23, 22, 26, 28, 25
		keyCodes := []int{18, 19, 20, 21, 23, 22, 26, 28, 25}
		script := fmt.Sprintf(`
			tell application "System Events"
				key code %d using control down
			end tell
			return "Switched to Space %d"
		`, keyCodes[space-1], space)

		out, err := exec.Command("osascript", "-e", script).Output()
		if err != nil {
			return "", fmt.Errorf("failed to switch to space %d: %v", space, err)
		}
		return strings.TrimSpace(string(out)), nil
	}

	// For spaces > 9, use directional navigation
	return "", fmt.Errorf("direct switch only supports spaces 1-9; use direction: 'left'/'right' for navigation")
}

func (t *SpacesTool) moveWindow(space int) (string, error) {
	if space < 1 {
		return "", fmt.Errorf("space number is required (1-based)")
	}

	// Move the frontmost window to another space:
	// 1. Enter Mission Control (Ctrl+Up)
	// 2. Drag window to target space
	// Unfortunately, there's no direct API for this.
	// The workaround: use accessibility to move window.
	//
	// Most reliable approach: open Mission Control, then use keyboard
	// But this is fragile. Instead, we'll document the limitation.

	script := fmt.Sprintf(`
		-- Moving windows between spaces requires Mission Control interaction.
		-- This uses the keyboard shortcut approach:
		-- 1. Hold the window (click and hold title bar)
		-- 2. Trigger space switch while holding

		tell application "System Events"
			set frontApp to name of first process whose frontmost is true
			set frontWin to ""
			try
				set frontWin to name of window 1 of (first process whose frontmost is true)
			end try

			-- Trigger Mission Control
			key code 126 using control down
			delay 0.5

			-- Note: Automated window-to-space movement is limited by macOS.
			-- For reliable results, use the keyboard shortcut Ctrl+<space number>
			-- while dragging the window.

			-- Exit Mission Control
			key code 53

			return "Mission Control opened for " & frontApp & ". To move window '" & frontWin & "' to Space %d: drag it to the target space in Mission Control, or use third-party tools like yabai."
		end tell
	`, space)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to move window: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

