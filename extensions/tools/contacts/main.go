// Contacts Plugin - macOS Contacts integration via AppleScript
// Build: go build -o ~/.gobot/plugins/tools/contacts
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

type ContactsTool struct{}

type contactsInput struct {
	Action    string `json:"action"`     // search, get, create, groups
	Query     string `json:"query"`      // Search query (name, email, phone)
	FirstName string `json:"first_name"` // First name
	LastName  string `json:"last_name"`  // Last name
	Email     string `json:"email"`      // Email address
	Phone     string `json:"phone"`      // Phone number
	Company   string `json:"company"`    // Company name
	Notes     string `json:"notes"`      // Notes
	Group     string `json:"group"`      // Group name
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *ContactsTool) Name() string {
	return "contacts"
}

func (t *ContactsTool) Description() string {
	return "Access macOS Contacts - search for people, look up phone numbers and emails, create new contacts."
}

func (t *ContactsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["search", "get", "create", "groups"],
				"description": "Action: search (find contacts), get (contact details), create (new contact), groups (list groups)"
			},
			"query": {
				"type": "string",
				"description": "Search query - name, email, or phone number"
			},
			"first_name": {
				"type": "string",
				"description": "First name (for create)"
			},
			"last_name": {
				"type": "string",
				"description": "Last name (for create)"
			},
			"email": {
				"type": "string",
				"description": "Email address (for create)"
			},
			"phone": {
				"type": "string",
				"description": "Phone number (for create)"
			},
			"company": {
				"type": "string",
				"description": "Company name (for create)"
			},
			"notes": {
				"type": "string",
				"description": "Notes (for create)"
			},
			"group": {
				"type": "string",
				"description": "Group name (for filtering or adding contact to group)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *ContactsTool) RequiresApproval() bool {
	return false
}

func (t *ContactsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params contactsInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "search":
		return t.searchContacts(params.Query)
	case "get":
		return t.getContactDetails(params.Query)
	case "create":
		return t.createContact(params)
	case "groups":
		return t.listGroups()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *ContactsTool) searchContacts(query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
		tell application "Contacts"
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
		end tell
	`, escapeAppleScript(query))

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v", err), IsError: true}, nil
	}
	if output == "" || output == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No contacts found matching '%s'", query), IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contacts matching '%s':\n%s", query, output), IsError: false}, nil
}

func (t *ContactsTool) getContactDetails(query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Contact name or query is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
		tell application "Contacts"
			set foundPeople to every person whose name contains "%s"
			if (count of foundPeople) = 0 then
				return "No contact found"
			end if
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

			if (count of addresses of p) > 0 then
				set details to details & "Addresses:" & return
				repeat with addr in addresses of p
					set details to details & "  " & label of addr & ": " & formatted address of addr & return
				end repeat
			end if

			if note of p is not missing value then
				set details to details & "Notes: " & note of p & return
			end if

			return details
		end tell
	`, escapeAppleScript(query))

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get contact: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *ContactsTool) createContact(params contactsInput) (*ToolResult, error) {
	if params.FirstName == "" && params.LastName == "" {
		return &ToolResult{Content: "First name or last name is required", IsError: true}, nil
	}

	script := `tell application "Contacts"
		set newPerson to make new person with properties {`

	props := []string{}
	if params.FirstName != "" {
		props = append(props, fmt.Sprintf(`first name:"%s"`, escapeAppleScript(params.FirstName)))
	}
	if params.LastName != "" {
		props = append(props, fmt.Sprintf(`last name:"%s"`, escapeAppleScript(params.LastName)))
	}
	if params.Company != "" {
		props = append(props, fmt.Sprintf(`company:"%s"`, escapeAppleScript(params.Company)))
	}
	if params.Notes != "" {
		props = append(props, fmt.Sprintf(`note:"%s"`, escapeAppleScript(params.Notes)))
	}

	script += strings.Join(props, ", ") + "}"

	if params.Email != "" {
		script += fmt.Sprintf(`
		tell newPerson
			make new email at end of emails with properties {label:"work", value:"%s"}
		end tell`, escapeAppleScript(params.Email))
	}

	if params.Phone != "" {
		script += fmt.Sprintf(`
		tell newPerson
			make new phone at end of phones with properties {label:"mobile", value:"%s"}
		end tell`, escapeAppleScript(params.Phone))
	}

	script += `
		save
	end tell
	return "Contact created successfully"`

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create contact: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *ContactsTool) listGroups() (*ToolResult, error) {
	script := `
		tell application "Contacts"
			set groupList to {}
			repeat with g in groups
				set groupInfo to name of g & " (" & (count of people of g) & " contacts)"
				set end of groupList to groupInfo
			end repeat
			return groupList
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list groups: %v", err), IsError: true}, nil
	}
	if output == "" || output == "{}" {
		return &ToolResult{Content: "No contact groups found", IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contact groups:\n%s", output), IsError: false}, nil
}

func escapeAppleScript(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return s
}

func runAppleScript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

// RPC wrapper
type ContactsToolRPC struct {
	tool *ContactsTool
}

func (t *ContactsToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *ContactsToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *ContactsToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *ContactsToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *ContactsToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type ContactsPlugin struct {
	tool *ContactsTool
}

func (p *ContactsPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &ContactsToolRPC{tool: p.tool}, nil
}

func (p *ContactsPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &ContactsPlugin{tool: &ContactsTool{}},
		},
	})
}
