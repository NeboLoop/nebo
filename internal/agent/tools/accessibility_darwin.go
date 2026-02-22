//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// AccessibilityTool provides access to macOS UI elements via Accessibility APIs.
type AccessibilityTool struct{}

func NewAccessibilityTool() *AccessibilityTool { return &AccessibilityTool{} }

func (t *AccessibilityTool) Name() string { return "accessibility" }

func (t *AccessibilityTool) Description() string {
	return `Inspect and interact with application UI elements via macOS Accessibility APIs.

Actions:
- tree: Get the UI element hierarchy for an app (buttons, text fields, menus, etc.)
- find: Search for elements by role and/or label
- click: Click a specific element by role+label match
- get_value/set_value: Read or change element values (text fields, checkboxes)
- list_apps: List all running apps with accessibility access

Requires Accessibility permissions in System Settings > Privacy & Security.
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
			"app": {"type": "string", "description": "Application name"},
			"role": {"type": "string", "description": "Element role: button, textfield, checkbox, menu, etc."},
			"label": {"type": "string", "description": "Element label/title to match"},
			"value": {"type": "string", "description": "Value to set"},
			"max_depth": {"type": "integer", "description": "Max depth for tree (default: 3)"}
		},
		"required": ["action"]
	}`)
}

func (t *AccessibilityTool) RequiresApproval() bool { return true }

type accessibilityInput struct {
	Action   string `json:"action"`
	App      string `json:"app"`
	Role     string `json:"role"`
	Label    string `json:"label"`
	Value    string `json:"value"`
	MaxDepth int    `json:"max_depth"`
}

func (t *AccessibilityTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p accessibilityInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "tree":
		if p.MaxDepth <= 0 {
			p.MaxDepth = 3
		}
		return t.getUITree(p.App, p.MaxDepth)
	case "find":
		return t.findElement(p.App, p.Role, p.Label)
	case "click":
		return t.clickElement(p.App, p.Role, p.Label)
	case "get_value":
		return t.getValue(p.App, p.Role, p.Label)
	case "set_value":
		return t.setValue(p.App, p.Role, p.Label, p.Value)
	case "list_apps":
		return t.listApps()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *AccessibilityTool) listApps() (*ToolResult, error) {
	script := `tell application "System Events"
		set appList to ""
		repeat with proc in (every process whose visible is true)
			set appList to appList & name of proc & "
"
		end repeat
	end tell
	return appList`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	apps := strings.Split(strings.TrimSpace(out), "\n")
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Visible Applications (%d):\n\n", len(apps)))
	for _, app := range apps {
		if app != "" {
			sb.WriteString(fmt.Sprintf("• %s\n", app))
		}
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *AccessibilityTool) getUITree(app string, maxDepth int) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`on getUITree(elem, depth, maxD, indent)
		if depth > maxD then return ""
		set result to ""
		try
			set elemRole to role of elem
			set elemTitle to ""
			try
				set elemTitle to title of elem
			end try
			set line to indent & "- " & elemRole
			if elemTitle is not "" then set line to line & " \"" & elemTitle & "\""
			set result to result & line & "
"
			try
				set children to UI elements of elem
				repeat with child in children
					set result to result & getUITree(child, depth + 1, maxD, indent & "  ")
				end repeat
			end try
		end try
		return result
	end getUITree
	tell application "System Events"
		tell process "%s"
			set uiTree to ""
			repeat with win in windows
				set uiTree to uiTree & "Window: " & (name of win) & "
"
				set uiTree to uiTree & my getUITree(win, 1, %d, "  ")
			end repeat
			return uiTree
		end tell
	end tell`, app, maxDepth)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed for %s: %v", app, err), IsError: true}, nil
	}
	if out == "" {
		return &ToolResult{Content: fmt.Sprintf("No UI elements in %s", app)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("UI Tree for %s:\n\n%s", app, out)}, nil
}

func (t *AccessibilityTool) findElement(app, role, label string) (*ToolResult, error) {
	if app == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}
	if role == "" && label == "" {
		return &ToolResult{Content: "Role or label is required", IsError: true}, nil
	}
	axRole := t.mapRole(role)
	if axRole == "" {
		axRole = "UI element"
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			set foundElems to {}
			repeat with win in windows
				try
					set elems to every %s of win whose title contains "%s" or description contains "%s"
					repeat with elem in elems
						set end of foundElems to {role:role of elem, title:(title of elem)}
					end repeat
				end try
			end repeat
			return foundElems
		end tell
	end tell`, app, axRole, label, label)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: "No matching elements found"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Found elements in %s:\n%s", app, out)}, nil
}

func (t *AccessibilityTool) clickElement(app, role, label string) (*ToolResult, error) {
	if app == "" || label == "" {
		return &ToolResult{Content: "App and label are required", IsError: true}, nil
	}
	axRole := t.mapRole(role)
	if axRole == "" {
		axRole = "UI element"
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			set frontmost to true
			repeat with win in windows
				try
					click (first %s of win whose title contains "%s" or description contains "%s")
					return "clicked"
				end try
				try
					click (first %s of first group of win whose title contains "%s" or description contains "%s")
					return "clicked"
				end try
			end repeat
		end tell
	end tell
	return "not found"`, app, axRole, label, label, axRole, label, label)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "not found" {
		return &ToolResult{Content: fmt.Sprintf("Element '%s' not found", label), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Clicked '%s' in %s", label, app)}, nil
}

func (t *AccessibilityTool) getValue(app, role, label string) (*ToolResult, error) {
	if app == "" || label == "" {
		return &ToolResult{Content: "App and label are required", IsError: true}, nil
	}
	axRole := t.mapRole(role)
	if axRole == "" {
		axRole = "UI element"
	}
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			repeat with win in windows
				try
					set elem to (first %s of win whose title contains "%s" or description contains "%s")
					return value of elem
				end try
			end repeat
		end tell
	end tell
	return "not found"`, app, axRole, label, label)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "not found" {
		return &ToolResult{Content: fmt.Sprintf("Element '%s' not found", label), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Value of '%s': %s", label, out)}, nil
}

func (t *AccessibilityTool) setValue(app, role, label, value string) (*ToolResult, error) {
	if app == "" || label == "" || value == "" {
		return &ToolResult{Content: "App, label, and value are required", IsError: true}, nil
	}
	axRole := t.mapRole(role)
	if axRole == "" {
		axRole = "text field"
	}
	escapedValue := strings.ReplaceAll(value, `"`, `\"`)
	script := fmt.Sprintf(`tell application "System Events"
		tell process "%s"
			set frontmost to true
			repeat with win in windows
				try
					set elem to (first %s of win whose title contains "%s" or description contains "%s")
					set value of elem to "%s"
					return "set"
				end try
			end repeat
		end tell
	end tell
	return "not found"`, app, axRole, label, label, escapedValue)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "not found" {
		return &ToolResult{Content: fmt.Sprintf("Element '%s' not found", label), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Set '%s' to: %s", label, value)}, nil
}

func (t *AccessibilityTool) mapRole(role string) string {
	roleMap := map[string]string{
		"button": "button", "textfield": "text field", "text field": "text field",
		"checkbox": "checkbox", "menu": "menu", "menuitem": "menu item",
		"window": "window", "list": "list", "table": "table", "group": "group",
		"popup": "pop up button", "slider": "slider", "text": "static text",
	}
	if mapped, ok := roleMap[strings.ToLower(role)]; ok {
		return mapped
	}
	return role
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewAccessibilityTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "automation",
	})
}
