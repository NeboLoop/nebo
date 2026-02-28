//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// MenubarTool interacts with Windows menu bar items via UI Automation API.
// Uses PowerShell + System.Windows.Automation to list and click menu bar items and system tray icons.
type MenubarTool struct{}

// NewMenubarTool creates a new menubar tool
func NewMenubarTool() *MenubarTool {
	return &MenubarTool{}
}

func (t *MenubarTool) Name() string {
	return "menubar"
}

func (t *MenubarTool) Description() string {
	return "Interact with the Windows menu bar: list menu bar items, list menu items for an app, click menu items, and access system tray (notification area) icons."
}

func (t *MenubarTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: list (menu bar items), menus (list items in a menu), click (click menu item), status (list system tray icons), click_status (click tray icon by index)",
				"enum": ["list", "menus", "click", "status", "click_status"]
			},
			"app": {
				"type": "string",
				"description": "Application name (for list/menus/click actions)"
			},
			"menu": {
				"type": "string",
				"description": "Menu name (e.g., 'File', 'Edit')"
			},
			"item": {
				"type": "string",
				"description": "Menu item name to click (e.g., 'Save', 'Copy')"
			},
			"index": {
				"type": "integer",
				"description": "System tray icon index (1-based) for click_status"
			}
		},
		"required": ["action"]
	}`)
}

func (t *MenubarTool) RequiresApproval() bool {
	return true
}

type menubarInputWin struct {
	Action   string `json:"action"`
	App      string `json:"app"`
	Menu     string `json:"menu"`
	Item     string `json:"item"`
	MenuPath string `json:"menu_path"` // Domain schema field: "File > Save"
	Name     string `json:"name"`      // Domain schema field: alternative to item
	Index    int    `json:"index"`
}

// resolveMenuFields fills Menu/Item from MenuPath/Name when the direct fields are empty.
// The domain schema exposes menu_path ("File > Save") and name, while the sub-tool
// methods use separate menu and item parameters.
func (in *menubarInputWin) resolveMenuFields() {
	if in.Menu == "" && in.MenuPath != "" {
		if parts := strings.SplitN(in.MenuPath, " > ", 2); len(parts) == 2 {
			in.Menu = parts[0]
			if in.Item == "" {
				in.Item = parts[1]
			}
		} else {
			in.Menu = in.MenuPath
		}
	}
	if in.Item == "" && in.Name != "" {
		in.Item = in.Name
	}
}

func (t *MenubarTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in menubarInputWin
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}
	in.resolveMenuFields()

	var result string
	var err error

	switch in.Action {
	case "list":
		result, err = t.listMenuBar(ctx, in.App)
	case "menus":
		result, err = t.listMenus(ctx, in.App, in.Menu)
	case "click":
		result, err = t.clickMenuItem(ctx, in.App, in.Menu, in.Item)
	case "status":
		result, err = t.listStatusItems(ctx)
	case "click_status":
		result, err = t.clickStatusItem(ctx, in.Index)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

// findWindowPS returns a PowerShell snippet that finds a window by app name using UI Automation.
// It tries: exact title match → partial title match → process name match.
// The result is stored in $window.
func findWindowPS(app string) string {
	if app == "" {
		// Find the foreground window
		return `
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class FgWin {
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
}
"@
$fgHwnd = [FgWin]::GetForegroundWindow()
$window = [System.Windows.Automation.AutomationElement]::FromHandle($fgHwnd)
`
	}
	escaped := escapeUIAutoPS(app)
	return fmt.Sprintf(`
$root = [System.Windows.Automation.AutomationElement]::RootElement
$condition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty,
    "%s",
    [System.Windows.Automation.PropertyConditionFlags]::IgnoreCase
)
$window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition)

$allCondition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)

if (-not $window) {
    # Try partial title match
    foreach ($w in $windows) {
        if ($w.Current.Name -like "*%s*") {
            $window = $w
            break
        }
    }
}

