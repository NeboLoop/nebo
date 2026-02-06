//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// ContactsTool provides Linux contacts integration via khard or abook.
type ContactsTool struct {
	backend string // "khard", "abook", or ""
}

func NewContactsTool() *ContactsTool {
	t := &ContactsTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *ContactsTool) detectBackend() string {
	if _, err := exec.LookPath("khard"); err == nil {
		return "khard"
	}
	if _, err := exec.LookPath("abook"); err == nil {
		return "abook"
	}
	return ""
}

func (t *ContactsTool) Name() string { return "contacts" }

func (t *ContactsTool) Description() string {
	if t.backend == "" {
		return "Access Contacts - requires khard or abook to be installed."
	}
	return fmt.Sprintf("Access Contacts (using %s) - search for people, look up phone numbers and emails.", t.backend)
}

func (t *ContactsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["search", "get", "create", "groups"],
				"description": "Action: search, get (details), create (new contact), groups (list groups)"
			},
			"query": {"type": "string", "description": "Search query - name, email, or phone"},
			"first_name": {"type": "string", "description": "First name (for create)"},
			"last_name": {"type": "string", "description": "Last name (for create)"},
			"email": {"type": "string", "description": "Email address (for create)"},
			"phone": {"type": "string", "description": "Phone number (for create)"},
			"company": {"type": "string", "description": "Company name (for create)"},
			"notes": {"type": "string", "description": "Notes (for create)"}
		},
		"required": ["action"]
	}`)
}

func (t *ContactsTool) RequiresApproval() bool { return false }

type contactsInputLinux struct {
	Action    string `json:"action"`
	Query     string `json:"query"`
	FirstName string `json:"first_name"`
	LastName  string `json:"last_name"`
	Email     string `json:"email"`
	Phone     string `json:"phone"`
	Company   string `json:"company"`
	Notes     string `json:"notes"`
}

func (t *ContactsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No contacts backend available. Please install one of:\n" +
				"  - khard: pip install khard (recommended, requires vdirsyncer for sync)\n" +
				"  - abook: sudo apt install abook (Debian/Ubuntu)",
			IsError: true,
		}, nil
	}

	var p contactsInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch t.backend {
	case "khard":
		return t.executeKhard(ctx, p)
	case "abook":
		return t.executeAbook(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// khard implementation
// ============================================================================

func (t *ContactsTool) executeKhard(ctx context.Context, p contactsInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "search":
		return t.khardSearch(ctx, p.Query)
	case "get":
		return t.khardGet(ctx, p.Query)
	case "create":
		return t.khardCreate(ctx, p)
	case "groups":
		return t.khardGroups(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *ContactsTool) khardSearch(ctx context.Context, query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	cmd := exec.CommandContext(ctx, "khard", "list", query)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if output == "" || strings.Contains(output, "Found no contacts") {
			return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || strings.Contains(output, "Found no contacts") {
		return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contacts matching '%s':\n%s", query, output)}, nil
}

func (t *ContactsTool) khardGet(ctx context.Context, query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Contact name is required", IsError: true}, nil
	}

	cmd := exec.CommandContext(ctx, "khard", "show", query)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "Found no contacts") {
			return &ToolResult{Content: fmt.Sprintf("No contact found matching '%s'", query)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *ContactsTool) khardCreate(ctx context.Context, p contactsInputLinux) (*ToolResult, error) {
	if p.FirstName == "" && p.LastName == "" {
		return &ToolResult{Content: "First name or last name is required", IsError: true}, nil
	}

	// Build vCard content
	var vcard strings.Builder
	vcard.WriteString("BEGIN:VCARD\n")
	vcard.WriteString("VERSION:3.0\n")

	fullName := strings.TrimSpace(p.FirstName + " " + p.LastName)
	vcard.WriteString(fmt.Sprintf("FN:%s\n", fullName))
	vcard.WriteString(fmt.Sprintf("N:%s;%s;;;\n", p.LastName, p.FirstName))

	if p.Email != "" {
		vcard.WriteString(fmt.Sprintf("EMAIL;TYPE=WORK:%s\n", p.Email))
	}
	if p.Phone != "" {
		vcard.WriteString(fmt.Sprintf("TEL;TYPE=CELL:%s\n", p.Phone))
	}
	if p.Company != "" {
		vcard.WriteString(fmt.Sprintf("ORG:%s\n", p.Company))
	}
	if p.Notes != "" {
		vcard.WriteString(fmt.Sprintf("NOTE:%s\n", p.Notes))
	}

	vcard.WriteString("END:VCARD\n")

	// Pipe vcard to khard
	cmd := exec.CommandContext(ctx, "khard", "new", "--vcard-version", "3.0", "-")
	cmd.Stdin = strings.NewReader(vcard.String())
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create contact: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Contact created: %s", fullName)}, nil
}

func (t *ContactsTool) khardGroups(ctx context.Context) (*ToolResult, error) {
	// khard doesn't have groups in the traditional sense, but we can list addressbooks
	cmd := exec.CommandContext(ctx, "khard", "addressbooks")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No addressbooks configured"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Addressbooks:\n%s", output)}, nil
}

// ============================================================================
// abook implementation
// ============================================================================

func (t *ContactsTool) executeAbook(ctx context.Context, p contactsInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "search":
		return t.abookSearch(ctx, p.Query)
	case "get":
		return t.abookSearch(ctx, p.Query) // abook doesn't have separate get, use search
	case "create":
		return &ToolResult{Content: "Creating contacts via abook requires interactive mode. Run 'abook' to add contacts."}, nil
	case "groups":
		return &ToolResult{Content: "abook does not support contact groups"}, nil
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *ContactsTool) abookSearch(ctx context.Context, query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	// abook uses mutt-query format for searching
	cmd := exec.CommandContext(ctx, "abook", "--mutt-query", query)
	out, err := cmd.CombinedOutput()
	if err != nil {
		// abook returns error if no results
		output := strings.TrimSpace(string(out))
		if output == "" {
			return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v", err), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contacts matching '%s':\n%s", query, output)}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewContactsTool(),
		Platforms: []string{PlatformLinux},
		Category:  "productivity",
	})
}
