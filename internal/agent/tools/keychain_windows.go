//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// KeychainTool provides Windows credential storage via Credential Manager.
type KeychainTool struct{}

func NewKeychainTool() *KeychainTool {
	return &KeychainTool{}
}

func (t *KeychainTool) Name() string { return "keychain" }

func (t *KeychainTool) Description() string {
	return "Access Credentials (using Windows Credential Manager) - securely store and retrieve passwords and API keys."
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

type keychainInputWin struct {
	Action   string `json:"action"`
	Service  string `json:"service"`
	Account  string `json:"account"`
	Password string `json:"password"`
	Kind     string `json:"kind"`
}

func (t *KeychainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p keychainInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "get":
		return t.getCredential(ctx, p)
	case "find":
		return t.findCredentials(ctx, p)
	case "add":
		return t.addCredential(ctx, p)
	case "delete":
		return t.deleteCredential(ctx, p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *KeychainTool) credentialTarget(p keychainInputWin) string {
	// Build target name: nebo:service or nebo:service:account
	target := "nebo:" + p.Service
	if p.Account != "" {
		target += ":" + p.Account
	}
	return target
}

func (t *KeychainTool) getCredential(ctx context.Context, p keychainInputWin) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}

	target := t.credentialTarget(p)

	// Use PowerShell with CredentialManager module or native .NET
	script := fmt.Sprintf(`
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public class CredManager {
    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CredRead(string target, int type, int reserved, out IntPtr credential);

    [DllImport("advapi32.dll")]
    public static extern void CredFree(IntPtr credential);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct CREDENTIAL {
        public int Flags;
        public int Type;
        public string TargetName;
        public string Comment;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
        public int CredentialBlobSize;
        public IntPtr CredentialBlob;
        public int Persist;
        public int AttributeCount;
        public IntPtr Attributes;
        public string TargetAlias;
        public string UserName;
    }

    public static string GetPassword(string target) {
        IntPtr credPtr;
        if (CredRead(target, 1, 0, out credPtr)) {
            CREDENTIAL cred = (CREDENTIAL)Marshal.PtrToStructure(credPtr, typeof(CREDENTIAL));
            string password = Marshal.PtrToStringUni(cred.CredentialBlob, cred.CredentialBlobSize / 2);
            CredFree(credPtr);
            return password;
        }
        return null;
    }
}
"@

$password = [CredManager]::GetPassword("%s")
if ($password) {
    Write-Output $password
} else {
    Write-Error "Credential not found"
}
`, escapeCredPS(target))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("No password found for service '%s'", p.Service)}, nil
	}

	password := strings.TrimSpace(string(out))
	if password == "" {
		return &ToolResult{Content: fmt.Sprintf("No password found for service '%s'", p.Service)}, nil
	}

	// Mask the password for display
	masked := password
	if len(password) > 4 {
		masked = password[:2] + strings.Repeat("*", len(password)-2)
	}

	return &ToolResult{Content: fmt.Sprintf("Password for %s: %s", p.Service, masked)}, nil
}

func (t *KeychainTool) findCredentials(ctx context.Context, p keychainInputWin) (*ToolResult, error) {
	filter := "nebo:*"
	if p.Service != "" {
		filter = "nebo:" + p.Service + "*"
	}

	// Use cmdkey to list credentials
	script := fmt.Sprintf(`
$creds = cmdkey /list:"%s" 2>$null | Select-String -Pattern "Target:" | ForEach-Object {
    $line = $_.Line -replace "^\s+Target:\s+", ""
    $line
}
if ($creds.Count -eq 0) {
    Write-Output "No credentials found"
} else {
    $creds | ForEach-Object { Write-Output $_ }
}
`, filter)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: "No credentials found"}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || output == "No credentials found" {
		return &ToolResult{Content: "No credentials found"}, nil
	}

	lines := strings.Split(output, "\n")
	var results []string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "nebo:") {
			// Parse target to service:account format
			parts := strings.SplitN(line, ":", 3)
			if len(parts) >= 2 {
				service := parts[1]
				account := ""
				if len(parts) >= 3 {
					account = parts[2]
				}
				if account != "" {
					results = append(results, fmt.Sprintf("- %s (account: %s)", service, account))
				} else {
					results = append(results, fmt.Sprintf("- %s", service))
				}
			}
		}
	}

	if len(results) == 0 {
		return &ToolResult{Content: "No credentials found"}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Found %d credentials:\n%s", len(results), strings.Join(results, "\n"))}, nil
}

func (t *KeychainTool) addCredential(ctx context.Context, p keychainInputWin) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}
	if p.Password == "" {
		return &ToolResult{Content: "Password is required", IsError: true}, nil
	}

	target := t.credentialTarget(p)
	username := p.Account
	if username == "" {
		username = "nebo"
	}

	// Use cmdkey to add credential
	script := fmt.Sprintf(`
cmdkey /generic:"%s" /user:"%s" /pass:"%s" 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Output "Credential stored successfully"
} else {
    Write-Error "Failed to store credential"
}
`, escapeCredPS(target), escapeCredPS(username), escapeCredPS(p.Password))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to store credential: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if strings.Contains(output, "successfully") || strings.Contains(output, "CMDKEY: Credential added") {
		return &ToolResult{Content: fmt.Sprintf("Password stored for service '%s'", p.Service)}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Failed to store credential: %s", output), IsError: true}, nil
}

func (t *KeychainTool) deleteCredential(ctx context.Context, p keychainInputWin) (*ToolResult, error) {
	if p.Service == "" {
		return &ToolResult{Content: "Service is required", IsError: true}, nil
	}

	target := t.credentialTarget(p)

	// Use cmdkey to delete credential
	script := fmt.Sprintf(`
cmdkey /delete:"%s" 2>&1
if ($LASTEXITCODE -eq 0) {
    Write-Output "Credential deleted successfully"
} else {
    Write-Error "Failed to delete credential"
}
`, escapeCredPS(target))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to delete credential: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if strings.Contains(output, "successfully") || strings.Contains(output, "deleted") {
		return &ToolResult{Content: fmt.Sprintf("Password deleted for service '%s'", p.Service)}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Credential not found for service '%s'", p.Service)}, nil
}

func escapeCredPS(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}
