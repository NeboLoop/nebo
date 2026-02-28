//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// DialogTool detects and interacts with dialogs and modal windows on Windows.
// Uses PowerShell + System.Windows.Automation to enumerate dialog UI elements.
type DialogTool struct{}

// NewDialogTool creates a new dialog tool
func NewDialogTool() *DialogTool {
	return &DialogTool{}
}

func (t *DialogTool) Name() string {
	return "dialog"
}

func (t *DialogTool) Description() string {
	return "Detect and interact with Windows dialogs and modal windows. List dialog buttons and fields, click buttons, fill text fields, and dismiss dialogs."
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
				"description": "Application name (e.g., 'Notepad', 'Explorer')"
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

type dialogInputWin struct {
	Action string `json:"action"`
	App    string `json:"app"`
	Button string `json:"button"`
	Field  int    `json:"field"`
	Value  string `json:"value"`
}

func (t *DialogTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in dialogInputWin
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "detect":
		result, err = t.detectDialogs(ctx, in.App)
	case "list":
		result, err = t.listDialogElements(ctx, in.App)
	case "click":
		result, err = t.clickButton(ctx, in.App, in.Button)
	case "fill":
		result, err = t.fillField(ctx, in.App, in.Field, in.Value)
	case "dismiss":
		result, err = t.dismissDialog(ctx, in.App)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

// findDialogsPS returns a PowerShell snippet that finds dialog/modal windows for an app.
// It looks for child windows with IsModal=true or Window control types that are owned by the app.
func findDialogsPS(app string) string {
	escaped := escapeUIAutoPS(app)
	return fmt.Sprintf(`
# Find the main app window
$root = [System.Windows.Automation.AutomationElement]::RootElement
$allCondition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)

$appWindow = $null
foreach ($w in $windows) {
    if ($w.Current.Name -like "*%s*") {
        $appWindow = $w
        break
    }
}

# Find dialog windows: check all top-level windows that belong to the same process
$dialogs = @()
if ($appWindow) {
    $appPid = $appWindow.Current.ProcessId

    foreach ($w in $windows) {
        if ($w.Current.ProcessId -eq $appPid -and $w.Current.NativeWindowHandle -ne $appWindow.Current.NativeWindowHandle) {
            # This is another window from the same process — likely a dialog
            $dialogs += $w
        }
    }

    # Also check for modal child windows within the main window
    $windowCondition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Window
    )
    $childWindows = $appWindow.FindAll([System.Windows.Automation.TreeScope]::Children, $windowCondition)
    foreach ($cw in $childWindows) {
        $dialogs += $cw
    }

    # Check for Pane control type (common for dialogs in some apps)
    $paneCondition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Pane
    )
    $childPanes = $appWindow.FindAll([System.Windows.Automation.TreeScope]::Children, $paneCondition)
    foreach ($cp in $childPanes) {
        try {
            $wp = $cp.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern)
            if ($wp -and $wp.Current.IsModal) {
                $dialogs += $cp
            }
        } catch {}
    }
}
`, escaped)
}

func (t *DialogTool) detectDialogs(ctx context.Context, app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $appWindow) {
    Write-Output "Window not found: %s"
    exit
}

if ($dialogs.Count -eq 0) {
    Write-Output "No dialogs detected in $($appWindow.Current.Name)"
    exit
}

$result = ""
foreach ($d in $dialogs) {
    $title = $d.Current.Name
    if (-not $title) { $title = "(untitled)" }
    $isModal = $false
    try {
        $wp = $d.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern)
        if ($wp) { $isModal = $wp.Current.IsModal }
    } catch {}
    $modalStr = ""
    if ($isModal) { $modalStr = " [modal]" }
    $result += "Dialog: $title$modalStr" + [char]10
}
Write-Output $result
`, findDialogsPS(app), escapeUIAutoPS(app))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to detect dialogs: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) listDialogElements(ctx context.Context, app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $appWindow) {
    Write-Output "Window not found: %s"
    exit
}

if ($dialogs.Count -eq 0) {
    Write-Output "No dialogs found in $($appWindow.Current.Name)"
    exit
}

$result = ""
foreach ($d in $dialogs) {
    $title = $d.Current.Name
    if (-not $title) { $title = "(untitled)" }
    $result += "=== Dialog: $title ===" + [char]10

    # List buttons
    $buttonCondition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Button
    )
    $buttons = $d.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    $bi = 1
    foreach ($btn in $buttons) {
        $bname = $btn.Current.Name
        if ($bname) {
            $result += "Button ${bi}: $bname" + [char]10
            $bi++
        }
    }

    # List text fields (Edit controls)
    $editCondition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Edit
    )
    $edits = $d.FindAll([System.Windows.Automation.TreeScope]::Descendants, $editCondition)
    $fi = 1
    foreach ($ed in $edits) {
        $val = ""
        try {
            $vp = $ed.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
            if ($vp) { $val = $vp.Current.Value }
        } catch {}
        $fname = $ed.Current.Name
        if ($fname) { $fname = " ($fname)" }
        $result += "Field ${fi}${fname}: $val" + [char]10
        $fi++
    }

    # List static text (labels)
    $textCondition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Text
    )
    $texts = $d.FindAll([System.Windows.Automation.TreeScope]::Descendants, $textCondition)
    foreach ($txt in $texts) {
        $tval = $txt.Current.Name
        if ($tval) {
            $result += "Text: $tval" + [char]10
        }
    }

    $result += [char]10
}

Write-Output $result
`, findDialogsPS(app), escapeUIAutoPS(app))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to list dialog elements: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) clickButton(ctx context.Context, app, button string) (string, error) {
	if app == "" || button == "" {
		return "", fmt.Errorf("app and button name are required")
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $appWindow) {
    Write-Output "Window not found: %s"
    exit
}

$buttonCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button
)

