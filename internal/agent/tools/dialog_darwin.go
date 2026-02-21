//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// DialogTool detects and interacts with system dialogs (sheets, alerts) on macOS.
// Uses AppleScript System Events to enumerate dialog UI elements.
type DialogTool struct{}

// NewDialogTool creates a new dialog tool
func NewDialogTool() *DialogTool {
	return &DialogTool{}
}

func (t *DialogTool) Name() string {
	return "dialog"
}

func (t *DialogTool) Description() string {
	return "Detect and interact with system dialogs, sheets, and alerts. List dialog buttons and fields, click buttons, fill text fields, and dismiss dialogs."
}

func (t *DialogTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: detect (find dialogs), list (buttons/fields in dialog), click (click button), fill (fill text field), dismiss (close dialog)",
				"enum": ["detect", "list", "click", "fill", "dismiss"]
			},
			"app": {
				"type": "string",
				"description": "Application name (e.g., 'Safari', 'Finder')"
			},
			"button": {
				"type": "string",
				"description": "Button name to click (for click action)"
			},
			"field": {
				"type": "integer",
				"description": "Text field index (1-based) for fill action"
			},
			"value": {
				"type": "string",
				"description": "Value to fill in the text field"
			}
		},
		"required": ["action"]
	}`)
}

func (t *DialogTool) RequiresApproval() bool {
	return true
}

type dialogInput struct {
	Action string `json:"action"`
	App    string `json:"app"`
	Button string `json:"button"`
	Field  int    `json:"field"`
	Value  string `json:"value"`
}

func (t *DialogTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in dialogInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "detect":
		result, err = t.detectDialogs(in.App)
	case "list":
		result, err = t.listDialogElements(in.App)
	case "click":
		result, err = t.clickButton(in.App, in.Button)
	case "fill":
		result, err = t.fillField(in.App, in.Field, in.Value)
	case "dismiss":
		result, err = t.dismissDialog(in.App)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

func (t *DialogTool) detectDialogs(app string) (string, error) {
	if app == "" {
		// Detect dialogs in frontmost app
		app = "frontmost application"
	}

	script := fmt.Sprintf(`
		tell application "System Events"
			set targetApp to %s
			set appName to name of targetApp
			set dialogInfo to ""

			-- Check for sheets
			try
				set sheetCount to count of sheets of window 1 of process appName
				if sheetCount > 0 then
					set dialogInfo to dialogInfo & "Sheet detected in " & appName & return
				end if
			end try

			-- Check for standard dialogs (windows with buttons but small size)
			try
				set winList to every window of process appName
				repeat with w in winList
					set winRole to role of w
					if winRole is "AXDialog" or winRole is "AXSheet" then
						set winTitle to ""
						try
							set winTitle to title of w
						end try
						set dialogInfo to dialogInfo & "Dialog: " & winTitle & " (role: " & winRole & ")" & return
					end if
				end repeat
			end try

			if dialogInfo is "" then
				return "No dialogs detected in " & appName
			end if
			return dialogInfo
		end tell
	`, t.appRef(app))

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to detect dialogs: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) listDialogElements(app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	script := fmt.Sprintf(`
		tell application "System Events"
			set targetProc to process %q
			set result to ""

			-- Check sheets first
			try
				set s to sheet 1 of window 1 of targetProc
				set result to result & "=== Sheet ===" & return

				-- List buttons
				try
					set btns to every button of s
					repeat with i from 1 to count of btns
						set btnName to name of item i of btns
						set result to result & "Button " & i & ": " & btnName & return
					end repeat
				end try

				-- List text fields
				try
					set fields to every text field of s
					repeat with i from 1 to count of fields
						set fieldVal to value of item i of fields
						set result to result & "Field " & i & ": " & fieldVal & return
					end repeat
				end try

				-- List static text
				try
					set texts to every static text of s
					repeat with i from 1 to count of texts
						set textVal to value of item i of texts
						set result to result & "Text: " & textVal & return
					end repeat
				end try

				return result
			end try

			-- Try dialog windows
			try
				set winList to every window of targetProc
				repeat with w in winList
					if role of w is "AXDialog" or role of w is "AXSheet" then
						set result to result & "=== Dialog: " & (title of w) & " ===" & return

						try
							set btns to every button of w
							repeat with i from 1 to count of btns
								set btnName to name of item i of btns
								set result to result & "Button " & i & ": " & btnName & return
							end repeat
						end try

						try
							set fields to every text field of w
							repeat with i from 1 to count of fields
								set fieldVal to value of item i of fields
								set result to result & "Field " & i & ": " & fieldVal & return
							end repeat
						end try
					end if
				end repeat
			end try

			if result is "" then
				return "No dialog elements found in " & (name of targetProc)
			end if
			return result
		end tell
	`, app)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to list dialog elements: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) clickButton(app, button string) (string, error) {
	if app == "" || button == "" {
		return "", fmt.Errorf("app and button name are required")
	}

	script := fmt.Sprintf(`
		tell application "System Events"
			set targetProc to process %q

			-- Try sheet first
			try
				click button %q of sheet 1 of window 1 of targetProc
				return "Clicked button %s in sheet"
			end try

			-- Try dialog windows
			try
				set winList to every window of targetProc
				repeat with w in winList
					if role of w is "AXDialog" or role of w is "AXSheet" then
						try
							click button %q of w
							return "Clicked button %s in dialog"
						end try
					end if
				end repeat
			end try

			-- Try any window
			try
				click button %q of window 1 of targetProc
				return "Clicked button %s in window"
			end try

			return "Button %s not found in " & (name of targetProc)
		end tell
	`, app, button, button, button, button, button, button, button)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to click button: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) fillField(app string, field int, value string) (string, error) {
	if app == "" || value == "" {
		return "", fmt.Errorf("app and value are required")
	}
	if field < 1 {
		field = 1
	}

	escapedValue := strings.ReplaceAll(value, `"`, `\"`)

	script := fmt.Sprintf(`
		tell application "System Events"
			set targetProc to process %q

			-- Try sheet first
			try
				set value of text field %d of sheet 1 of window 1 of targetProc to "%s"
				return "Filled field %d in sheet"
			end try

			-- Try dialog windows
			try
				set winList to every window of targetProc
				repeat with w in winList
					if role of w is "AXDialog" or role of w is "AXSheet" then
						try
							set value of text field %d of w to "%s"
							return "Filled field %d in dialog"
						end try
					end if
				end repeat
			end try

			-- Try any window
			try
				set value of text field %d of window 1 of targetProc to "%s"
				return "Filled field %d in window"
			end try

			return "Field %d not found in " & (name of targetProc)
		end tell
	`, app, field, escapedValue, field, field, escapedValue, field, field, escapedValue, field, field)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to fill field: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) dismissDialog(app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	// Try common dismiss patterns: Cancel, Close, OK, Done
	for _, btn := range []string{"Cancel", "Close", "OK", "Done"} {
		result, err := t.clickButton(app, btn)
		if err == nil && !strings.Contains(result, "not found") {
			return fmt.Sprintf("Dismissed dialog with %q button", btn), nil
		}
	}

	// Try pressing Escape
	script := fmt.Sprintf(`
		tell application "System Events"
			set frontmost of process %q to true
			key code 53
		end tell
		return "Sent Escape key to dismiss dialog"
	`, app)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to dismiss dialog: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) appRef(app string) string {
	if app == "" || app == "frontmost application" {
		return "first process whose frontmost is true"
	}
	return fmt.Sprintf("process %q", app)
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewDialogTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "automation",
	})
}
