// Shortcuts Plugin - macOS Shortcuts app integration
// Build: go build -o ~/.gobot/plugins/tools/shortcuts
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/rpc"
	"os/exec"
	"strings"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type ShortcutsTool struct{}

type shortcutsInput struct {
	Action string `json:"action"` // run, list
	Name   string `json:"name"`   // Shortcut name
	Input  string `json:"input"`  // Input to pass to shortcut
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *ShortcutsTool) Name() string {
	return "shortcuts"
}

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
				"description": "Action: run (execute shortcut), list (show all shortcuts)"
			},
			"name": {
				"type": "string",
				"description": "Name of the shortcut to run"
			},
			"input": {
				"type": "string",
				"description": "Input text to pass to the shortcut"
			}
		},
		"required": ["action"]
	}`)
}

func (t *ShortcutsTool) RequiresApproval() bool {
	return true // Running shortcuts can have significant effects
}

func (t *ShortcutsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params shortcutsInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "list":
		return t.listShortcuts()
	case "run":
		return t.runShortcut(params.Name, params.Input)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *ShortcutsTool) listShortcuts() (*ToolResult, error) {
	cmd := exec.Command("shortcuts", "list")
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list shortcuts: %v\n%s", err, string(output)), IsError: true}, nil
	}

	shortcuts := strings.TrimSpace(string(output))
	if shortcuts == "" {
		return &ToolResult{Content: "No shortcuts found", IsError: false}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Available shortcuts:\n%s", shortcuts), IsError: false}, nil
}

func (t *ShortcutsTool) runShortcut(name string, input string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Shortcut name is required", IsError: true}, nil
	}

	args := []string{"run", name}
	if input != "" {
		args = append(args, "-i", input)
	}

	cmd := exec.Command("shortcuts", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		errStr := string(output)
		if strings.Contains(errStr, "Couldn't find") {
			return &ToolResult{Content: fmt.Sprintf("Shortcut '%s' not found", name), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed to run shortcut: %v\n%s", err, errStr), IsError: true}, nil
	}

	result := strings.TrimSpace(string(output))
	if result == "" {
		return &ToolResult{Content: fmt.Sprintf("Shortcut '%s' executed successfully", name), IsError: false}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Shortcut '%s' output:\n%s", name, result), IsError: false}, nil
}

// RPC wrapper
type ShortcutsToolRPC struct {
	tool *ShortcutsTool
}

func (t *ShortcutsToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *ShortcutsToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *ShortcutsToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *ShortcutsToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *ShortcutsToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type ShortcutsPlugin struct {
	tool *ShortcutsTool
}

func (p *ShortcutsPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &ShortcutsToolRPC{tool: p.tool}, nil
}

func (p *ShortcutsPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &ShortcutsPlugin{tool: &ShortcutsTool{}},
		},
	})
}
