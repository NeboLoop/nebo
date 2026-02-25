//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// ContactsTool provides macOS Contacts integration via AppleScript.
type ContactsTool struct{}

func NewContactsTool() *ContactsTool { return &ContactsTool{} }

func (t *ContactsTool) Name() string { return "contacts" }

func (t *ContactsTool) Description() string {
	return "Access Contacts - search for people, look up phone numbers and emails, create new contacts."
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

type contactsInput struct {
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
	var p contactsInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "search":
		return t.searchContacts(p.Query)
	case "get":
		return t.getContactDetails(p.Query)
	case "create":
		return t.createContact(p)
	case "groups":
		return t.listGroups()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *ContactsTool) searchContacts(query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "Contacts"
		set foundPeople to every person whose name contains "%s"
		set resultList to {}
		repeat with p in foundPeople
			set personInfo to name of p
			if (count of emails of p) > 0 then
				set personInfo to personInfo & " | " & (value of first email of p)
			end if
			if (count of phones of p) > 0 then
				set personInfo to personInfo & " | " & (value of first phone of p)
			end if
			set end of resultList to personInfo
		end repeat
		return resultList
	end tell`, escapeAS(query))
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contacts matching '%s':\n%s", query, out)}, nil
}

func (t *ContactsTool) getContactDetails(query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Contact name is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "Contacts"
		set foundPeople to every person whose name contains "%s"
		if (count of foundPeople) = 0 then return "No contact found"
		set p to first item of foundPeople
		set details to "Name: " & name of p & return
		if company of p is not missing value then
			set details to details & "Company: " & company of p & return
		end if
		if (count of emails of p) > 0 then
			set details to details & "Emails:" & return
			repeat with e in emails of p
				set details to details & "  " & label of e & ": " & value of e & return
			end repeat
		end if
		if (count of phones of p) > 0 then
			set details to details & "Phones:" & return
			repeat with ph in phones of p
				set details to details & "  " & label of ph & ": " & value of ph & return
			end repeat
		end if
		if note of p is not missing value then
			set details to details & "Notes: " & note of p & return
		end if
		return details
	end tell`, escapeAS(query))
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *ContactsTool) createContact(p contactsInput) (*ToolResult, error) {
	if p.FirstName == "" && p.LastName == "" {
		return &ToolResult{Content: "First name or last name is required", IsError: true}, nil
	}

	var props []string
	if p.FirstName != "" {
		props = append(props, fmt.Sprintf(`first name:"%s"`, escapeAS(p.FirstName)))
	}
	if p.LastName != "" {
		props = append(props, fmt.Sprintf(`last name:"%s"`, escapeAS(p.LastName)))
	}
	if p.Company != "" {
		props = append(props, fmt.Sprintf(`company:"%s"`, escapeAS(p.Company)))
	}
	if p.Notes != "" {
		props = append(props, fmt.Sprintf(`note:"%s"`, escapeAS(p.Notes)))
	}

	script := fmt.Sprintf(`tell application "Contacts"
		set newPerson to make new person with properties {%s}`, strings.Join(props, ", "))
	if p.Email != "" {
		script += fmt.Sprintf(`
		tell newPerson
			make new email at end of emails with properties {label:"work", value:"%s"}
		end tell`, escapeAS(p.Email))
	}
	if p.Phone != "" {
		script += fmt.Sprintf(`
		tell newPerson
			make new phone at end of phones with properties {label:"mobile", value:"%s"}
		end tell`, escapeAS(p.Phone))
	}
	script += `
		save
	end tell
	return "Contact created successfully"`

	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *ContactsTool) listGroups() (*ToolResult, error) {
	script := `tell application "Contacts"
		set groupList to {}
		repeat with g in groups
			set groupInfo to name of g & " (" & (count of people of g) & " contacts)"
			set end of groupList to groupInfo
		end repeat
		return groupList
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: "No contact groups found"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contact groups:\n%s", out)}, nil
}

