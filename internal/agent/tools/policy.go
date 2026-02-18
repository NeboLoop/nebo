package tools

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

// PolicyLevel defines the security level
type PolicyLevel string

const (
	PolicyDeny      PolicyLevel = "deny"      // Deny all dangerous operations
	PolicyAllowlist PolicyLevel = "allowlist" // Allow only whitelisted commands
	PolicyFull      PolicyLevel = "full"      // Allow all (dangerous!)
)

// AskMode defines when to ask for approval
type AskMode string

const (
	AskModeOff    AskMode = "off"     // Never ask
	AskModeOnMiss AskMode = "on-miss" // Ask only for non-whitelisted
	AskModeAlways AskMode = "always"  // Always ask
)

// ApprovalCallback is called to request approval from a remote source (e.g., web UI)
// It receives the tool name and input, and returns true if approved
type ApprovalCallback func(ctx context.Context, toolName string, input json.RawMessage) (bool, error)

// AutonomousCheck is a live check that returns true when autonomous mode is active.
// This is read on every approval check so settings changes take effect immediately.
type AutonomousCheck func() bool

// Policy manages approval for dangerous operations
type Policy struct {
	Level            PolicyLevel
	AskMode          AskMode
	Allowlist        map[string]bool
	ApprovalCallback ApprovalCallback // If set, used instead of stdin prompts
	IsAutonomous     AutonomousCheck  // If set, checked on every approval call

	// Origin-based tool restrictions: maps Origin -> set of denied tool names.
	// If a tool name appears in the deny list for the current origin, execution
	// is blocked unconditionally (no approval prompt, just denied).
	OriginDenyList map[Origin]map[string]bool
}

// SafeBins are commands that never require approval
var SafeBins = []string{
	"ls", "pwd", "cat", "head", "tail", "grep", "find", "which", "type",
	"jq", "cut", "sort", "uniq", "wc", "echo", "date", "env", "printenv",
	"git status", "git log", "git diff", "git branch", "git show",
	"go version", "node --version", "python --version",
}

// defaultOriginDenyList returns the default per-origin tool restrictions.
// Non-user origins are denied access to dangerous tools by default.
// User origin has no restrictions (governed by existing policy level/approval).
func defaultOriginDenyList() map[Origin]map[string]bool {
	// Tools denied for comm-origin (inter-agent messages)
	commDeny := map[string]bool{
		"shell": true, // No shell access from remote agents
	}

	// Tools denied for plugin-origin (external binaries)
	appDeny := map[string]bool{
		"shell": true, // No shell from apps
	}

	// Tools denied for skill-origin (matched skill templates)
	skillDeny := map[string]bool{
		"shell": true, // No shell from skill templates
	}

	return map[Origin]map[string]bool{
		OriginComm:  commDeny,
		OriginApp:   appDeny,
		OriginSkill: skillDeny,
		// OriginUser: no restrictions (existing policy governs)
		// OriginSystem: no restrictions (internal operations need full access)
	}
}

// NewPolicy creates a new policy with defaults
func NewPolicy() *Policy {
	allowlist := make(map[string]bool)
	for _, cmd := range SafeBins {
		allowlist[cmd] = true
	}

	return &Policy{
		Level:          PolicyAllowlist,
		AskMode:        AskModeOnMiss,
		Allowlist:      allowlist,
		OriginDenyList: defaultOriginDenyList(),
	}
}

// NewPolicyFromConfig creates a policy from config values
func NewPolicyFromConfig(level, askMode string, allowlist []string) *Policy {
	p := NewPolicy()

	switch level {
	case "deny":
		p.Level = PolicyDeny
	case "full":
		p.Level = PolicyFull
	default:
		p.Level = PolicyAllowlist
	}

	switch askMode {
	case "off":
		p.AskMode = AskModeOff
	case "always":
		p.AskMode = AskModeAlways
	default:
		p.AskMode = AskModeOnMiss
	}

	// Add custom allowlist items
	for _, item := range allowlist {
		p.Allowlist[item] = true
	}

	return p
}

// IsDeniedForOrigin returns true if the given tool is blocked for the given origin.
// This is a hard deny — no approval prompt, just rejected.
func (p *Policy) IsDeniedForOrigin(origin Origin, toolName string) bool {
	if p.OriginDenyList == nil {
		return false
	}
	denied, ok := p.OriginDenyList[origin]
	if !ok {
		return false
	}
	return denied[toolName]
}

