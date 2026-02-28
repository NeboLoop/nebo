//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// WindowTool provides Windows window management via PowerShell.
type WindowTool struct{}

func NewWindowTool() *WindowTool { return &WindowTool{} }

func (t *WindowTool) Name() string { return "window" }

func (t *WindowTool) Description() string {
	return "Manage windows: list, focus, move, resize, minimize, maximize, or close windows."
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
			"app": {"type": "string", "description": "Application/process name"},
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
		return t.listWindows(ctx)
	case "focus":
		return t.focusWindow(ctx, p.App, p.Title)
	case "move":
		return t.moveWindow(ctx, p.App, p.X, p.Y)
	case "resize":
		return t.resizeWindow(ctx, p.App, p.Width, p.Height)
	case "minimize":
		return t.minimizeWindow(ctx, p.App)
	case "maximize":
		return t.maximizeWindow(ctx, p.App)
	case "close":
		return t.closeWindow(ctx, p.App)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *WindowTool) listWindows(ctx context.Context) (*ToolResult, error) {
	script := `
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object {
    $rect = New-Object WinAPI+RECT
    [WinAPI]::GetWindowRect($_.MainWindowHandle, [ref]$rect) | Out-Null
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    "$($_.ProcessName)|||$($_.MainWindowTitle)|||$($rect.Left)|||$($rect.Top)|||$w|||$h"
}
`
	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	var sb strings.Builder
	count := 0
	for _, line := range lines {
		if line == "" {
			continue
		}
		parts := strings.Split(line, "|||")
		if len(parts) >= 6 {
			count++
			title := parts[1]
			if len(title) > 50 {
				title = title[:47] + "..."
			}
			sb.WriteString(fmt.Sprintf("• %s\n  Title: %s\n  Position: (%s, %s), Size: %sx%s\n\n",
				parts[0], title, parts[2], parts[3], parts[4], parts[5]))
		}
	}
	return &ToolResult{Content: fmt.Sprintf("Found %d windows:\n\n%s", count, sb.String())}, nil
}

// findProcessPS returns a PowerShell snippet that finds a process by:
// 1. Exact process name match (e.g., "msedge")
// 2. Partial process name match (e.g., "edge" matches "msedge")
// 3. Window title match (e.g., "Microsoft Edge" matches title containing it)
// 4. FileDescription match (e.g., "Microsoft Edge" matches msedge.exe's description)
// The title parameter, if provided, further filters by window title.
func findProcessPS(app, title string) string {
	escapedApp := strings.ReplaceAll(app, "'", "''")
	titleFilter := ""
	if title != "" {
		escapedTitle := strings.ReplaceAll(title, "'", "''")
		titleFilter = fmt.Sprintf(` | Where-Object { $_.MainWindowTitle -like '*%s*' }`, escapedTitle)
	}

	return fmt.Sprintf(`
$allProcs = Get-Process | Where-Object { $_.MainWindowHandle -ne 0 }
# Try exact process name first
$proc = $allProcs | Where-Object { $_.ProcessName -eq '%s' }%s | Select-Object -First 1
if (-not $proc) {
    # Try partial process name match (case-insensitive)
    $proc = $allProcs | Where-Object { $_.ProcessName -like '*%s*' }%s | Select-Object -First 1
}
if (-not $proc) {
    # Try matching window title
    $proc = $allProcs | Where-Object { $_.MainWindowTitle -like '*%s*' }%s | Select-Object -First 1
}
if (-not $proc) {
    # Try matching FileDescription from the executable's version info
    # This handles human-readable names like "Microsoft Edge" -> msedge.exe
    $proc = $allProcs | Where-Object {
        try { $_.MainModule.FileVersionInfo.FileDescription -like '*%s*' } catch { $false }
    }%s | Select-Object -First 1
}
`, escapedApp, titleFilter, escapedApp, titleFilter, escapedApp, titleFilter, escapedApp, titleFilter)
}

func (t *WindowTool) focusWindow(ctx context.Context, app, title string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
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
%s
if ($proc) {
    [WinAPI]::ShowWindow($proc.MainWindowHandle, 9)
    [WinAPI]::SetForegroundWindow($proc.MainWindowHandle)
    "focused"
} else {
    "not found"
}
`, findProcessPS(app, title))

	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if strings.TrimSpace(string(out)) == "not found" {
		return &ToolResult{Content: fmt.Sprintf("Window for '%s' not found", app), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Focused %s", app)}, nil
}

func (t *WindowTool) moveWindow(ctx context.Context, app string, x, y int) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
}
"@
%s
if ($proc) {
    [WinAPI]::SetWindowPos($proc.MainWindowHandle, [IntPtr]::Zero, %d, %d, 0, 0, 0x0001)
    "moved"
} else {
    "not found"
}
`, findProcessPS(app, ""), x, y)

	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil || strings.TrimSpace(string(out)) == "not found" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Moved window to (%d, %d)", x, y)}, nil
}

func (t *WindowTool) resizeWindow(ctx context.Context, app string, width, height int) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	if width <= 0 || height <= 0 {
		return &ToolResult{Content: "Width and height must be positive", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
%s
if ($proc) {
    $rect = New-Object WinAPI+RECT
    [WinAPI]::GetWindowRect($proc.MainWindowHandle, [ref]$rect) | Out-Null
    [WinAPI]::SetWindowPos($proc.MainWindowHandle, [IntPtr]::Zero, $rect.Left, $rect.Top, %d, %d, 0x0000)
    "resized"
} else {
    "not found"
}
`, findProcessPS(app, ""), width, height)

	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil || strings.TrimSpace(string(out)) == "not found" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Resized window to %dx%d", width, height)}, nil
}

func (t *WindowTool) minimizeWindow(ctx context.Context, app string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
%s
if ($proc) {
    [WinAPI]::ShowWindow($proc.MainWindowHandle, 6)
    "minimized"
} else {
    "not found"
}
`, findProcessPS(app, ""))

	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil || strings.TrimSpace(string(out)) == "not found" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: "Minimized window"}, nil
}

func (t *WindowTool) maximizeWindow(ctx context.Context, app string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
"@
%s
if ($proc) {
    [WinAPI]::ShowWindow($proc.MainWindowHandle, 3)
    "maximized"
} else {
    "not found"
}
`, findProcessPS(app, ""))

	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil || strings.TrimSpace(string(out)) == "not found" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: "Maximized window"}, nil
}

func (t *WindowTool) closeWindow(ctx context.Context, app string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
%s
if ($proc) {
    $proc.CloseMainWindow()
    "closed"
} else {
    "not found"
}
`, findProcessPS(app, ""))

	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil || strings.TrimSpace(string(out)) == "not found" {
		return &ToolResult{Content: "Window not found", IsError: true}, nil
	}
	return &ToolResult{Content: "Closed window"}, nil
}

