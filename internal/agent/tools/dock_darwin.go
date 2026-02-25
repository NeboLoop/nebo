//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// DockTool interacts with the macOS Dock.
// Uses AppleScript and defaults to list, launch, and manage Dock items.
type DockTool struct{}

// NewDockTool creates a new dock tool
func NewDockTool() *DockTool {
	return &DockTool{}
}

func (t *DockTool) Name() string {
	return "dock"
}

func (t *DockTool) Description() string {
	return "Interact with the macOS Dock: list persistent apps, launch apps, show/hide Dock, and check running state."
}

func (t *DockTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: list (show dock items), launch (open app), hide (hide dock), show (show dock), running (list running dock apps)",
				"enum": ["list", "launch", "hide", "show", "running"]
			},
			"app": {
				"type": "string",
				"description": "Application name (for launch action)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *DockTool) RequiresApproval() bool {
	return true
}

type dockInput struct {
	Action string `json:"action"`
	App    string `json:"app"`
}

func (t *DockTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in dockInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "list":
		result, err = t.listDockItems()
	case "launch":
		result, err = t.launchApp(in.App)
	case "hide":
		result, err = t.hideDock()
	case "show":
		result, err = t.showDock()
	case "running":
		result, err = t.listRunning()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

func (t *DockTool) listDockItems() (string, error) {
	// Read persistent apps from Dock plist
	out, err := exec.Command("defaults", "read", "com.apple.dock", "persistent-apps").Output()
	if err != nil {
		return "", fmt.Errorf("failed to read dock items: %v", err)
	}

	// Parse the plist output to extract app names
	lines := strings.Split(string(out), "\n")
	var apps []string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.Contains(line, "\"file-label\"") {
			// Extract the label value: "file-label" = "AppName";
			parts := strings.SplitN(line, "=", 2)
			if len(parts) == 2 {
				name := strings.TrimSpace(parts[1])
				name = strings.Trim(name, "\";")
				name = strings.TrimSpace(name)
				if name != "" {
					apps = append(apps, name)
				}
			}
		}
	}

	if len(apps) == 0 {
		return "No persistent dock items found", nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Dock Items (%d):\n", len(apps)))
	for i, app := range apps {
		sb.WriteString(fmt.Sprintf("  %d. %s\n", i+1, app))
	}
	return sb.String(), nil
}

func (t *DockTool) launchApp(app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	script := fmt.Sprintf(`
		try
			tell application %q to activate
			return "Launched " & %q
		on error errMsg
			return "Failed to launch " & %q & ": " & errMsg
		end try
	`, app, app, app)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to launch app: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DockTool) hideDock() (string, error) {
	script := `
		tell application "System Events"
			set autohide of dock preferences to true
		end tell
		return "Dock auto-hide enabled"
	`
	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to hide dock: %v", err)
	}
	return strings.TrimSpace(string(out)), nil
}

func (t *DockTool) showDock() (string, error) {
	script := `
		tell application "System Events"
			set autohide of dock preferences to false
		end tell
		return "Dock auto-hide disabled"
	`
	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to show dock: %v", err)
	}
	return strings.TrimSpace(string(out)), nil
}

func (t *DockTool) listRunning() (string, error) {
	script := `
		tell application "System Events"
			set runningApps to name of every process whose background only is false
			set appList to ""
			repeat with appName in runningApps
				set appList to appList & appName & return
			end repeat
			return appList
		end tell
	`

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to list running apps: %v", err)
	}

	apps := strings.TrimSpace(string(out))
	if apps == "" {
		return "No running apps found", nil
	}

	lines := strings.Split(apps, "\n")
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Running Apps (%d):\n", len(lines)))
	for i, app := range lines {
		app = strings.TrimSpace(app)
		if app != "" {
			sb.WriteString(fmt.Sprintf("  %d. %s\n", i+1, app))
		}
	}
	return sb.String(), nil
}

