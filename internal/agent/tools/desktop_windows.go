//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// DesktopTool provides Windows mouse and keyboard control via PowerShell and .NET.
type DesktopTool struct{}

func NewDesktopTool() *DesktopTool {
	return &DesktopTool{}
}

func (t *DesktopTool) Name() string { return "desktop" }

func (t *DesktopTool) Description() string {
	return "Control Desktop (using PowerShell) - mouse clicks, keyboard input, scrolling, cursor movement."
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
			"keys": {"type": "string", "description": "Keyboard shortcut (e.g., 'ctrl+c', 'enter', 'alt+tab')"},
			"direction": {"type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction"},
			"amount": {"type": "integer", "description": "Scroll amount (default: 3)"},
			"to_x": {"type": "integer", "description": "Destination X for drag"},
			"to_y": {"type": "integer", "description": "Destination Y for drag"},
			"delay": {"type": "integer", "description": "Delay between keystrokes in ms"}
		},
		"required": ["action"]
	}`)
}

func (t *DesktopTool) RequiresApproval() bool { return true }

type desktopInputWin struct {
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
	var p desktopInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "click":
		return t.click(ctx, p.X, p.Y, false)
	case "double_click":
		return t.doubleClick(ctx, p.X, p.Y)
	case "right_click":
		return t.click(ctx, p.X, p.Y, true)
	case "type":
		return t.typeText(ctx, p.Text, p.Delay)
	case "hotkey":
		return t.hotkey(ctx, p.Keys)
	case "scroll":
		return t.scroll(ctx, p.Direction, p.Amount)
	case "move":
		return t.move(ctx, p.X, p.Y)
	case "drag":
		return t.drag(ctx, p.X, p.Y, p.ToX, p.ToY)
	case "get_mouse_pos":
		return t.getMousePos(ctx)
	case "get_active_window":
		return t.getActiveWindow(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *DesktopTool) click(ctx context.Context, x, y int, rightClick bool) (*ToolResult, error) {
	button := "MOUSEEVENTF_LEFTDOWN"
	buttonUp := "MOUSEEVENTF_LEFTUP"
	if rightClick {
		button = "MOUSEEVENTF_RIGHTDOWN"
		buttonUp = "MOUSEEVENTF_RIGHTUP"
	}

	script := fmt.Sprintf(`
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MouseOps {
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, int dwExtraInfo);
    public const uint MOUSEEVENTF_LEFTDOWN = 0x02;
    public const uint MOUSEEVENTF_LEFTUP = 0x04;
    public const uint MOUSEEVENTF_RIGHTDOWN = 0x08;
    public const uint MOUSEEVENTF_RIGHTUP = 0x10;
}
"@
[MouseOps]::SetCursorPos(%d, %d)
Start-Sleep -Milliseconds 50
[MouseOps]::mouse_event([MouseOps]::%s, 0, 0, 0, 0)
Start-Sleep -Milliseconds 50
[MouseOps]::mouse_event([MouseOps]::%s, 0, 0, 0, 0)
Write-Output "Clicked"
`, x, y, button, buttonUp)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Click failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	clickType := "Clicked"
	if rightClick {
		clickType = "Right-clicked"
	}
	return &ToolResult{Content: fmt.Sprintf("%s at (%d, %d)", clickType, x, y)}, nil
}

func (t *DesktopTool) doubleClick(ctx context.Context, x, y int) (*ToolResult, error) {
	script := fmt.Sprintf(`
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MouseOps {
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, int dwExtraInfo);
    public const uint MOUSEEVENTF_LEFTDOWN = 0x02;
    public const uint MOUSEEVENTF_LEFTUP = 0x04;
}
"@
[MouseOps]::SetCursorPos(%d, %d)
Start-Sleep -Milliseconds 50
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
Start-Sleep -Milliseconds 100
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
Write-Output "Double-clicked"
`, x, y)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Double-click failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Double-clicked at (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) typeText(ctx context.Context, text string, delay int) (*ToolResult, error) {
	if text == "" {
		return &ToolResult{Content: "Text is required for type action", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait("%s")
Write-Output "Typed"
`, escapeDesktopSendKeys(text))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Type failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Typed: %s", text)}, nil
}

