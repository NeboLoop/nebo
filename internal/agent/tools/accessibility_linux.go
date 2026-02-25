//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// AccessibilityTool provides Linux UI automation via AT-SPI (Assistive Technology Service Provider Interface).
type AccessibilityTool struct {
	backend string // "python-atspi", "xdotool", or ""
}

func NewAccessibilityTool() *AccessibilityTool {
	t := &AccessibilityTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *AccessibilityTool) detectBackend() string {
	// Check for python3 with gi module (PyGObject for AT-SPI)
	cmd := exec.Command("python3", "-c", "import gi; gi.require_version('Atspi', '2.0')")
	if cmd.Run() == nil {
		return "python-atspi"
	}
	// xdotool for basic window operations
	if _, err := exec.LookPath("xdotool"); err == nil {
		return "xdotool"
	}
	return ""
}

func (t *AccessibilityTool) Name() string { return "accessibility" }

func (t *AccessibilityTool) Description() string {
	base := `Inspect and interact with application UI elements via Linux accessibility APIs.

Actions:
- tree: Get the UI element hierarchy for an app (buttons, text fields, menus, etc.)
- find: Search for elements by role and/or label
- click: Click a specific element by role+label match
- get_value/set_value: Read or change element values (text fields, checkboxes)
- list_apps: List all running apps with accessibility access

For visual element targeting, use screenshot(action: "see") instead.`

	switch t.backend {
	case "python-atspi":
		return base + "\nBackend: AT-SPI (full support)."
	case "xdotool":
		return base + "\nBackend: xdotool (basic window operations only; install python3-gi for full support)."
	default:
		return base + "\nNo backend available. Install python3-gi and gir1.2-atspi-2.0, or xdotool."
	}
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

type accessibilityInputLinux struct {
	Action   string `json:"action"`
	App      string `json:"app"`
	Role     string `json:"role"`
	Label    string `json:"label"`
	Value    string `json:"value"`
	MaxDepth int    `json:"max_depth"`
}

func (t *AccessibilityTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No accessibility backend available. Please install one of:\n" +
				"  - python3-gi: sudo apt install python3-gi gir1.2-atspi-2.0\n" +
				"  - xdotool: sudo apt install xdotool (basic window operations only)",
			IsError: true,
		}, nil
	}

	var p accessibilityInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if p.MaxDepth <= 0 {
		p.MaxDepth = 3
	}

	switch t.backend {
	case "python-atspi":
		return t.executeAtspi(ctx, p)
	case "xdotool":
		return t.executeXdotool(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// AT-SPI implementation (via Python)
// ============================================================================

func (t *AccessibilityTool) executeAtspi(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "list_apps":
		return t.atspiListApps(ctx)
	case "tree":
		return t.atspiTree(ctx, p)
	case "find":
		return t.atspiFind(ctx, p)
	case "click":
		return t.atspiClick(ctx, p)
	case "get_value":
		return t.atspiGetValue(ctx, p)
	case "set_value":
		return t.atspiSetValue(ctx, p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *AccessibilityTool) atspiListApps(ctx context.Context) (*ToolResult, error) {
	script := `
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

desktop = Atspi.get_desktop(0)
apps = []
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app:
        name = app.get_name()
        if name:
            apps.append(name)

for app in sorted(set(apps)):
    print(app)
`
	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list apps: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No accessible applications found. Make sure AT-SPI is enabled."}, nil
	}

	lines := strings.Split(output, "\n")
	return &ToolResult{Content: fmt.Sprintf("Accessible applications (%d):\n%s", len(lines), output)}, nil
}

func (t *AccessibilityTool) atspiTree(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" {
		return &ToolResult{Content: "App name is required for tree action", IsError: true}, nil
	}

	script := fmt.Sprintf(`
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

def get_tree(obj, depth=0, max_depth=%d):
    if depth > max_depth or not obj:
        return ""

    role = obj.get_role_name()
    name = obj.get_name() or ""
    state_set = obj.get_state_set()

    indent = "  " * depth
    result = f"{indent}[{role}] {name}\n"

    for i in range(obj.get_child_count()):
        child = obj.get_child_at_index(i)
        if child:
            result += get_tree(child, depth + 1, max_depth)

    return result

desktop = Atspi.get_desktop(0)
found = False
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app and "%s".lower() in (app.get_name() or "").lower():
        print(f"Application: {app.get_name()}")
        for j in range(app.get_child_count()):
            window = app.get_child_at_index(j)
            if window:
                print(get_tree(window, 0, %d))
        found = True
        break

if not found:
    print("Application not found: %s")
`, p.MaxDepth, escapeAtspyPy(p.App), p.MaxDepth, escapeAtspyPy(p.App))

	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get tree: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if strings.HasPrefix(output, "Application not found") {
		return &ToolResult{Content: output}, nil
	}

	return &ToolResult{Content: output}, nil
}

func (t *AccessibilityTool) atspiFind(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" {
		return &ToolResult{Content: "App name is required", IsError: true}, nil
	}

	roleFilter := ""
	if p.Role != "" {
		roleFilter = fmt.Sprintf(`and obj.get_role_name().lower() == "%s"`, escapeAtspyPy(strings.ToLower(p.Role)))
	}

	labelFilter := ""
	if p.Label != "" {
		labelFilter = fmt.Sprintf(`and "%s".lower() in (obj.get_name() or "").lower()`, escapeAtspyPy(p.Label))
	}

	script := fmt.Sprintf(`
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

def find_elements(obj, results, depth=0, max_depth=10):
    if depth > max_depth or not obj:
        return

    if True %s %s:
        role = obj.get_role_name()
        name = obj.get_name() or ""
        results.append(f"[{role}] {name}")

    for i in range(obj.get_child_count()):
        child = obj.get_child_at_index(i)
        if child:
            find_elements(child, results, depth + 1, max_depth)

desktop = Atspi.get_desktop(0)
results = []
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app and "%s".lower() in (app.get_name() or "").lower():
        for j in range(app.get_child_count()):
            window = app.get_child_at_index(j)
            if window:
                find_elements(window, results)
        break

if results:
    for r in results[:20]:
        print(r)
    if len(results) > 20:
        print(f"... and {len(results) - 20} more")
else:
    print("No matching elements found")
`, roleFilter, labelFilter, escapeAtspyPy(p.App))

	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to find elements: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) atspiClick(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" || p.Label == "" {
		return &ToolResult{Content: "App and label are required for click action", IsError: true}, nil
	}

	roleFilter := ""
	if p.Role != "" {
		roleFilter = fmt.Sprintf(`and obj.get_role_name().lower() == "%s"`, escapeAtspyPy(strings.ToLower(p.Role)))
	}

	script := fmt.Sprintf(`
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

def find_and_click(obj, label, depth=0, max_depth=10):
    if depth > max_depth or not obj:
        return False

    name = obj.get_name() or ""
    if "%s".lower() in name.lower() %s:
        # Try to perform default action
        action = obj.get_action_iface()
        if action:
            for i in range(action.get_n_actions()):
                action_name = action.get_action_name(i)
                if action_name in ["click", "activate", "press"]:
                    action.do_action(i)
                    print(f"Clicked: [{obj.get_role_name()}] {name}")
                    return True
            # Try first action if no click found
            if action.get_n_actions() > 0:
                action.do_action(0)
                print(f"Activated: [{obj.get_role_name()}] {name}")
                return True

    for i in range(obj.get_child_count()):
        child = obj.get_child_at_index(i)
        if child and find_and_click(child, label, depth + 1, max_depth):
            return True

    return False

desktop = Atspi.get_desktop(0)
clicked = False
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app and "%s".lower() in (app.get_name() or "").lower():
        for j in range(app.get_child_count()):
            window = app.get_child_at_index(j)
            if window and find_and_click(window, "%s"):
                clicked = True
                break
        break

if not clicked:
    print("Element not found or not clickable: %s")
`, escapeAtspyPy(p.Label), roleFilter, escapeAtspyPy(p.App), escapeAtspyPy(p.Label), escapeAtspyPy(p.Label))

	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to click: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) atspiGetValue(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" || p.Label == "" {
		return &ToolResult{Content: "App and label are required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

def find_value(obj, label, depth=0, max_depth=10):
    if depth > max_depth or not obj:
        return None

    name = obj.get_name() or ""
    if "%s".lower() in name.lower():
        # Try text interface
        text = obj.get_text_iface()
        if text:
            return text.get_text(0, text.get_character_count())
        # Try value interface
        value = obj.get_value_iface()
        if value:
            return str(value.get_current_value())
        return name

    for i in range(obj.get_child_count()):
        child = obj.get_child_at_index(i)
        if child:
            result = find_value(child, label, depth + 1, max_depth)
            if result is not None:
                return result

    return None

desktop = Atspi.get_desktop(0)
value = None
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app and "%s".lower() in (app.get_name() or "").lower():
        for j in range(app.get_child_count()):
            window = app.get_child_at_index(j)
            if window:
                value = find_value(window, "%s")
                if value is not None:
                    break
        break

if value is not None:
    print(value)
else:
    print("Element not found: %s")
`, escapeAtspyPy(p.Label), escapeAtspyPy(p.App), escapeAtspyPy(p.Label), escapeAtspyPy(p.Label))

	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get value: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *AccessibilityTool) atspiSetValue(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" || p.Label == "" {
		return &ToolResult{Content: "App and label are required", IsError: true}, nil
	}
	if p.Value == "" {
		return &ToolResult{Content: "Value is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi

def find_and_set(obj, label, value, depth=0, max_depth=10):
    if depth > max_depth or not obj:
        return False

    name = obj.get_name() or ""
    if "%s".lower() in name.lower():
        # Try editable text interface
        text = obj.get_editable_text_iface()
        if text:
            # Clear and set new text
            length = obj.get_text_iface().get_character_count() if obj.get_text_iface() else 0
            text.delete_text(0, length)
            text.insert_text(0, value, len(value))
            print(f"Set value on: [{obj.get_role_name()}] {name}")
            return True
        # Try value interface
        val_iface = obj.get_value_iface()
        if val_iface:
            try:
                val_iface.set_current_value(float(value))
                print(f"Set value on: [{obj.get_role_name()}] {name}")
                return True
            except:
                pass

    for i in range(obj.get_child_count()):
        child = obj.get_child_at_index(i)
        if child and find_and_set(child, label, value, depth + 1, max_depth):
            return True

    return False

desktop = Atspi.get_desktop(0)
success = False
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app and "%s".lower() in (app.get_name() or "").lower():
        for j in range(app.get_child_count()):
            window = app.get_child_at_index(j)
            if window and find_and_set(window, "%s", "%s"):
                success = True
                break
        break

if not success:
    print("Element not found or not editable: %s")
`, escapeAtspyPy(p.Label), escapeAtspyPy(p.App), escapeAtspyPy(p.Label), escapeAtspyPy(p.Value), escapeAtspyPy(p.Label))

	cmd := exec.CommandContext(ctx, "python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set value: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

// ============================================================================
// xdotool fallback implementation
// ============================================================================

func (t *AccessibilityTool) executeXdotool(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "list_apps":
		return t.xdotoolListApps(ctx)
	case "tree":
		return &ToolResult{Content: "UI tree inspection requires AT-SPI. Install python3-gi and gir1.2-atspi-2.0"}, nil
	case "find":
		return t.xdotoolFind(ctx, p)
	case "click":
		return t.xdotoolClick(ctx, p)
	case "get_value", "set_value":
		return &ToolResult{Content: "Getting/setting values requires AT-SPI. Install python3-gi and gir1.2-atspi-2.0"}, nil
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *AccessibilityTool) xdotoolListApps(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "xdotool", "search", "--name", ".")
	out, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: "No windows found"}, nil
	}

	windowIDs := strings.Split(strings.TrimSpace(string(out)), "\n")
	var apps []string
	seen := make(map[string]bool)

	for _, id := range windowIDs {
		if id == "" {
			continue
		}
		nameCmd := exec.CommandContext(ctx, "xdotool", "getwindowname", id)
		nameOut, _ := nameCmd.Output()
		name := strings.TrimSpace(string(nameOut))
		if name != "" && !seen[name] {
			apps = append(apps, name)
			seen[name] = true
		}
	}

	if len(apps) == 0 {
		return &ToolResult{Content: "No windows found"}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Windows (%d):\n%s", len(apps), strings.Join(apps, "\n"))}, nil
}

