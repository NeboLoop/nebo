//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// KeychainTool provides macOS Keychain access for secure credential storage.
type KeychainTool struct{}

func NewKeychainTool() *KeychainTool { return &KeychainTool{} }

func (t *KeychainTool) Name() string { return "keychain" }

func (t *KeychainTool) Description() string {
	return "Access Keychain to securely retrieve/store passwords and credentials. Requires user authentication."
}

func (t *KeychainTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["get", "find", "add", "delete"],
				"description": "Action: get (password), find (search), add (store), delete"
			},
			"service": {"type": "string", "description": "Service name (e.g., 'github.com')"},
			"account": {"type": "string", "description": "Account/username"},
			"password": {"type": "string", "description": "Password to store (for add)"},
			"kind": {"type": "string", "enum": ["generic", "internet"], "description": "Password type"}
		},
		"required": ["action"]
	}`)
}

func (t *KeychainTool) RequiresApproval() bool { return true }

type keychainInput struct {
	Action   string `json:"action"`
	Service  string `json:"service"`
	Account  string `json:"account"`
	Password string `json:"password"`
	Kind     string `json:"kind"`
}

func (t *KeychainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p keychainInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "get":
		return t.getPassword(p)
	case "find":
		return t.findPasswords(p)
	case "add":
		return t.addPassword(p)
	case "delete":
		return t.deletePassword(p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *KeychainTool) getPassword(p keychainInput) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service name is required", IsError: true}, nil
	}
	args := []string{"find-generic-password", "-w"}
	if p.Kind == "internet" {
		args = []string{"find-internet-password", "-w"}
	}
	args = append(args, "-s", p.Service)
	if p.Account != "" {
		args = append(args, "-a", p.Account)
	}
	out, err := exec.Command("security", args...).CombinedOutput()
	if err != nil {
		errStr := string(out)
		if strings.Contains(errStr, "could not be found") {
			return &ToolResult{Content: fmt.Sprintf("No password found for '%s'", p.Service), IsError: true}, nil
		}
		if strings.Contains(errStr, "User interaction is not allowed") {
			return &ToolResult{Content: "Keychain access requires user authentication", IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\n%s", err, errStr), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Password for '%s': %s", p.Service, strings.TrimSpace(string(out)))}, nil
}

func (t *KeychainTool) findPasswords(p keychainInput) (*ToolResult, error) {
	query := p.Service
	if query == "" {
		query = p.Account
	}
	if query == "" {
		return &ToolResult{Content: "Service or account is required", IsError: true}, nil
	}
	out, err := exec.Command("security", "dump-keychain").CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	lines := strings.Split(string(out), "\n")
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
		return &ToolResult{Content: fmt.Sprintf("No items found matching '%s'", query)}, nil
	}
	if len(matches) > 20 {
		matches = matches[:20]
	}
	return &ToolResult{Content: fmt.Sprintf("Items matching '%s':\n%s", query, strings.Join(matches, "\n"))}, nil
}

func (t *KeychainTool) addPassword(p keychainInput) (*ToolResult, error) {
	if p.Service == "" || p.Account == "" || p.Password == "" {
		return &ToolResult{Content: "Service, account, and password are required", IsError: true}, nil
	}
	args := []string{"add-generic-password"}
	if p.Kind == "internet" {
		args = []string{"add-internet-password"}
	}
	args = append(args, "-s", p.Service, "-a", p.Account, "-w", p.Password, "-U")
	out, err := exec.Command("security", args...).CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\n%s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Password stored for '%s' / '%s'", p.Service, p.Account)}, nil
}

func (t *KeychainTool) deletePassword(p keychainInput) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service name is required", IsError: true}, nil
	}
	args := []string{"delete-generic-password"}
	if p.Kind == "internet" {
		args = []string{"delete-internet-password"}
	}
	args = append(args, "-s", p.Service)
	if p.Account != "" {
		args = append(args, "-a", p.Account)
	}
	out, err := exec.Command("security", args...).CombinedOutput()
	if err != nil {
		errStr := string(out)
		if strings.Contains(errStr, "could not be found") {
			return &ToolResult{Content: fmt.Sprintf("No password found for '%s'", p.Service), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\n%s", err, errStr), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Password deleted for '%s'", p.Service)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewKeychainTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "security",
	})
}
