//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// AppTool provides macOS application control via AppleScript.
type AppTool struct{}

func NewAppTool() *AppTool { return &AppTool{} }

func (t *AppTool) Name() string { return "app" }

func (t *AppTool) Description() string {
	return "Control applications: list running apps, launch, quit, activate, hide, get info, and interact with menus."
}

func (t *AppTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "launch", "quit", "quit_all", "activate", "hide", "info", "menu", "frontmost"],
				"description": "Action to perform"
			},
			"name": {"type": "string", "description": "Application name (e.g., 'Safari', 'Terminal')"},
			"path": {"type": "string", "description": "Application path for launch (optional if name provided)"},
			"menu_path": {"type": "string", "description": "Menu path for menu action (e.g., 'File > New Window')"},
			"force": {"type": "boolean", "description": "Force quit without saving"}
		},
		"required": ["action"]
	}`)
}

func (t *AppTool) RequiresApproval() bool { return true }

type appInput struct {
	Action   string `json:"action"`
	Name     string `json:"name"`
	Path     string `json:"path"`
	MenuPath string `json:"menu_path"`
	Force    bool   `json:"force"`
}

func (t *AppTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p appInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "list":
		return t.listApps()
	case "launch":
		return t.launchApp(p.Name, p.Path)
	case "quit":
		return t.quitApp(p.Name, p.Force)
	case "quit_all":
		return t.quitAllApps()
	case "activate":
		return t.activateApp(p.Name)
	case "hide":
		return t.hideApp(p.Name)
	case "info":
		return t.getAppInfo(p.Name)
	case "menu":
		return t.clickMenu(p.Name, p.MenuPath)
	case "frontmost":
		return t.getFrontmostApp()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *AppTool) listApps() (*ToolResult, error) {
	script := `tell application "System Events"
		set appInfo to ""
		repeat with proc in (every process whose background only is false)
			set appName to name of proc
			set appFront to frontmost of proc
			set winCount to 0
			try
				set winCount to count of windows of proc
			end try
			set frontMark to ""
			if appFront then set frontMark to " (frontmost)"
			set appInfo to appInfo & appName & frontMark & " - " & winCount & " windows
"
		end repeat
	end tell
	return appInfo`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	lines := strings.Split(strings.TrimSpace(out), "\n")
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Running Applications (%d):\n\n", len(lines)))
	for _, line := range lines {
		if line != "" {
			sb.WriteString(fmt.Sprintf("• %s\n", line))
		}
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *AppTool) launchApp(name, path string) (*ToolResult, error) {
	if name == "" && path == "" {
		return &ToolResult{Content: "Name or path is required", IsError: true}, nil
	}
	var cmd *exec.Cmd
	if path != "" {
		cmd = exec.Command("open", path)
	} else {
		cmd = exec.Command("open", "-a", name)
	}
	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to launch: %v", err), IsError: true}, nil
	}
	appName := name
	if appName == "" {
		appName = path
	}
	return &ToolResult{Content: fmt.Sprintf("Launched %s", appName)}, nil
}

func (t *AppTool) quitApp(name string, force bool) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	var script string
	if force {
		script = fmt.Sprintf(`tell application "System Events"
			set targetProcess to first process whose name is "%s"
			do shell script "kill -9 " & (unix id of targetProcess)
		end tell`, name)
	} else {
		script = fmt.Sprintf(`tell application "%s" to quit`, name)
	}
	if _, err := execAppleScript(script); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to quit: %v", err), IsError: true}, nil
	}
	if force {
		return &ToolResult{Content: fmt.Sprintf("Force quit %s", name)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Quit %s", name)}, nil
}

func (t *AppTool) quitAllApps() (*ToolResult, error) {
	script := `tell application "System Events"
		set appList to name of every process whose background only is false
		set quitCount to 0
		repeat with appName in appList
			if appName is not "Finder" and appName is not "osascript" then
				try
					tell application appName to quit
					set quitCount to quitCount + 1
				end try
			end if
		end repeat
		return quitCount as string
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Requested quit for %s applications", strings.TrimSpace(out))}, nil
}

func (t *AppTool) activateApp(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "%s" to activate`, name)
	if _, err := execAppleScript(script); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Activated %s", name)}, nil
}

func (t *AppTool) hideApp(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "System Events"
		set visible of process "%s" to false
	end tell`, name)
	if _, err := execAppleScript(script); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Hidden %s", name)}, nil
}

func (t *AppTool) getAppInfo(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "System Events"
		set proc to first process whose name is "%s"
		set appName to name of proc
		set appPath to ""
		try
			set appPath to POSIX path of (file of proc as alias)
		end try
		set appPID to unix id of proc
		set appFront to frontmost of proc
		set winCount to 0
		try
			set winCount to count of windows of proc
		end try
		set appVisible to visible of proc
	end tell
	set info to "Name: " & appName & return
	set info to info & "Path: " & appPath & return
	set info to info & "PID: " & appPID & return
	set info to info & "Frontmost: " & appFront & return
	set info to info & "Visible: " & appVisible & return
	set info to info & "Windows: " & winCount
	return info`, name)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Application Info:\n\n%s", strings.TrimSpace(out))}, nil
}

func (t *AppTool) clickMenu(name, menuPath string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	if menuPath == "" {
		return &ToolResult{Content: "menu_path is required (e.g., 'File > New Window')", IsError: true}, nil
	}
	parts := strings.Split(menuPath, ">")
	for i := range parts {
		parts[i] = strings.TrimSpace(parts[i])
	}
	if len(parts) < 2 {
		return &ToolResult{Content: "menu_path must have menu and item (e.g., 'File > New')", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "%s" to activate
		delay 0.2
		tell application "System Events"
			tell process "%s"
				click menu item "%s" of menu 1 of menu bar item "%s" of menu bar 1
			end tell
		end tell`, name, name, parts[len(parts)-1], parts[0])
	if _, err := execAppleScript(script); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to click menu: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Clicked menu: %s > %s", name, menuPath)}, nil
}

func (t *AppTool) getFrontmostApp() (*ToolResult, error) {
	script := `tell application "System Events"
		set frontApp to first process whose frontmost is true
		set appName to name of frontApp
		set winName to ""
		try
			set winName to name of window 1 of frontApp
		end try
	end tell
	return appName & "|||" & winName`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	parts := strings.Split(strings.TrimSpace(out), "|||")
	result := fmt.Sprintf("Frontmost application: %s", parts[0])
	if len(parts) > 1 && parts[1] != "" {
		result += fmt.Sprintf("\nActive window: %s", parts[1])
	}
	return &ToolResult{Content: result}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewAppTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "system",
	})
}