if (-not $window) {
    # Try matching by process name (e.g., "Zed" when title is "project - file.go")
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class WinProc {
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
}
"@
    foreach ($w in $windows) {
        try {
            $hwnd = New-Object IntPtr($w.Current.NativeWindowHandle)
            $pid = [uint32]0
            [void][WinProc]::GetWindowThreadProcessId($hwnd, [ref]$pid)
            if ($pid -gt 0) {
                $proc = Get-Process -Id $pid -ErrorAction SilentlyContinue
                if ($proc -and $proc.ProcessName -like "*%s*") {
                    $window = $w
                    break
                }
            }
        } catch {}
    }
}
`, escaped, escaped, escaped)
}

func (t *MenubarTool) listMenuBar(ctx context.Context, app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

$menuBarCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuBar
)
$menuBar = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $menuBarCondition)

if (-not $menuBar) {
    Write-Output "No menu bar found in $($window.Current.Name)"
    exit
}

$menuItemCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem
)
$menuItems = $menuBar.FindAll([System.Windows.Automation.TreeScope]::Children, $menuItemCondition)

$result = "Menu bar for $($window.Current.Name):" + [char]10
foreach ($item in $menuItems) {
    $name = $item.Current.Name
    if ($name) {
        $result += $name + [char]10
    }
}
Write-Output $result
`, findWindowPS(app), escapeUIAutoPS(app))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to list menu bar: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

// collectMenuBarNamesPS returns a PS snippet that collects top-level menu bar item names
// into the $topMenuNames hashset for filtering.
func collectMenuBarNamesPS() string {
	return `
$topMenuNames = @{}
$topItems = $menuBar.FindAll([System.Windows.Automation.TreeScope]::Children, $menuItemCondition)
foreach ($ti in $topItems) {
    if ($ti.Current.Name) {
        $topMenuNames[$ti.Current.Name] = $true
    }
}
`
}

func (t *MenubarTool) listMenus(ctx context.Context, app, menu string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}
	if menu == "" {
		return "", fmt.Errorf("menu name is required (e.g., 'File', 'Edit')")
	}

	// After expanding a menu, WinUI 3 apps (e.g., Windows 11 Notepad) place flyout items
	// as descendants of the window, not the MenuBarItem. Classic Win32 apps place them as
	// descendants of the MenuBarItem. We try the MenuBarItem first, then fall back to the
	// window search (filtering out top-level MenuBarItems).
	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

$menuBarCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuBar
)
$menuBar = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $menuBarCondition)

if (-not $menuBar) {
    Write-Output "No menu bar found in $($window.Current.Name)"
    exit
}

$menuItemCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem
)
$topItems = $menuBar.FindAll([System.Windows.Automation.TreeScope]::Children, $menuItemCondition)

$targetMenu = $null
foreach ($item in $topItems) {
    if ($item.Current.Name -eq "%s") {
        $targetMenu = $item
        break
    }
}

# Fallback: case-insensitive partial match
if (-not $targetMenu) {
    foreach ($item in $topItems) {
        if ($item.Current.Name -like "*%s*") {
            $targetMenu = $item
            break
        }
    }
}

if (-not $targetMenu) {
    Write-Output "Menu '%s' not found"
    exit
}

# Expand the menu using ExpandCollapsePattern
try {
    $expandPattern = $targetMenu.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
    $expandPattern.Expand()
    Start-Sleep -Milliseconds 500
} catch {
    Write-Output "Cannot expand menu '%s'"
    exit
}

# Try 1: Classic Win32 - items are descendants of the MenuBarItem
$subItems = $targetMenu.FindAll([System.Windows.Automation.TreeScope]::Descendants, $menuItemCondition)

if ($subItems.Count -eq 0) {
    # Try 2: WinUI 3 - flyout items appear as descendants of the window, not the MenuBarItem.
    # Collect top-level menu bar names so we can filter them out.
    %s
    $allMenuItems = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $menuItemCondition)
    $subItems = [System.Collections.ArrayList]::new()
    foreach ($mi in $allMenuItems) {
        if ($mi.Current.Name -and -not $topMenuNames.ContainsKey($mi.Current.Name)) {
            [void]$subItems.Add($mi)
        }
    }
}

