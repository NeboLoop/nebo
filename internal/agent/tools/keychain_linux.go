//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// KeychainTool provides Linux credential storage via secret-tool (GNOME Keyring) or pass.
type KeychainTool struct {
	backend string // "secret-tool", "pass", or ""
}

func NewKeychainTool() *KeychainTool {
	t := &KeychainTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *KeychainTool) detectBackend() string {
	// secret-tool is the standard GNOME Keyring CLI
	if _, err := exec.LookPath("secret-tool"); err == nil {
		return "secret-tool"
	}
	// pass is a popular password manager using GPG
	if _, err := exec.LookPath("pass"); err == nil {
		// Check if pass is initialized
		passDir := filepath.Join(os.Getenv("HOME"), ".password-store")
		if _, err := os.Stat(passDir); err == nil {
			return "pass"
		}
	}
	return ""
}

func (t *KeychainTool) Name() string { return "keychain" }

func (t *KeychainTool) Description() string {
	switch t.backend {
	case "secret-tool":
		return "Access Credentials (using GNOME Keyring) - securely store and retrieve passwords and API keys."
	case "pass":
		return "Access Credentials (using pass) - securely store and retrieve passwords using GPG encryption."
	default:
		return "Access Credentials - requires secret-tool (GNOME Keyring) or pass to be installed."
	}
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

type keychainInputLinux struct {
	Action   string `json:"action"`
	Service  string `json:"service"`
	Account  string `json:"account"`
	Password string `json:"password"`
	Kind     string `json:"kind"`
}

func (t *KeychainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No credential storage backend available. Please install one of:\n" +
				"  - secret-tool: sudo apt install libsecret-tools (Debian/Ubuntu)\n" +
				"  - pass: sudo apt install pass && pass init <gpg-id>",
			IsError: true,
		}, nil
	}

	var p keychainInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch t.backend {
	case "secret-tool":
		return t.executeSecretTool(ctx, p)
	case "pass":
		return t.executePass(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// secret-tool implementation (GNOME Keyring / libsecret)
// ============================================================================

func (t *KeychainTool) executeSecretTool(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "get":
		return t.secretToolGet(ctx, p)
	case "find":
		return t.secretToolFind(ctx, p)
	case "add":
		return t.secretToolAdd(ctx, p)
	case "delete":
		return t.secretToolDelete(ctx, p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *KeychainTool) secretToolGet(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}

	args := []string{"lookup"}
	args = append(args, "service", p.Service)
	if p.Account != "" {
		args = append(args, "account", p.Account)
	}

	cmd := exec.CommandContext(ctx, "secret-tool", args...)
	out, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("No password found for service '%s'", p.Service)}, nil
	}

	password := strings.TrimSpace(string(out))
	if password == "" {
		return &ToolResult{Content: fmt.Sprintf("No password found for service '%s'", p.Service)}, nil
	}

	// Mask the password for display (show first 2 chars)
	masked := password
	if len(password) > 4 {
		masked = password[:2] + strings.Repeat("*", len(password)-2)
	}

	return &ToolResult{Content: fmt.Sprintf("Password for %s: %s", p.Service, masked)}, nil
}

func (t *KeychainTool) secretToolFind(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	// secret-tool search returns all matching entries
	args := []string{"search", "--all"}

	if p.Service != "" {
		args = append(args, "service", p.Service)
	}
	if p.Account != "" {
		args = append(args, "account", p.Account)
	}

	// If no filters, search by nebo label
	if p.Service == "" && p.Account == "" {
		args = append(args, "application", "nebo")
	}

	cmd := exec.CommandContext(ctx, "secret-tool", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if output == "" || strings.Contains(output, "No matching") {
			return &ToolResult{Content: "No credentials found"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No credentials found"}, nil
	}

	// Parse and format results
	var results []string
	lines := strings.Split(output, "\n")
	var currentEntry string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "[") {
			if currentEntry != "" {
				results = append(results, currentEntry)
			}
			currentEntry = line
		} else if strings.Contains(line, "=") && currentEntry != "" {
			parts := strings.SplitN(line, "=", 2)
			if len(parts) == 2 {
				key := strings.TrimSpace(parts[0])
				value := strings.TrimSpace(parts[1])
				if key == "service" || key == "account" || key == "label" {
					currentEntry += fmt.Sprintf(" %s=%s", key, value)
				}
			}
		}
	}
	if currentEntry != "" {
		results = append(results, currentEntry)
	}

	if len(results) == 0 {
		return &ToolResult{Content: "No credentials found"}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Found %d credentials:\n%s", len(results), strings.Join(results, "\n"))}, nil
}

func (t *KeychainTool) secretToolAdd(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}
	if p.Password == "" {
		return &ToolResult{Content: "Password is required", IsError: true}, nil
	}

	label := p.Service
	if p.Account != "" {
		label = fmt.Sprintf("%s (%s)", p.Service, p.Account)
	}

	args := []string{"store", "--label=" + label}
	args = append(args, "service", p.Service)
	args = append(args, "application", "nebo")
	if p.Account != "" {
		args = append(args, "account", p.Account)
	}

	cmd := exec.CommandContext(ctx, "secret-tool", args...)
	cmd.Stdin = strings.NewReader(p.Password)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to store password: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Password stored for service '%s'", p.Service)}, nil
}

