//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// AccessibilityTool provides Windows UI automation via UI Automation API.
type AccessibilityTool struct{}

func NewAccessibilityTool() *AccessibilityTool {
	return &AccessibilityTool{}
}

func (t *AccessibilityTool) Name() string { return "accessibility" }

func (t *AccessibilityTool) Description() string {
	return `Inspect and interact with application UI elements via Windows UI Automation APIs.

Actions:
- tree: Get the UI element hierarchy for an app (buttons, text fields, menus, etc.)
- find: Search for elements by role and/or label
- click: Click a specific element by role+label match
- get_value/set_value: Read or change element values (text fields, checkboxes)
- list_apps: List all windows with UI Automation access

For visual element targeting, use screenshot(action: "see") instead.`
}

func (t *AccessibilityTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["tree", "find", "click", "get_value", "set_value", "list_apps"],
				"description": "Action to perform"
			},
			"app": {"type": "string", "description": "Application name or window title"},
			"role": {"type": "string", "description": "Element role: Button, Edit, CheckBox, MenuItem, etc."},
			"label": {"type": "string", "description": "Element name/label to match"},
			"value": {"type": "string", "description": "Value to set"},
			"max_depth": {"type": "integer", "description": "Max depth for tree (default: 3)"}
		},
		"required": ["action"]
	}`)
}

func (t *AccessibilityTool) RequiresApproval() bool { return true }

type accessibilityInputWin struct {
	Action   string `json:"action"`
	App      string `json:"app"`
	Role     string `json:"role"`
	Label    string `json:"label"`
	Value    string `json:"value"`
	MaxDepth int    `json:"max_depth"`
}

func (t *AccessibilityTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p accessibilityInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if p.MaxDepth <= 0 {
		p.MaxDepth = 3
	}

	switch p.Action {
	case "list_apps":
		return t.listApps(ctx)
	case "tree":
		return t.getTree(ctx, p)
	case "find":
		return t.findElements(ctx, p)
	case "click":
		return t.clickElement(ctx, p)
	case "get_value":
		return t.getValue(ctx, p)
	case "set_value":
		return t.setValue(ctx, p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *AccessibilityTool) listApps(ctx context.Context) (*ToolResult, error) {
	script := `
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$condition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)

$apps = @()
foreach ($window in $windows) {
    $name = $window.Current.Name
    if ($name -and $name.Length -gt 0) {
        $apps += $name
    }
}

$apps | Sort-Object -Unique | ForEach-Object { Write-Output $_ }
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list apps: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No windows found"}, nil
	}

	lines := strings.Split(output, "\n")
	return &ToolResult{Content: fmt.Sprintf("Windows (%d):\n%s", len(lines), output)}, nil
}

func (t *AccessibilityTool) getTree(ctx context.Context, p accessibilityInputWin) (*ToolResult, error) {
	if p.App == "" {
		return &ToolResult{Content: "App name is required for tree action", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

function Get-UITree {
    param($element, $depth = 0, $maxDepth = %d)

    if ($depth -gt $maxDepth) { return "" }

    $indent = "  " * $depth
    $name = $element.Current.Name
    $controlType = $element.Current.ControlType.ProgrammaticName -replace "ControlType.", ""

    $result = "$indent[$controlType] $name" + [char]10

    $condition = [System.Windows.Automation.Condition]::TrueCondition
    $children = $element.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)

    foreach ($child in $children) {
        $result += Get-UITree -element $child -depth ($depth + 1) -maxDepth $maxDepth
    }

    return $result
}

$root = [System.Windows.Automation.AutomationElement]::RootElement
$condition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty,
    "%s",
    [System.Windows.Automation.PropertyConditionFlags]::IgnoreCase
)
$window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition)

if (-not $window) {
    # Try partial match
    $allCondition = [System.Windows.Automation.Condition]::TrueCondition
    $windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)
    foreach ($w in $windows) {
        if ($w.Current.Name -like "*%s*") {
            $window = $w
            break
        }
    }
}

if ($window) {
    Write-Output "Window: $($window.Current.Name)"
    Write-Output (Get-UITree -element $window -maxDepth %d)
} else {
    Write-Output "Window not found: %s"
}
`, p.MaxDepth, escapeUIAutoPS(p.App), escapeUIAutoPS(p.App), p.MaxDepth, escapeUIAutoPS(p.App))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get tree: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) findElements(ctx context.Context, p accessibilityInputWin) (*ToolResult, error) {
	if p.App == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}

	roleFilter := ""
	if p.Role != "" {
		roleFilter = fmt.Sprintf(`
    $typeCondition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::%s
    )`, escapeUIAutoPS(p.Role))
	}

	labelFilter := `[System.Windows.Automation.Condition]::TrueCondition`
	if p.Label != "" {
		labelFilter = fmt.Sprintf(`New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty,
        "%s",
        [System.Windows.Automation.PropertyConditionFlags]::IgnoreCase
    )`, escapeUIAutoPS(p.Label))
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$allCondition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)

$window = $null
foreach ($w in $windows) {
    if ($w.Current.Name -like "*%s*") {
        $window = $w
        break
    }
}

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

%s
$searchCondition = %s

$elements = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $searchCondition)

$count = 0
foreach ($el in $elements) {
    $name = $el.Current.Name
    $type = $el.Current.ControlType.ProgrammaticName -replace "ControlType.", ""
    if ($name) {
        Write-Output "[$type] $name"
        $count++
        if ($count -ge 20) {
            $remaining = $elements.Count - 20
            if ($remaining -gt 0) {
                Write-Output "... and $remaining more"
            }
            break
        }
    }
}

if ($count -eq 0) {
    Write-Output "No matching elements found"
}
`, escapeUIAutoPS(p.App), escapeUIAutoPS(p.App), roleFilter, labelFilter)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to find elements: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) clickElement(ctx context.Context, p accessibilityInputWin) (*ToolResult, error) {
	if p.App == "" || p.Label == "" {
		return &ToolResult{Content: "App and label are required for click action", IsError: true}, nil
	}

	roleFilter := "[System.Windows.Automation.Condition]::TrueCondition"
	if p.Role != "" {
		roleFilter = fmt.Sprintf(`New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::%s
    )`, escapeUIAutoPS(p.Role))
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$allCondition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)

$window = $null
foreach ($w in $windows) {
    if ($w.Current.Name -like "*%s*") {
        $window = $w
        break
    }
}

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

$nameCondition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty,
    "%s",
    [System.Windows.Automation.PropertyConditionFlags]::IgnoreCase
)

