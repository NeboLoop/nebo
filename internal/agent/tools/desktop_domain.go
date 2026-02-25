package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// DesktopDomainTool consolidates desktop automation tools (input, ui, window, menu,
// dialog, space, shortcut) into a single STRAP domain tool with resource-based routing.
// Each resource delegates to the existing platform-specific tool implementation.
type DesktopDomainTool struct {
	subTools map[string]Tool // resource name → sub-tool
}

// NewDesktopDomainTool creates a desktop domain tool from the given sub-tools.
// Pass nil for resources not available on this platform.
func NewDesktopDomainTool(opts DesktopDomainOpts) *DesktopDomainTool {
	t := &DesktopDomainTool{
		subTools: make(map[string]Tool),
	}
	if opts.Input != nil {
		t.subTools["input"] = opts.Input
	}
	if opts.UI != nil {
		t.subTools["ui"] = opts.UI
	}
	if opts.Window != nil {
		t.subTools["window"] = opts.Window
	}
	if opts.Menu != nil {
		t.subTools["menu"] = opts.Menu
	}
	if opts.Dialog != nil {
		t.subTools["dialog"] = opts.Dialog
	}
	if opts.Space != nil {
		t.subTools["space"] = opts.Space
	}
	if opts.Shortcut != nil {
		t.subTools["shortcut"] = opts.Shortcut
	}
	return t
}

// DesktopDomainOpts configures which sub-tools are available for the desktop domain.
type DesktopDomainOpts struct {
	Input    Tool // mouse, keyboard, scroll (was: desktop)
	UI       Tool // accessibility tree inspection (was: accessibility)
	Window   Tool // window management (was: window)
	Menu     Tool // menu bar interaction (was: menubar) — darwin only
	Dialog   Tool // system dialog handling (was: dialog) — darwin only
	Space    Tool // virtual desktop/spaces (was: spaces) — darwin only
	Shortcut Tool // OS shortcuts/automations (was: shortcuts)
}

func (t *DesktopDomainTool) Name() string          { return "desktop" }
func (t *DesktopDomainTool) Domain() string         { return "desktop" }
func (t *DesktopDomainTool) RequiresApproval() bool { return true }

func (t *DesktopDomainTool) Resources() []string {
	resources := make([]string, 0, len(t.subTools))
	for r := range t.subTools {
		resources = append(resources, r)
	}
	return resources
}

func (t *DesktopDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "input":
		return []string{"click", "double_click", "right_click", "type", "hotkey", "scroll", "move", "drag", "paste"}
	case "ui":
		return []string{"tree", "find", "click", "get_value", "set_value", "list_apps"}
	case "window":
		return []string{"list", "focus", "move", "resize", "minimize", "maximize", "close"}
	case "menu":
		return []string{"list", "menus", "click", "status", "click_status"}
	case "dialog":
		return []string{"detect", "list", "click", "fill", "dismiss"}
	case "space":
		return []string{"list", "switch", "move_window"}
	case "shortcut":
		return []string{"list", "run"}
	default:
		return nil
	}
}

func (t *DesktopDomainTool) Description() string {
	return BuildDomainDescription(t.schemaConfig())
}

func (t *DesktopDomainTool) Schema() json.RawMessage {
	return BuildDomainSchema(t.schemaConfig())
}

func (t *DesktopDomainTool) schemaConfig() DomainSchemaConfig {
	resources := make(map[string]ResourceConfig)
	for name := range t.subTools {
		resources[name] = ResourceConfig{
			Name:    name,
			Actions: t.ActionsFor(name),
		}
	}

	return DomainSchemaConfig{
		Domain:      "desktop",
		Description: "Desktop automation: mouse/keyboard input, UI inspection, window management, menus, dialogs, spaces, shortcuts.",
		Resources:   resources,
		Fields: []FieldConfig{
			{Name: "x", Type: "integer", Description: "X coordinate"},
			{Name: "y", Type: "integer", Description: "Y coordinate"},
			{Name: "text", Type: "string", Description: "Text to type or paste"},
			{Name: "keys", Type: "string", Description: "Keyboard shortcut (e.g., 'cmd+c', 'return')"},
			{Name: "direction", Type: "string", Description: "Scroll direction: up, down, left, right"},
			{Name: "amount", Type: "integer", Description: "Scroll amount (default: 3)"},
			{Name: "to_x", Type: "integer", Description: "Destination X for drag"},
			{Name: "to_y", Type: "integer", Description: "Destination Y for drag"},
			{Name: "element", Type: "string", Description: "Element ID from screenshot see action (e.g., B3, T2)"},
			{Name: "snapshot_id", Type: "string", Description: "Snapshot to look up element in"},
			{Name: "app", Type: "string", Description: "Application name"},
			{Name: "role", Type: "string", Description: "UI element role: button, textfield, checkbox, etc."},
			{Name: "label", Type: "string", Description: "Element label/title to match"},
			{Name: "value", Type: "string", Description: "Value to set or fill"},
			{Name: "max_depth", Type: "integer", Description: "Max depth for UI tree (default: 3)"},
			{Name: "name", Type: "string", Description: "Window or shortcut name"},
			{Name: "title", Type: "string", Description: "Window title to match"},
			{Name: "width", Type: "integer", Description: "Width for window resize"},
			{Name: "height", Type: "integer", Description: "Height for window resize"},
			{Name: "menu_path", Type: "string", Description: "Menu path (e.g., 'File > New Window')"},
			{Name: "space", Type: "integer", Description: "Space/desktop number"},
			{Name: "button_label", Type: "string", Description: "Dialog button label to click"},
			{Name: "input", Type: "string", Description: "Input for shortcut run"},
		},
		Examples: []string{
			`desktop(resource: "input", action: "click", x: 100, y: 200)`,
			`desktop(resource: "input", action: "type", text: "hello world")`,
			`desktop(resource: "input", action: "hotkey", keys: "cmd+c")`,
			`desktop(resource: "ui", action: "tree", app: "Safari")`,
			`desktop(resource: "window", action: "list")`,
			`desktop(resource: "menu", action: "click", app: "Safari", menu_path: "File > New Window")`,
			`desktop(resource: "shortcut", action: "run", name: "My Shortcut")`,
		},
	}
}

// inferResource guesses the resource from the action name when resource is omitted.
func (t *DesktopDomainTool) inferResource(action string) string {
	switch action {
	case "click", "double_click", "right_click", "type", "hotkey", "scroll", "move", "drag", "paste":
		return "input"
	case "tree", "find", "get_value", "set_value", "list_apps":
		return "ui"
	case "focus", "minimize", "maximize":
		return "window"
	case "menus", "status", "click_status":
		return "menu"
	case "detect", "dismiss":
		return "dialog"
	case "switch", "move_window":
		return "space"
	case "run":
		return "shortcut"
	default:
		return "" // ambiguous — require explicit resource
	}
}

func (t *DesktopDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p DomainInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	resource := p.Resource
	if resource == "" {
		resource = t.inferResource(p.Action)
	}
	if resource == "" {
		return &ToolResult{
			Content: fmt.Sprintf("Resource is required. Available: %v", t.Resources()),
			IsError: true,
		}, nil
	}

	sub, ok := t.subTools[resource]
	if !ok {
		return &ToolResult{
			Content: fmt.Sprintf("Resource %q not available on this platform. Available: %v", resource, t.Resources()),
			IsError: true,
		}, nil
	}

	return sub.Execute(ctx, input)
}
