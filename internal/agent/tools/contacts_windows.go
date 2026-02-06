//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// ContactsTool provides Windows contacts integration via Outlook COM.
type ContactsTool struct {
	hasOutlook bool
}

func NewContactsTool() *ContactsTool {
	t := &ContactsTool{}
	t.hasOutlook = t.checkOutlook()
	return t
}

func (t *ContactsTool) checkOutlook() bool {
	script := `try { $null = New-Object -ComObject Outlook.Application; Write-Output "true" } catch { Write-Output "false" }`
	cmd := exec.Command("powershell", "-NoProfile", "-Command", script)
	out, err := cmd.Output()
	if err != nil {
		return false
	}
	return strings.TrimSpace(string(out)) == "true"
}

func (t *ContactsTool) Name() string { return "contacts" }

func (t *ContactsTool) Description() string {
	if !t.hasOutlook {
		return "Access Contacts - requires Microsoft Outlook to be installed."
	}
	return "Access Contacts (using Outlook) - search for people, look up phone numbers and emails, create new contacts."
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

type contactsInputWin struct {
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
	if !t.hasOutlook {
		return &ToolResult{
			Content: "Microsoft Outlook is not installed or not accessible.\n\n" +
				"Contacts integration on Windows requires Outlook. Please install Microsoft Outlook to use this feature.",
			IsError: true,
		}, nil
	}

	var p contactsInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "search":
		return t.searchContacts(ctx, p.Query)
	case "get":
		return t.getContact(ctx, p.Query)
	case "create":
		return t.createContact(ctx, p)
	case "groups":
		return t.listGroups(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *ContactsTool) searchContacts(ctx context.Context, query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$contacts = $namespace.GetDefaultFolder(10).Items
$results = @()
foreach ($contact in $contacts) {
    if ($contact.FullName -like "*%s*" -or $contact.Email1Address -like "*%s*") {
        $email = if ($contact.Email1Address) { $contact.Email1Address } else { "N/A" }
        $phone = if ($contact.BusinessTelephoneNumber) { $contact.BusinessTelephoneNumber } elseif ($contact.MobileTelephoneNumber) { $contact.MobileTelephoneNumber } else { "N/A" }
        $results += "$($contact.FullName) | $email | $phone"
    }
}
if ($results.Count -eq 0) {
    "No contacts found"
} else {
    $results -join [char]10
}
`, escapePSContactsQuery(query), escapePSContactsQuery(query))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || output == "No contacts found" {
		return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contacts matching '%s':\n%s", query, output)}, nil
}

func (t *ContactsTool) getContact(ctx context.Context, query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Contact name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$contacts = $namespace.GetDefaultFolder(10).Items
$found = $null
foreach ($contact in $contacts) {
    if ($contact.FullName -like "*%s*") {
        $found = $contact
        break
    }
}
if ($found -eq $null) {
    "Contact not found"
} else {
    $details = @()
    $details += "Name: $($found.FullName)"
    if ($found.CompanyName) { $details += "Company: $($found.CompanyName)" }
    if ($found.Email1Address) { $details += "Email (Work): $($found.Email1Address)" }
    if ($found.Email2Address) { $details += "Email (Other): $($found.Email2Address)" }
    if ($found.BusinessTelephoneNumber) { $details += "Phone (Business): $($found.BusinessTelephoneNumber)" }
    if ($found.MobileTelephoneNumber) { $details += "Phone (Mobile): $($found.MobileTelephoneNumber)" }
    if ($found.HomeTelephoneNumber) { $details += "Phone (Home): $($found.HomeTelephoneNumber)" }
    if ($found.BusinessAddress) { $details += "Address: $($found.BusinessAddress)" }
    if ($found.Body) { $details += "Notes: $($found.Body)" }
    $details -join [char]10
}
`, escapePSContactsQuery(query))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "Contact not found" {
		return &ToolResult{Content: fmt.Sprintf("No contact found matching '%s'", query)}, nil
	}
	return &ToolResult{Content: output}, nil
}

func (t *ContactsTool) createContact(ctx context.Context, p contactsInputWin) (*ToolResult, error) {
	if p.FirstName == "" && p.LastName == "" {
		return &ToolResult{Content: "First name or last name is required", IsError: true}, nil
	}

	script := `
$outlook = New-Object -ComObject Outlook.Application
$contact = $outlook.CreateItem(2)
`
	if p.FirstName != "" {
		script += fmt.Sprintf(`$contact.FirstName = "%s"
`, escapePSContactsQuery(p.FirstName))
	}
	if p.LastName != "" {
		script += fmt.Sprintf(`$contact.LastName = "%s"
`, escapePSContactsQuery(p.LastName))
	}
	if p.Email != "" {
		script += fmt.Sprintf(`$contact.Email1Address = "%s"
`, escapePSContactsQuery(p.Email))
	}
	if p.Phone != "" {
		script += fmt.Sprintf(`$contact.MobileTelephoneNumber = "%s"
`, escapePSContactsQuery(p.Phone))
	}
	if p.Company != "" {
		script += fmt.Sprintf(`$contact.CompanyName = "%s"
`, escapePSContactsQuery(p.Company))
	}
	if p.Notes != "" {
		script += fmt.Sprintf(`$contact.Body = "%s"
`, escapePSContactsQuery(p.Notes))
	}

	script += `$contact.Save()
Write-Output "Contact created successfully"
`

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create contact: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	fullName := strings.TrimSpace(p.FirstName + " " + p.LastName)
	return &ToolResult{Content: fmt.Sprintf("Contact created: %s", fullName)}, nil
}

func (t *ContactsTool) listGroups(ctx context.Context) (*ToolResult, error) {
	script := `
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$contactsFolder = $namespace.GetDefaultFolder(10)
$results = @()
foreach ($folder in $contactsFolder.Folders) {
    $count = $folder.Items.Count
    $results += "$($folder.Name) ($count contacts)"
}
if ($results.Count -eq 0) {
    "No contact folders found"
} else {
    $results -join [char]10
}
`

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || output == "No contact folders found" {
		return &ToolResult{Content: "No contact groups found"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contact folders:\n%s", output)}, nil
}

func escapePSContactsQuery(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewContactsTool(),
		Platforms: []string{PlatformWindows},
		Category:  "productivity",
	})
}
