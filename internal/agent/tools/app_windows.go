//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// AppTool provides Windows application control via PowerShell.
type AppTool struct{}

func NewAppTool() *AppTool { return &AppTool{} }

func (t *AppTool) Name() string { return "desktop" }

func (t *AppTool) Description() string {
	return "Control applications: list running apps, launch, quit, activate, and get app info."
}

func (t *AppTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "launch", "quit", "activate", "info", "frontmost"],
				"description": "Action to perform"
			},
			"name": {"type": "string", "description": "Application name or process name"},
			"path": {"type": "string", "description": "Application path for launch"},
			"force": {"type": "boolean", "description": "Force quit without saving"}
		},
		"required": ["action"]
	}`)
}

func (t *AppTool) RequiresApproval() bool { return true }

type appInput struct {
	Action string `json:"action"`
	Name   string `json:"name"`
	Path   string `json:"path"`
	Force  bool   `json:"force"`
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
	case "activate":
		return t.activateApp(p.Name)
	case "info":
		return t.getAppInfo(p.Name)
	case "frontmost":
		return t.getFrontmostApp()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *AppTool) listApps() (*ToolResult, error) {
	script := `
Get-Process | Where-Object { $_.MainWindowTitle -ne '' } |
    Select-Object ProcessName, Id, MainWindowTitle, @{N='Memory';E={[math]::Round($_.WorkingSet64/1MB,1)}} |
    ForEach-Object { "$($_.ProcessName) (PID: $($_.Id)) - $($_.MainWindowTitle) [$($_.Memory) MB]" }
`
	out, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
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
		cmd = exec.Command("cmd", "/C", "start", "", path)
	} else {
		// Try to find and launch by name
		cmd = exec.Command("cmd", "/C", "start", "", name)
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

	var cmd *exec.Cmd
	if force {
		cmd = exec.Command("taskkill", "/F", "/IM", name+"*")
	} else {
		cmd = exec.Command("taskkill", "/IM", name+"*")
	}

	if err := cmd.Run(); err != nil {
		// Try with process name without wildcard
		if force {
			cmd = exec.Command("taskkill", "/F", "/IM", name+".exe")
		} else {
			cmd = exec.Command("taskkill", "/IM", name+".exe")
		}
		if err2 := cmd.Run(); err2 != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to quit: %v", err), IsError: true}, nil
		}
	}

	if force {
		return &ToolResult{Content: fmt.Sprintf("Force quit %s", name)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Quit %s", name)}, nil
}

func (t *AppTool) activateApp(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
$proc = Get-Process -Name '%s' -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if ($proc) {
    [WinAPI]::ShowWindow($proc.MainWindowHandle, 9)
    [WinAPI]::SetForegroundWindow($proc.MainWindowHandle)
    "Activated"
} else {
    "Not found"
}
`, name)

	out, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	result := strings.TrimSpace(string(out))
	if result == "Not found" {
		return &ToolResult{Content: fmt.Sprintf("Application '%s' not found or has no window", name), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Activated %s", name)}, nil
}

func (t *AppTool) getAppInfo(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$proc = Get-Process -Name '%s' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($proc) {
    "Name: $($proc.ProcessName)"
    "PID: $($proc.Id)"
    "Path: $($proc.Path)"
    "Memory: $([math]::Round($proc.WorkingSet64/1MB,1)) MB"
    "CPU Time: $($proc.TotalProcessorTime)"
    "Window Title: $($proc.MainWindowTitle)"
    "Threads: $($proc.Threads.Count)"
    "Start Time: $($proc.StartTime)"
} else {
    "Not found"
}
`, name)

	out, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	result := strings.TrimSpace(string(out))
	if result == "Not found" {
		return &ToolResult{Content: fmt.Sprintf("Application '%s' not found", name), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Application Info:\n\n%s", result)}, nil
}

func (t *AppTool) getFrontmostApp() (*ToolResult, error) {
	script := `
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
}
"@
$hwnd = [WinAPI]::GetForegroundWindow()
$title = New-Object System.Text.StringBuilder 256
[WinAPI]::GetWindowText($hwnd, $title, 256) | Out-Null
$processId = 0
[WinAPI]::GetWindowThreadProcessId($hwnd, [ref]$processId) | Out-Null
$proc = Get-Process -Id $processId -ErrorAction SilentlyContinue
"$($proc.ProcessName)|||$($title.ToString())"
`
	out, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	parts := strings.Split(strings.TrimSpace(string(out)), "|||")
	result := fmt.Sprintf("Frontmost application: %s", parts[0])
	if len(parts) > 1 && parts[1] != "" {
		result += fmt.Sprintf("\nActive window: %s", parts[1])
	}
	return &ToolResult{Content: result}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewAppTool(),
		Platforms: []string{PlatformWindows},
		Category:  "system",
	})
}
