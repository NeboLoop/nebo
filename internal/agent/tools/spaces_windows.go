//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SpacesTool manages Windows Virtual Desktops.
// Uses PowerShell with keybd_event P/Invoke for keyboard shortcuts (Win+Ctrl+Arrow, etc.)
// since SendKeys cannot send Windows key combinations.
type SpacesTool struct{}

// NewSpacesTool creates a new spaces tool
func NewSpacesTool() *SpacesTool {
	return &SpacesTool{}
}

func (t *SpacesTool) Name() string {
	return "spaces"
}

func (t *SpacesTool) Description() string {
	return "Manage Windows Virtual Desktops: list visible windows/desktop info, switch between desktops, move windows to adjacent desktops."
}

func (t *SpacesTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: list (show desktop info), switch (go to adjacent desktop or create new), move_window (move frontmost window to adjacent desktop)",
				"enum": ["list", "switch", "move_window"]
			},
			"direction": {
				"type": "string",
				"description": "Direction to switch: left, right, or 'new' to create a new desktop",
				"enum": ["left", "right", "new"]
			}
		},
		"required": ["action"]
	}`)
}

func (t *SpacesTool) RequiresApproval() bool {
	return true
}

type spacesInputWin struct {
	Action    string `json:"action"`
	Direction string `json:"direction"`
}

func (t *SpacesTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in spacesInputWin
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "list":
		result, err = t.listDesktops(ctx)
	case "switch":
		result, err = t.switchDesktop(ctx, in.Direction)
	case "move_window":
		result, err = t.moveWindow(ctx, in.Direction)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

// keybdEventPS returns the PowerShell C# type definition for keyboard simulation via keybd_event.
// This is needed because SendKeys cannot send Windows key combinations.
func keybdEventPS() string {
	return `
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class KbdOps {
    [DllImport("user32.dll")]
    public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);

    public const byte VK_LWIN    = 0x5B;
    public const byte VK_CONTROL = 0x11;
    public const byte VK_SHIFT   = 0x10;
    public const byte VK_LEFT    = 0x25;
    public const byte VK_RIGHT   = 0x27;
    public const byte VK_D       = 0x44;
    public const byte VK_F4      = 0x73;
    public const uint KEYEVENTF_KEYUP = 0x0002;
}
"@
`
}

func (t *SpacesTool) listDesktops(ctx context.Context) (string, error) {
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
$frontTitle = $title.ToString()

$result = "Windows Virtual Desktop Info:" + [char]10
$result += "Current desktop frontmost window: $frontTitle" + [char]10
$result += [char]10

# List all visible windows with main handles
$result += "Visible apps with windows:" + [char]10
$procs = Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | Sort-Object ProcessName
$grouped = $procs | Group-Object ProcessName
foreach ($g in $grouped) {
    $count = $g.Count
    $titles = ($g.Group | ForEach-Object { $_.MainWindowTitle } | Where-Object { $_ }) -join ", "
    if ($titles.Length -gt 80) { $titles = $titles.Substring(0, 77) + "..." }
    $result += "  $($g.Name) ($count windows): $titles" + [char]10
}

$result += [char]10
$result += "Note: Windows does not expose a public API to enumerate virtual desktops." + [char]10
$result += "Use switch(direction: left/right) to navigate, or switch(direction: new) to create a new desktop." + [char]10

Write-Output $result
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to list desktops: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *SpacesTool) switchDesktop(ctx context.Context, direction string) (string, error) {
	if direction == "" {
		return "", fmt.Errorf("direction is required: 'left', 'right', or 'new'")
	}

	var keyAction string
	var desc string

	switch direction {
	case "left":
		// Win+Ctrl+Left
		keyAction = `
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_LEFT, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 50
[KbdOps]::keybd_event([KbdOps]::VK_LEFT, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
`
		desc = "Switched to desktop on the left"

	case "right":
		// Win+Ctrl+Right
		keyAction = `
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_RIGHT, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 50
[KbdOps]::keybd_event([KbdOps]::VK_RIGHT, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
`
		desc = "Switched to desktop on the right"

	case "new":
		// Win+Ctrl+D
		keyAction = `
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_D, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 50
[KbdOps]::keybd_event([KbdOps]::VK_D, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
`
		desc = "Created and switched to new virtual desktop"

	default:
		return "", fmt.Errorf("invalid direction: %s (use 'left', 'right', or 'new')", direction)
	}

	script := keybdEventPS() + keyAction + fmt.Sprintf(`
Write-Output "%s"
`, desc)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to switch desktop: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *SpacesTool) moveWindow(ctx context.Context, direction string) (string, error) {
	if direction == "" {
		return "", fmt.Errorf("direction is required: 'left' or 'right'")
	}

	var vkDir string
	switch direction {
	case "left":
		vkDir = "VK_LEFT"
	case "right":
		vkDir = "VK_RIGHT"
	default:
		return "", fmt.Errorf("invalid direction: %s (use 'left' or 'right')", direction)
	}

	// Win+Ctrl+Shift+Arrow (Windows 11 only; on Windows 10 this does nothing)
	script := keybdEventPS() + fmt.Sprintf(`
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_SHIFT, 0, 0, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::%s, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 50
[KbdOps]::keybd_event([KbdOps]::%s, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_SHIFT, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_CONTROL, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)
[KbdOps]::keybd_event([KbdOps]::VK_LWIN, 0, [KbdOps]::KEYEVENTF_KEYUP, [UIntPtr]::Zero)

Write-Output "Moved frontmost window to %s desktop (Windows 11 only; no effect on Windows 10)"
`, vkDir, vkDir, direction)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to move window: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}