$result = "$($window.Current.Name) > %s:" + [char]10
foreach ($sub in $subItems) {
    $name = $sub.Current.Name
    if ($name) {
        $enabled = $sub.Current.IsEnabled
        $suffix = ""
        if (-not $enabled) { $suffix = " (disabled)" }

        # Try to get keyboard shortcut from AccessKey
        $accel = $sub.Current.AcceleratorKey
        if ($accel) { $suffix += " [$accel]" }

        $result += $name + $suffix + [char]10
    } else {
        $result += "---" + [char]10
    }
}

# Collapse the menu back
try {
    $expandPattern.Collapse()
} catch {}

Write-Output $result
`, findWindowPS(app), escapeUIAutoPS(app),
		escapeUIAutoPS(menu), escapeUIAutoPS(menu), escapeUIAutoPS(menu), escapeUIAutoPS(menu),
		collectMenuBarNamesPS(), escapeUIAutoPS(menu))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to list menu items: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) clickMenuItem(ctx context.Context, app, menu, item string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}
	if menu == "" || item == "" {
		return "", fmt.Errorf("menu and item names are required")
	}

	// Same WinUI 3 fallback as listMenus: search window descendants if MenuBarItem has no children.
	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

$menuBarCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuBar
)
$menuBar = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $menuBarCondition)

if (-not $menuBar) {
    Write-Output "No menu bar found"
    exit
}

# Find and expand the top-level menu
$menuItemCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::MenuItem
)
$topItems = $menuBar.FindAll([System.Windows.Automation.TreeScope]::Children, $menuItemCondition)

$targetMenu = $null
foreach ($mi in $topItems) {
    if ($mi.Current.Name -eq "%s") {
        $targetMenu = $mi
        break
    }
}

if (-not $targetMenu) {
    Write-Output "Menu '%s' not found"
    exit
}

# Expand the menu
try {
    $expandPattern = $targetMenu.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
    $expandPattern.Expand()
    Start-Sleep -Milliseconds 300
} catch {
    Write-Output "Cannot expand menu '%s'"
    exit
}

# Try 1: Classic Win32 - items are children of the MenuBarItem
$subItems = $targetMenu.FindAll([System.Windows.Automation.TreeScope]::Children, $menuItemCondition)

if ($subItems.Count -eq 0) {
    # Try 2: WinUI 3 - flyout items are descendants of the window
    %s
    $allMenuItems = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $menuItemCondition)
    $subItems = [System.Collections.ArrayList]::new()
    foreach ($mi in $allMenuItems) {
        if ($mi.Current.Name -and -not $topMenuNames.ContainsKey($mi.Current.Name)) {
            [void]$subItems.Add($mi)
        }
    }
}

# Find the target item
$targetItem = $null
foreach ($sub in $subItems) {
    if ($sub.Current.Name -eq "%s") {
        $targetItem = $sub
        break
    }
}

if (-not $targetItem) {
    # Try partial match
    foreach ($sub in $subItems) {
        if ($sub.Current.Name -like "*%s*") {
            $targetItem = $sub
            break
        }
    }
}

if (-not $targetItem) {
    try { $expandPattern.Collapse() } catch {}
    Write-Output "Item '%s' not found in menu '%s'"
    exit
}

# Click the item: try InvokePattern, then TogglePattern (for checkboxes like Word Wrap),
# then ExpandCollapsePattern (for sub-menus).
try {
    $invokePattern = $targetItem.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $invokePattern.Invoke()
    Write-Output "Clicked $($window.Current.Name) > %s > $($targetItem.Current.Name)"
} catch {
    try {
        $togglePattern = $targetItem.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
        $togglePattern.Toggle()
        $state = $togglePattern.Current.ToggleState
        Write-Output "Toggled $($window.Current.Name) > %s > $($targetItem.Current.Name) (state: $state)"
    } catch {
        try {
            $subExpand = $targetItem.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern)
            $subExpand.Expand()
            Write-Output "Expanded sub-menu: $($targetItem.Current.Name)"
        } catch {
            Write-Output "Cannot click item '%s': $_"
        }
    }
}
`, findWindowPS(app), escapeUIAutoPS(app),
		escapeUIAutoPS(menu), escapeUIAutoPS(menu), escapeUIAutoPS(menu),
		collectMenuBarNamesPS(),
		escapeUIAutoPS(item), escapeUIAutoPS(item),
		escapeUIAutoPS(item), escapeUIAutoPS(menu),
		escapeUIAutoPS(menu), escapeUIAutoPS(menu),
		escapeUIAutoPS(item))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to click menu item: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) listStatusItems(ctx context.Context) (string, error) {
	script := `
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement

# Find the system tray: Shell_TrayWnd > TrayNotifyWnd > SysPager > ToolbarWindow32
$trayCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    "Shell_TrayWnd"
)
$tray = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $trayCondition)

if (-not $tray) {
    Write-Output "System tray not found"
    exit
}

# Find toolbar buttons in the notification area
$buttonCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button
)

# Search in the notification area (SysPager and overflow)
$pagerCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    "SysPager"
)
$pager = $tray.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $pagerCondition)

$result = "System Tray Icons:" + [char]10
$idx = 1

if ($pager) {
    $buttons = $pager.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    foreach ($btn in $buttons) {
        $name = $btn.Current.Name
        if ($name) {
            $result += "$idx. $name" + [char]10
            $idx++
        }
    }
}

# Also check overflow area
$overflowCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    "NotifyIconOverflowWindow"
)
$overflow = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $overflowCondition)
if ($overflow) {
    $overflowButtons = $overflow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    foreach ($btn in $overflowButtons) {
        $name = $btn.Current.Name
        if ($name) {
            $result += "$idx. $name (overflow)" + [char]10
            $idx++
        }
    }
}

if ($idx -eq 1) {
    Write-Output "No system tray icons found"
} else {
    Write-Output $result
}
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to list status items: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) clickStatusItem(ctx context.Context, index int) (string, error) {
	if index < 1 {
		return "", fmt.Errorf("index is required (1-based, use status action to see available items)")
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement

$trayCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    "Shell_TrayWnd"
)
$tray = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $trayCondition)

if (-not $tray) {
    Write-Output "System tray not found"
    exit
}

$buttonCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button
)

$pagerCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    "SysPager"
)
$pager = $tray.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $pagerCondition)

$allButtons = @()
if ($pager) {
    $buttons = $pager.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    foreach ($btn in $buttons) {
        if ($btn.Current.Name) {
            $allButtons += $btn
        }
    }
}

# Also check overflow
$overflowCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty,
    "NotifyIconOverflowWindow"
)
$overflow = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $overflowCondition)
if ($overflow) {
    $overflowButtons = $overflow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    foreach ($btn in $overflowButtons) {
        if ($btn.Current.Name) {
            $allButtons += $btn
        }
    }
}

$targetIdx = %d
if ($targetIdx -gt $allButtons.Count) {
    Write-Output "Index $targetIdx out of range (only $($allButtons.Count) tray icons)"
    exit
}

$target = $allButtons[$targetIdx - 1]
try {
    $invokePattern = $target.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    $invokePattern.Invoke()
    Write-Output "Clicked tray icon: $($target.Current.Name)"
} catch {
    # Fall back to legacy click
    try {
        $target.SetFocus()
        Start-Sleep -Milliseconds 100
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.SendKeys]::SendWait("{ENTER}")
        Write-Output "Activated tray icon: $($target.Current.Name)"
    } catch {
        Write-Output "Cannot click tray icon: $($target.Current.Name) - $_"
    }
}
`, index)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to click status item: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}