# First try dialogs
$clicked = $false
foreach ($d in $dialogs) {
    $buttons = $d.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    foreach ($btn in $buttons) {
        if ($btn.Current.Name -eq "%s") {
            try {
                $invokePattern = $btn.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                $invokePattern.Invoke()
                Write-Output "Clicked button '%s' in dialog"
                $clicked = $true
                break
            } catch {}
        }
    }
    if ($clicked) { break }
}

# Try partial match in dialogs
if (-not $clicked) {
    foreach ($d in $dialogs) {
        $buttons = $d.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
        foreach ($btn in $buttons) {
            if ($btn.Current.Name -like "*%s*") {
                try {
                    $invokePattern = $btn.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                    $invokePattern.Invoke()
                    Write-Output "Clicked button '$($btn.Current.Name)' in dialog"
                    $clicked = $true
                    break
                } catch {}
            }
        }
        if ($clicked) { break }
    }
}

# Try main window as fallback
if (-not $clicked) {
    $buttons = $appWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    foreach ($btn in $buttons) {
        if ($btn.Current.Name -eq "%s" -or $btn.Current.Name -like "*%s*") {
            try {
                $invokePattern = $btn.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
                $invokePattern.Invoke()
                Write-Output "Clicked button '$($btn.Current.Name)' in window"
                $clicked = $true
                break
            } catch {}
        }
    }
}

if (-not $clicked) {
    Write-Output "Button '%s' not found in %s"
}
`, findDialogsPS(app), escapeUIAutoPS(app),
		escapeUIAutoPS(button), escapeUIAutoPS(button),
		escapeUIAutoPS(button),
		escapeUIAutoPS(button), escapeUIAutoPS(button),
		escapeUIAutoPS(button), escapeUIAutoPS(app))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to click button: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) fillField(ctx context.Context, app string, field int, value string) (string, error) {
	if app == "" || value == "" {
		return "", fmt.Errorf("app and value are required")
	}
	if field < 1 {
		field = 1
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if (-not $appWindow) {
    Write-Output "Window not found: %s"
    exit
}

$editCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Edit
)

$filled = $false

# Try dialogs first
foreach ($d in $dialogs) {
    $edits = $d.FindAll([System.Windows.Automation.TreeScope]::Descendants, $editCondition)
    if ($edits.Count -ge %d) {
        $target = $edits[%d]
        try {
            $vp = $target.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
            $vp.SetValue("%s")
            Write-Output "Filled field %d in dialog"
            $filled = $true
            break
        } catch {
            Write-Output "Field found but not editable: $_"
            $filled = $true
            break
        }
    }
}

# Try main window as fallback
if (-not $filled) {
    $edits = $appWindow.FindAll([System.Windows.Automation.TreeScope]::Descendants, $editCondition)
    if ($edits.Count -ge %d) {
        $target = $edits[%d]
        try {
            $vp = $target.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
            $vp.SetValue("%s")
            Write-Output "Filled field %d in window"
        } catch {
            Write-Output "Field found but not editable: $_"
        }
    } else {
        Write-Output "Field %d not found (only $($edits.Count) fields)"
    }
}
`, findDialogsPS(app), escapeUIAutoPS(app),
		field, field-1, escapeUIAutoPS(value), field,
		field, field-1, escapeUIAutoPS(value), field,
		field)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to fill field: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *DialogTool) dismissDialog(ctx context.Context, app string) (string, error) {
	if app == "" {
		return "", fmt.Errorf("app name is required")
	}

	// Try common dismiss buttons first
	for _, btn := range []string{"Cancel", "Close", "OK", "No", "Done"} {
		result, err := t.clickButton(ctx, app, btn)
		if err == nil && !strings.Contains(result, "not found") {
			return fmt.Sprintf("Dismissed dialog with %q button", btn), nil
		}
	}

	// Fall back to sending Escape key
	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

%s

if ($appWindow) {
    try {
        $appWindow.SetFocus()
    } catch {}
}

Add-Type -AssemblyName System.Windows.Forms
Start-Sleep -Milliseconds 100
[System.Windows.Forms.SendKeys]::SendWait("{ESC}")
Write-Output "Sent Escape key to dismiss dialog"
`, findDialogsPS(app))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("failed to dismiss dialog: %v\nOutput: %s", err, string(out))
	}

	return strings.TrimSpace(string(out)), nil
}
