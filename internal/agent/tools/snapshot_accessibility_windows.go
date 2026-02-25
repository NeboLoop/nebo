//go:build windows

package tools

import (
	"context"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// getUITreeWithBounds retrieves the accessibility tree for an app with element positions.
func getUITreeWithBounds(ctx context.Context, app string, windowBounds Rect) []RawElement {
	if app == "" {
		return nil
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

function Get-Elements {
    param($element, $depth = 0, $maxDepth = 5)
    if ($depth -gt $maxDepth) { return }

    $name = $element.Current.Name
    $controlType = $element.Current.ControlType.ProgrammaticName -replace "ControlType.", ""
    $rect = $element.Current.BoundingRectangle

    $isActionable = $false
    try {
        $patterns = $element.GetSupportedPatterns()
        if ($patterns.Length -gt 0) { $isActionable = $true }
    } catch {}

    $w = [int]$rect.Width
    $h = [int]$rect.Height

    if ($w -gt 0 -and $h -gt 0) {
        $x = [int]$rect.X
        $y = [int]$rect.Y
        $value = ""
        try {
            $vp = $element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
            if ($vp) { $value = $vp.Current.Value }
        } catch {}

        $actionStr = "0"
        if ($isActionable) { $actionStr = "1" }

        Write-Output "$controlType|$name||$value|$x|$y|$w|$h|$actionStr"
    }

    $condition = [System.Windows.Automation.Condition]::TrueCondition
    $children = $element.FindAll([System.Windows.Automation.TreeScope]::Children, $condition)
    foreach ($child in $children) {
        Get-Elements -element $child -depth ($depth + 1) -maxDepth $maxDepth
    }
}

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

if ($window) {
    Get-Elements -element $window -maxDepth 5
}
`, escapeUIAutoPS(app))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		fmt.Printf("[screenshot:see] UIAutomation error: %v\n", err)
		return nil
	}

	return parseElementOutput(strings.TrimSpace(string(out)))
}

func parseElementOutput(output string) []RawElement {
	if output == "" {
		return nil
	}

	var elements []RawElement
	for _, line := range strings.Split(output, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		parts := strings.SplitN(line, "|", 9)
		if len(parts) < 9 {
			continue
		}

		x, _ := strconv.Atoi(parts[4])
		y, _ := strconv.Atoi(parts[5])
		w, _ := strconv.Atoi(parts[6])
		h, _ := strconv.Atoi(parts[7])
		actionable := parts[8] == "1"

		elements = append(elements, RawElement{
			Role:        normalizeWinRole(parts[0]),
			Title:       parts[1],
			Description: parts[2],
			Value:       parts[3],
			Position:    Rect{X: x, Y: y, Width: w, Height: h},
			Actionable:  actionable,
		})
	}
	return elements
}

func normalizeWinRole(role string) string {
	roleMap := map[string]string{
		"Button":   "button",
		"Edit":     "textfield",
		"Text":     "static text",
		"CheckBox": "checkbox",
		"RadioButton": "radio",
		"ComboBox": "combobox",
		"MenuItem": "menu item",
		"Menu":     "menu",
		"Slider":   "slider",
		"Tab":      "tab",
		"TabItem":  "tab",
		"Hyperlink": "link",
		"Image":    "image",
		"ToolBar":  "toolbar",
		"List":     "list",
		"ListItem": "list",
		"Table":    "table",
		"ScrollBar": "scrollbar",
		"Group":    "group",
		"Window":   "window",
	}
	if mapped, ok := roleMap[role]; ok {
		return mapped
	}
	return strings.ToLower(role)
}