func (t *AccessibilityTool) xdotoolFind(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" && p.Label == "" {
		return &ToolResult{Content: "App or label is required", IsError: true}, nil
	}

	searchTerm := p.App
	if p.Label != "" {
		searchTerm = p.Label
	}

	cmd := exec.CommandContext(ctx, "xdotool", "search", "--name", searchTerm)
	out, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("No windows found matching '%s'", searchTerm)}, nil
	}

	windowIDs := strings.Split(strings.TrimSpace(string(out)), "\n")
	var results []string

	for _, id := range windowIDs {
		if id == "" {
			continue
		}
		nameCmd := exec.CommandContext(ctx, "xdotool", "getwindowname", id)
		nameOut, _ := nameCmd.Output()
		name := strings.TrimSpace(string(nameOut))
		if name != "" {
			results = append(results, fmt.Sprintf("[window %s] %s", id, name))
		}
	}

	if len(results) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No windows found matching '%s'", searchTerm)}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Found %d windows:\n%s", len(results), strings.Join(results, "\n"))}, nil
}

func (t *AccessibilityTool) xdotoolClick(ctx context.Context, p accessibilityInputLinux) (*ToolResult, error) {
	if p.App == "" && p.Label == "" {
		return &ToolResult{Content: "App or label is required", IsError: true}, nil
	}

	searchTerm := p.App
	if p.Label != "" {
		searchTerm = p.Label
	}

	// Find window
	cmd := exec.CommandContext(ctx, "xdotool", "search", "--name", searchTerm)
	out, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("No windows found matching '%s'", searchTerm)}, nil
	}

	windowIDs := strings.Split(strings.TrimSpace(string(out)), "\n")
	if len(windowIDs) == 0 || windowIDs[0] == "" {
		return &ToolResult{Content: fmt.Sprintf("No windows found matching '%s'", searchTerm)}, nil
	}

	// Activate and focus the window
	windowID := windowIDs[0]
	activateCmd := exec.CommandContext(ctx, "xdotool", "windowactivate", "--sync", windowID)
	if err := activateCmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to activate window: %v", err), IsError: true}, nil
	}

	// Get window name for confirmation
	nameCmd := exec.CommandContext(ctx, "xdotool", "getwindowname", windowID)
	nameOut, _ := nameCmd.Output()
	name := strings.TrimSpace(string(nameOut))

	return &ToolResult{Content: fmt.Sprintf("Activated window: %s (ID: %s)", name, windowID)}, nil
}

func escapeAtspyPy(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `"`, `\"`)
	s = strings.ReplaceAll(s, "\n", `\n`)
	return s
}

