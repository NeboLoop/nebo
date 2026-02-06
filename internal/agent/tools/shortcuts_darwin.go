//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// ShortcutsTool runs Apple Shortcuts automations.
type ShortcutsTool struct{}

func NewShortcutsTool() *ShortcutsTool { return &ShortcutsTool{} }

func (t *ShortcutsTool) Name() string { return "shortcuts" }

func (t *ShortcutsTool) Description() string {
	return "Run Apple Shortcuts automations. List available shortcuts and execute them with optional input."
}

func (t *ShortcutsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["run", "list"],
				"description": "Action: run (execute shortcut), list (show all)"
			},
			"name": {"type": "string", "description": "Name of the shortcut to run"},
			"input": {"type": "string", "description": "Input text to pass to the shortcut"}
		},
		"required": ["action"]
	}`)
}

func (t *ShortcutsTool) RequiresApproval() bool { return true }

type shortcutsInput struct {
	Action string `json:"action"`
	Name   string `json:"name"`
	Input  string `json:"input"`
}

func (t *ShortcutsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p shortcutsInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "list":
		return t.listShortcuts()
	case "run":
		return t.runShortcut(p.Name, p.Input)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *ShortcutsTool) listShortcuts() (*ToolResult, error) {
	out, err := exec.Command("shortcuts", "list").CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\n%s", err, string(out)), IsError: true}, nil
	}
	shortcuts := strings.TrimSpace(string(out))
	if shortcuts == "" {
		return &ToolResult{Content: "No shortcuts found"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available shortcuts:\n%s", shortcuts)}, nil
}

func (t *ShortcutsTool) runShortcut(name, input string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Shortcut name is required", IsError: true}, nil
	}
	args := []string{"run", name}
	if input != "" {
		args = append(args, "-i", input)
	}
	out, err := exec.Command("shortcuts", args...).CombinedOutput()
	if err != nil {
		errStr := string(out)
		if strings.Contains(errStr, "Couldn't find") {
			return &ToolResult{Content: fmt.Sprintf("Shortcut '%s' not found", name), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\n%s", err, errStr), IsError: true}, nil
	}
	result := strings.TrimSpace(string(out))
	if result == "" {
		return &ToolResult{Content: fmt.Sprintf("Shortcut '%s' executed successfully", name)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Shortcut '%s' output:\n%s", name, result)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewShortcutsTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "automation",
	})
}