$typeCondition = %s

$andCondition = New-Object System.Windows.Automation.AndCondition($nameCondition, $typeCondition)
$element = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $andCondition)

if (-not $element) {
    # Try partial match
    $elements = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $typeCondition)
    foreach ($el in $elements) {
        if ($el.Current.Name -like "*%s*") {
            $element = $el
            break
        }
    }
}

if ($element) {
    $invokePattern = $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    if ($invokePattern) {
        $invokePattern.Invoke()
        Write-Output "Clicked: [$($element.Current.ControlType.ProgrammaticName -replace 'ControlType.', '')] $($element.Current.Name)"
    } else {
        # Try toggle pattern for checkboxes
        try {
            $togglePattern = $element.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
            if ($togglePattern) {
                $togglePattern.Toggle()
                Write-Output "Toggled: [$($element.Current.ControlType.ProgrammaticName -replace 'ControlType.', '')] $($element.Current.Name)"
            }
        } catch {
            Write-Output "Element found but not clickable: $($element.Current.Name)"
        }
    }
} else {
    Write-Output "Element not found: %s"
}
`, escapeUIAutoPS(p.App), escapeUIAutoPS(p.App), escapeUIAutoPS(p.Label), roleFilter, escapeUIAutoPS(p.Label), escapeUIAutoPS(p.Label))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to click: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) getValue(ctx context.Context, p accessibilityInputWin) (*ToolResult, error) {
	if p.App == "" || p.Label == "" {
		return &ToolResult{Content: "App and label are required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$allCondition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)

$window = $null
foreach ($w in $windows) {
    if ($w.Current.Name -like "*%s*") {
        $window = $w
        break
    }
}

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

$elements = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

$element = $null
foreach ($el in $elements) {
    if ($el.Current.Name -like "*%s*") {
        $element = $el
        break
    }
}

if ($element) {
    # Try ValuePattern
    try {
        $valuePattern = $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        if ($valuePattern) {
            Write-Output $valuePattern.Current.Value
            exit
        }
    } catch {}

    # Try TextPattern
    try {
        $textPattern = $element.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern)
        if ($textPattern) {
            $range = $textPattern.DocumentRange
            Write-Output $range.GetText(-1)
            exit
        }
    } catch {}

    # Return the name as fallback
    Write-Output $element.Current.Name
} else {
    Write-Output "Element not found: %s"
}
`, escapeUIAutoPS(p.App), escapeUIAutoPS(p.App), escapeUIAutoPS(p.Label), escapeUIAutoPS(p.Label))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get value: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) setValue(ctx context.Context, p accessibilityInputWin) (*ToolResult, error) {
	if p.App == "" || p.Label == "" {
		return &ToolResult{Content: "App and label are required", IsError: true}, nil
	}
	if p.Value == "" {
		return &ToolResult{Content: "Value is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$allCondition = [System.Windows.Automation.Condition]::TrueCondition
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $allCondition)

$window = $null
foreach ($w in $windows) {
    if ($w.Current.Name -like "*%s*") {
        $window = $w
        break
    }
}

if (-not $window) {
    Write-Output "Window not found: %s"
    exit
}

$elements = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

$element = $null
foreach ($el in $elements) {
    if ($el.Current.Name -like "*%s*") {
        $element = $el
        break
    }
}

if ($element) {
    try {
        $valuePattern = $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        if ($valuePattern) {
            $valuePattern.SetValue("%s")
            Write-Output "Set value on: [$($element.Current.ControlType.ProgrammaticName -replace 'ControlType.', '')] $($element.Current.Name)"
            exit
        }
    } catch {}

    Write-Output "Element found but not editable: $($element.Current.Name)"
} else {
    Write-Output "Element not found: %s"
}
`, escapeUIAutoPS(p.App), escapeUIAutoPS(p.App), escapeUIAutoPS(p.Label), escapeUIAutoPS(p.Value), escapeUIAutoPS(p.Label))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set value: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func escapeUIAutoPS(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewAccessibilityTool(),
		Platforms: []string{PlatformWindows},
		Category:  "automation",
	})
}