func (t *DesktopTool) hotkey(ctx context.Context, keys string) (*ToolResult, error) {
	if keys == "" {
		return &ToolResult{Content: "Keys are required for hotkey action", IsError: true}, nil
	}

	// Convert to SendKeys format
	sendKeys := convertToSendKeys(keys)

	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait("%s")
Write-Output "Pressed"
`, sendKeys)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Hotkey failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Pressed: %s", keys)}, nil
}

func (t *DesktopTool) scroll(ctx context.Context, direction string, amount int) (*ToolResult, error) {
	if amount <= 0 {
		amount = 3
	}

	// Scroll amount in wheel clicks (120 = 1 notch)
	scrollAmount := amount * 120
	if direction == "down" {
		scrollAmount = -scrollAmount
	}

	script := fmt.Sprintf(`
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MouseOps {
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, int dwExtraInfo);
    public const uint MOUSEEVENTF_WHEEL = 0x0800;
}
"@
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_WHEEL, 0, 0, %d, 0)
Write-Output "Scrolled"
`, scrollAmount)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Scroll failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Scrolled %s %d clicks", direction, amount)}, nil
}

func (t *DesktopTool) move(ctx context.Context, x, y int) (*ToolResult, error) {
	script := fmt.Sprintf(`
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MouseOps {
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int X, int Y);
}
"@
[MouseOps]::SetCursorPos(%d, %d)
Write-Output "Moved"
`, x, y)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Move failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Moved mouse to (%d, %d)", x, y)}, nil
}

func (t *DesktopTool) drag(ctx context.Context, fromX, fromY, toX, toY int) (*ToolResult, error) {
	script := fmt.Sprintf(`
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MouseOps {
    [DllImport("user32.dll")]
    public static extern bool SetCursorPos(int X, int Y);
    [DllImport("user32.dll")]
    public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, int dwExtraInfo);
    public const uint MOUSEEVENTF_LEFTDOWN = 0x02;
    public const uint MOUSEEVENTF_LEFTUP = 0x04;
}
"@
[MouseOps]::SetCursorPos(%d, %d)
Start-Sleep -Milliseconds 50
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
Start-Sleep -Milliseconds 50
[MouseOps]::SetCursorPos(%d, %d)
Start-Sleep -Milliseconds 50
[MouseOps]::mouse_event([MouseOps]::MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)
Write-Output "Dragged"
`, fromX, fromY, toX, toY)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Drag failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Dragged from (%d, %d) to (%d, %d)", fromX, fromY, toX, toY)}, nil
}

func (t *DesktopTool) getMousePos(ctx context.Context) (*ToolResult, error) {
	script := `
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class MouseOps {
    [StructLayout(LayoutKind.Sequential)]
    public struct POINT { public int X; public int Y; }
    [DllImport("user32.dll")]
    public static extern bool GetCursorPos(out POINT lpPoint);
}
"@
$point = New-Object MouseOps+POINT
[MouseOps]::GetCursorPos([ref]$point) | Out-Null
Write-Output "X=$($point.X) Y=$($point.Y)"
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get mouse position: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *DesktopTool) getActiveWindow(ctx context.Context) (*ToolResult, error) {
	script := `
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class WindowOps {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", CharSet = CharSet.Auto, SetLastError = true)]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);
}
"@
$hwnd = [WindowOps]::GetForegroundWindow()
$title = New-Object System.Text.StringBuilder 256
[WindowOps]::GetWindowText($hwnd, $title, 256) | Out-Null
Write-Output $title.ToString()
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get active window: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Active window: %s", strings.TrimSpace(string(out)))}, nil
}

func escapeDesktopSendKeys(s string) string {
	// Escape special SendKeys characters
	s = strings.ReplaceAll(s, "+", "{+}")
	s = strings.ReplaceAll(s, "^", "{^}")
	s = strings.ReplaceAll(s, "%", "{%}")
	s = strings.ReplaceAll(s, "~", "{~}")
	s = strings.ReplaceAll(s, "(", "{(}")
	s = strings.ReplaceAll(s, ")", "{)}")
	s = strings.ReplaceAll(s, "[", "{[}")
	s = strings.ReplaceAll(s, "]", "{]}")
	s = strings.ReplaceAll(s, "{", "{{}")
	s = strings.ReplaceAll(s, "}", "{}}")
	return s
}

func convertToSendKeys(keys string) string {
	// Convert common key combinations to SendKeys format
	keys = strings.ToLower(keys)
	parts := strings.Split(keys, "+")

	var result strings.Builder
	for _, part := range parts {
		part = strings.TrimSpace(part)
		switch part {
		case "ctrl", "control":
			result.WriteString("^")
		case "alt":
			result.WriteString("%")
		case "shift":
			result.WriteString("+")
		case "win", "cmd", "super":
			result.WriteString("^{ESC}") // Windows key approximation
		case "enter", "return":
			result.WriteString("{ENTER}")
		case "tab":
			result.WriteString("{TAB}")
		case "esc", "escape":
			result.WriteString("{ESC}")
		case "backspace", "back":
			result.WriteString("{BACKSPACE}")
		case "delete", "del":
			result.WriteString("{DELETE}")
		case "home":
			result.WriteString("{HOME}")
		case "end":
			result.WriteString("{END}")
		case "pageup", "pgup":
			result.WriteString("{PGUP}")
		case "pagedown", "pgdn":
			result.WriteString("{PGDN}")
		case "up":
			result.WriteString("{UP}")
		case "down":
			result.WriteString("{DOWN}")
		case "left":
			result.WriteString("{LEFT}")
		case "right":
			result.WriteString("{RIGHT}")
		case "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12":
			result.WriteString("{" + strings.ToUpper(part) + "}")
		case "space":
			result.WriteString(" ")
		default:
			if len(part) == 1 {
				result.WriteString(part)
			} else {
				result.WriteString("{" + strings.ToUpper(part) + "}")
			}
		}
	}
	return result.String()
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewDesktopTool(),
		Platforms: []string{PlatformWindows},
		Category:  "automation",
	})
}