func (t *KeychainTool) secretToolDelete(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}

	args := []string{"clear"}
	args = append(args, "service", p.Service)
	if p.Account != "" {
		args = append(args, "account", p.Account)
	}

	cmd := exec.CommandContext(ctx, "secret-tool", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to delete password: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Password deleted for service '%s'", p.Service)}, nil
}

// ============================================================================
// pass implementation (password-store with GPG)
// ============================================================================

func (t *KeychainTool) executePass(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "get":
		return t.passGet(ctx, p)
	case "find":
		return t.passFind(ctx, p)
	case "add":
		return t.passAdd(ctx, p)
	case "delete":
		return t.passDelete(ctx, p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *KeychainTool) passPath(p keychainInputLinux) string {
	// Build path: nebo/service/account or nebo/service
	path := "nebo"
	if p.Service != "" {
		path = filepath.Join(path, p.Service)
	}
	if p.Account != "" {
		path = filepath.Join(path, p.Account)
	}
	return path
}

func (t *KeychainTool) passGet(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}

	path := t.passPath(p)
	cmd := exec.CommandContext(ctx, "pass", "show", path)
	out, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("No password found for '%s'", path)}, nil
	}

	// First line is the password
	lines := strings.SplitN(string(out), "\n", 2)
	password := strings.TrimSpace(lines[0])
	if password == "" {
		return &ToolResult{Content: fmt.Sprintf("No password found for '%s'", path)}, nil
	}

	// Mask the password
	masked := password
	if len(password) > 4 {
		masked = password[:2] + strings.Repeat("*", len(password)-2)
	}

	return &ToolResult{Content: fmt.Sprintf("Password for %s: %s", path, masked)}, nil
}

func (t *KeychainTool) passFind(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	query := p.Service
	if query == "" {
		query = "nebo"
	}

	cmd := exec.CommandContext(ctx, "pass", "find", query)
	out, err := cmd.CombinedOutput()
	if err != nil {
		// pass find returns error if no matches
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "not found") || output == "" {
			return &ToolResult{Content: "No matching credentials found"}, nil
		}
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No matching credentials found"}, nil
	}

	// Format the tree output
	lines := strings.Split(output, "\n")
	var results []string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line != "" && !strings.HasPrefix(line, "Search Terms:") {
			results = append(results, line)
		}
	}

	if len(results) == 0 {
		return &ToolResult{Content: "No matching credentials found"}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Found credentials:\n%s", strings.Join(results, "\n"))}, nil
}

func (t *KeychainTool) passAdd(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}
	if p.Password == "" {
		return &ToolResult{Content: "Password is required", IsError: true}, nil
	}

	path := t.passPath(p)

	// Use pass insert with echo to avoid interactive prompt
	cmd := exec.CommandContext(ctx, "pass", "insert", "-f", "-m", path)
	cmd.Stdin = strings.NewReader(p.Password + "\n")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to store password: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Password stored at '%s'", path)}, nil
}

func (t *KeychainTool) passDelete(ctx context.Context, p keychainInputLinux) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}

	path := t.passPath(p)
	cmd := exec.CommandContext(ctx, "pass", "rm", "-f", path)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to delete password: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Password deleted at '%s'", path)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewKeychainTool(),
		Platforms: []string{PlatformLinux},
		Category:  "security",
	})
}