// RequiresApproval checks if a command requires user approval
func (p *Policy) RequiresApproval(cmd string) bool {
	// Live check: autonomous mode overrides everything
	if p.IsAutonomous != nil && p.IsAutonomous() {
		return false
	}

	if p.Level == PolicyFull {
		return false
	}

	if p.Level == PolicyDeny {
		return true
	}

	// Check allowlist
	if p.isAllowed(cmd) {
		return p.AskMode == AskModeAlways
	}

	return p.AskMode != AskModeOff
}

// isAllowed checks if a command matches the allowlist
func (p *Policy) isAllowed(cmd string) bool {
	cmd = strings.TrimSpace(cmd)

	// Check exact match
	if p.Allowlist[cmd] {
		return true
	}

	// Check if command starts with an allowed prefix
	parts := strings.Fields(cmd)
	if len(parts) > 0 {
		// Check binary name
		if p.Allowlist[parts[0]] {
			return true
		}
		// Check binary with first arg (e.g., "git status")
		if len(parts) > 1 && p.Allowlist[parts[0]+" "+parts[1]] {
			return true
		}
	}

	return false
}

// RequestApproval asks the user for approval
func (p *Policy) RequestApproval(ctx context.Context, toolName string, input json.RawMessage) (bool, error) {
	// System-origin tasks (reminders, heartbeat, recovery) auto-approve —
	// there's nobody to ask and these are internally scheduled by the agent.
	if GetOrigin(ctx) == OriginSystem {
		fmt.Printf("[Policy] Auto-approving (system origin)\n")
		return true, nil
	}

	// Live check: autonomous mode auto-approves everything
	if p.IsAutonomous != nil && p.IsAutonomous() {
		return true, nil
	}

	// Fast path: full policy level means auto-approve everything
	if p.Level == PolicyFull {
		fmt.Printf("[Policy] Auto-approving (full policy level)\n")
		return true, nil
	}

	// Format the request nicely
	var inputStr string
	if toolName == "bash" {
		var bashInput struct {
			Command string `json:"command"`
		}
		if err := json.Unmarshal(input, &bashInput); err == nil {
			inputStr = bashInput.Command
		}
	}
	if inputStr == "" {
		inputStr = string(input)
	}

	// Check if we need to ask at all
	if toolName == "bash" && !p.RequiresApproval(inputStr) {
		fmt.Printf("[Policy] Command is in allowlist, auto-approving\n")
		return true, nil
	}

	// Use callback if set (for remote/web UI approval)
	if p.ApprovalCallback != nil {
		fmt.Printf("[Policy] Requesting approval via callback for tool=%s\n", toolName)
		return p.ApprovalCallback(ctx, toolName, input)
	}

	// Fall back to stdin prompts for CLI mode
	fmt.Printf("\n\033[33m⚠ Tool '%s' requires approval:\033[0m\n", toolName)
	fmt.Printf("\033[90m%s\033[0m\n", inputStr)
	fmt.Print("\033[33mApprove? [y/N/a(lways)]: \033[0m")

	reader := bufio.NewReader(os.Stdin)
	response, err := reader.ReadString('\n')
	if err != nil {
		return false, err
	}

	response = strings.TrimSpace(strings.ToLower(response))

	switch response {
	case "y", "yes":
		return true, nil
	case "a", "always":
		// Add to allowlist for this session
		p.AddToAllowlist(inputStr)
		return true, nil
	default:
		return false, nil
	}
}

// AddToAllowlist adds a command pattern to the allowlist
func (p *Policy) AddToAllowlist(pattern string) {
	p.Allowlist[pattern] = true
}

// IsDangerous checks if a command appears dangerous
func IsDangerous(cmd string) bool {
	dangerous := []string{
		"rm -rf", "rm -r", "rmdir",
		"sudo", "su ",
		"chmod 777", "chown",
		"dd ", "mkfs",
		"> /dev/", ">/dev/",
		"curl | sh", "curl | bash", "wget | sh",
		"eval ", "exec ",
		":(){ :|:& };:",
	}

	cmdLower := strings.ToLower(cmd)
	for _, d := range dangerous {
		if strings.Contains(cmdLower, d) {
			return true
		}
	}
	return false
}
