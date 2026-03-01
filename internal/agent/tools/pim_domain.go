package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// OrganizerDomainTool consolidates personal information management tools (mail, contacts,
// calendar, reminders) into a single STRAP domain tool with resource-based routing.
// Each resource delegates to the existing platform-specific tool implementation.
type OrganizerDomainTool struct {
	subTools map[string]Tool // resource name → sub-tool
}

// NewOrganizerDomainTool creates a PIM domain tool with the given sub-tools.
// Pass nil for resources not available on this platform.
func NewOrganizerDomainTool(mail, contacts, calendar, reminders Tool) *OrganizerDomainTool {
	t := &OrganizerDomainTool{
		subTools: make(map[string]Tool),
	}
	if mail != nil {
		t.subTools["mail"] = mail
	}
	if contacts != nil {
		t.subTools["contacts"] = contacts
	}
	if calendar != nil {
		t.subTools["calendar"] = calendar
	}
	if reminders != nil {
		t.subTools["reminders"] = reminders
	}
	return t
}

func (t *OrganizerDomainTool) Name() string        { return "organizer" }
func (t *OrganizerDomainTool) Domain() string       { return "organizer" }
func (t *OrganizerDomainTool) RequiresApproval() bool { return true }

func (t *OrganizerDomainTool) Resources() []string {
	resources := make([]string, 0, len(t.subTools))
	for r := range t.subTools {
		resources = append(resources, r)
	}
	return resources
}

func (t *OrganizerDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "mail":
		return []string{"accounts", "unread", "read", "send", "search"}
	case "contacts":
		return []string{"search", "get", "create", "groups"}
	case "calendar":
		return []string{"calendars", "today", "upcoming", "create", "list"}
	case "reminders":
		return []string{"lists", "list", "create", "complete", "delete"}
	default:
		return nil
	}
}

func (t *OrganizerDomainTool) Description() string {
	return BuildDomainDescription(t.schemaConfig())
}

func (t *OrganizerDomainTool) Schema() json.RawMessage {
	return BuildDomainSchema(t.schemaConfig())
}

func (t *OrganizerDomainTool) schemaConfig() DomainSchemaConfig {
	resources := make(map[string]ResourceConfig)
	for name := range t.subTools {
		resources[name] = ResourceConfig{
			Name:    name,
			Actions: t.ActionsFor(name),
		}
	}

	return DomainSchemaConfig{
		Domain:      "organizer",
		Description: "Personal information management: mail, contacts, calendar, reminders.",
		Resources:   resources,
		Fields: []FieldConfig{
			{Name: "to", Type: "array", Description: "Recipients (for mail send) or single-element array (for messages send)", Items: "string"},
			{Name: "cc", Type: "array", Description: "CC recipients (for mail send)", Items: "string"},
			{Name: "subject", Type: "string", Description: "Email subject (for mail send)"},
			{Name: "body", Type: "string", Description: "Email body (for mail send)"},
			{Name: "mailbox", Type: "string", Description: "Mailbox name (INBOX, Sent, etc.)"},
			{Name: "query", Type: "string", Description: "Search query"},
			{Name: "count", Type: "integer", Description: "Number of items to fetch (default: 10)"},
			{Name: "name", Type: "string", Description: "Contact/reminder name or list name"},
			{Name: "email", Type: "string", Description: "Email address (for contacts create)"},
			{Name: "phone", Type: "string", Description: "Phone number (for contacts create)"},
			{Name: "title", Type: "string", Description: "Event/reminder title"},
			{Name: "date", Type: "string", Description: "Date (e.g., '2024-01-15')"},
			{Name: "time", Type: "string", Description: "Time (e.g., '14:00')"},
			{Name: "duration", Type: "integer", Description: "Duration in minutes"},
			{Name: "location", Type: "string", Description: "Event location"},
			{Name: "notes", Type: "string", Description: "Additional notes"},
			{Name: "calendar_name", Type: "string", Description: "Calendar name"},
			{Name: "reminder_list", Type: "string", Description: "Reminder list name"},
			{Name: "reminder_id", Type: "string", Description: "Reminder ID (for complete/delete)"},
			{Name: "due_date", Type: "string", Description: "Due date for reminder"},
			{Name: "priority", Type: "integer", Description: "Priority (0=none, 1=high, 5=medium, 9=low)"},
			{Name: "days", Type: "integer", Description: "Number of days to look ahead"},
		},
		Examples: []string{
			`organizer(resource: "mail", action: "unread")`,
			`organizer(resource: "mail", action: "send", to: ["alice@example.com"], subject: "Hi", body: "Hello!")`,
			`organizer(resource: "contacts", action: "search", query: "Alice")`,
			`organizer(resource: "calendar", action: "today")`,
			`organizer(resource: "reminders", action: "create", title: "Buy groceries", reminder_list: "Personal")`,
		},
	}
}

// inferResource guesses the resource from the action name when resource is omitted.
func (t *OrganizerDomainTool) inferResource(action string) string {
	switch action {
	case "accounts", "unread":
		return "mail"
	case "groups":
		return "contacts"
	case "calendars", "today", "upcoming":
		return "calendar"
	case "lists", "complete":
		return "reminders"
	default:
		return "" // ambiguous — require explicit resource
	}
}

func (t *OrganizerDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p DomainInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	resource := p.Resource
	if resource == "" {
		resource = t.inferResource(p.Action)
	}
	if resource == "" {
		return &ToolResult{
			Content: fmt.Sprintf("Resource is required. Available: %v", t.Resources()),
			IsError: true,
		}, nil
	}

	sub, ok := t.subTools[resource]
	if !ok {
		return &ToolResult{
			Content: fmt.Sprintf("Resource %q not available on this platform. Available: %v", resource, t.Resources()),
			IsError: true,
		}, nil
	}

	return sub.Execute(ctx, input)
}
