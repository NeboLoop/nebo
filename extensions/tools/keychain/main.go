// Keychain Plugin - macOS Keychain access (secure password lookup)
// Build: go build -o ~/.gobot/plugins/tools/keychain
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

type KeychainTool struct{}

type keychainInput struct {
	Action   string `json:"action"`   // get, find, add, delete
	Service  string `json:"service"`  // Service name (like "github.com")
	Account  string `json:"account"`  // Account/username
	Password string `json:"password"` // Password (for add)
	Label    string `json:"label"`    // Label for the keychain item
	Kind     string `json:"kind"`     // generic or internet
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *KeychainTool) Name() string {
	return "keychain"
}

func (t *KeychainTool) Description() string {
	return "Access macOS Keychain to securely retrieve passwords and credentials. Requires user authentication."
}

func (t *KeychainTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["get", "find", "add", "delete"],
				"description": "Action: get (retrieve password), find (search), add (store new), delete"
			},
			"service": {
				"type": "string",
				"description": "Service name (e.g., 'github.com', 'AWS')"
			},
			"account": {
				"type": "string",
				"description": "Account name/username"
			},
			"password": {
				"type": "string",
				"description": "Password to store (for add action)"
			},
			"label": {
				"type": "string",
				"description": "Human-readable label for the item"
			},
			"kind": {
				"type": "string",
				"enum": ["generic", "internet"],
				"description": "Password type: generic (apps) or internet (websites)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *KeychainTool) RequiresApproval() bool {
	return true // Always require approval for keychain access
}

func (t *KeychainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params keychainInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "get":
		return t.getPassword(params)
	case "find":
		return t.findPasswords(params)
	case "add":
		return t.addPassword(params)
	case "delete":
		return t.deletePassword(params)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *KeychainTool) getPassword(params keychainInput) (*ToolResult, error) {
	if params.Service == "" {
		return &ToolResult{Content: "Service name is required", IsError: true}, nil
	}

	args := []string{"find-generic-password", "-w"}
	if params.Kind == "internet" {
		args = []string{"find-internet-password", "-w"}
	}

	args = append(args, "-s", params.Service)
	if params.Account != "" {
		args = append(args, "-a", params.Account)
	}

	cmd := exec.Command("security", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		errStr := string(output)
		if strings.Contains(errStr, "could not be found") {
			return &ToolResult{Content: fmt.Sprintf("No password found for service '%s'", params.Service), IsError: true}, nil
		}
		if strings.Contains(errStr, "User interaction is not allowed") {
			return &ToolResult{Content: "Keychain access requires user authentication. Please unlock keychain.", IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed to get password: %v\n%s", err, errStr), IsError: true}, nil
	}

	password := strings.TrimSpace(string(output))
	return &ToolResult{Content: fmt.Sprintf("Password for '%s': %s", params.Service, password), IsError: false}, nil
}

func (t *KeychainTool) findPasswords(params keychainInput) (*ToolResult, error) {
	query := params.Service
	if query == "" {
		query = params.Account
	}
	if query == "" {
		return &ToolResult{Content: "Service or account is required for search", IsError: true}, nil
	}

	// Use security dump-keychain and grep for matching items
	// Note: This only shows metadata, not actual passwords
	args := []string{"dump-keychain"}
	cmd := exec.Command("security", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to search keychain: %v", err), IsError: true}, nil
	}

	// Parse output to find matching entries
	lines := strings.Split(string(output), "\n")
	var matches []string
	var currentEntry []string
	queryLower := strings.ToLower(query)

	for _, line := range lines {
		if strings.HasPrefix(line, "keychain:") || strings.HasPrefix(line, "class:") {
			if len(currentEntry) > 0 {
				entryStr := strings.Join(currentEntry, " | ")
				if strings.Contains(strings.ToLower(entryStr), queryLower) {
					matches = append(matches, entryStr)
				}
			}
			currentEntry = []string{}
		}

		if strings.Contains(line, "\"svce\"") || strings.Contains(line, "\"acct\"") || strings.Contains(line, "\"srvr\"") {
			// Extract the value
			parts := strings.SplitN(line, "=", 2)
			if len(parts) == 2 {
				value := strings.TrimSpace(parts[1])
				value = strings.Trim(value, "\"<>")
				if value != "" && value != "NULL" {
					currentEntry = append(currentEntry, value)
				}
			}
		}
	}

	if len(matches) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No keychain items found matching '%s'", query), IsError: false}, nil
	}

	// Limit results
	if len(matches) > 20 {
		matches = matches[:20]
	}

	return &ToolResult{Content: fmt.Sprintf("Keychain items matching '%s':\n%s", query, strings.Join(matches, "\n")), IsError: false}, nil
}

func (t *KeychainTool) addPassword(params keychainInput) (*ToolResult, error) {
	if params.Service == "" {
		return &ToolResult{Content: "Service name is required", IsError: true}, nil
	}
	if params.Account == "" {
		return &ToolResult{Content: "Account name is required", IsError: true}, nil
	}
	if params.Password == "" {
		return &ToolResult{Content: "Password is required", IsError: true}, nil
	}

	args := []string{"add-generic-password"}
	if params.Kind == "internet" {
		args = []string{"add-internet-password"}
	}

	args = append(args, "-s", params.Service, "-a", params.Account, "-w", params.Password)
	if params.Label != "" {
		args = append(args, "-l", params.Label)
	}
	args = append(args, "-U") // Update if exists

	cmd := exec.Command("security", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to add password: %v\n%s", err, string(output)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Password stored for service '%s', account '%s'", params.Service, params.Account), IsError: false}, nil
}

func (t *KeychainTool) deletePassword(params keychainInput) (*ToolResult, error) {
	if params.Service == "" {
		return &ToolResult{Content: "Service name is required", IsError: true}, nil
	}

	args := []string{"delete-generic-password"}
	if params.Kind == "internet" {
		args = []string{"delete-internet-password"}
	}

	args = append(args, "-s", params.Service)
	if params.Account != "" {
		args = append(args, "-a", params.Account)
	}

	cmd := exec.Command("security", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		errStr := string(output)
		if strings.Contains(errStr, "could not be found") {
			return &ToolResult{Content: fmt.Sprintf("No password found for service '%s'", params.Service), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed to delete password: %v\n%s", err, errStr), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Password deleted for service '%s'", params.Service), IsError: false}, nil
}

// RPC wrapper
type KeychainToolRPC struct {
	tool *KeychainTool
}

func (t *KeychainToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *KeychainToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *KeychainToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *KeychainToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *KeychainToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type KeychainPlugin struct {
	tool *KeychainTool
}

func (p *KeychainPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &KeychainToolRPC{tool: p.tool}, nil
}

func (p *KeychainPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &KeychainPlugin{tool: &KeychainTool{}},
		},
	})
}
