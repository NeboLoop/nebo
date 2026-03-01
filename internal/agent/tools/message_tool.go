package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// pendingMessageResources collects platform-specific sub-tools registered via init().
// They are pulled into the MsgTool when NewMsgTool is called.
var pendingMessageResources = make(map[string]Tool)

// RegisterMessageResourceInit registers a platform-specific resource for the MsgTool.
// Called from platform init() functions (e.g., pim_domain_darwin.go, system_domain_darwin.go).
func RegisterMessageResourceInit(name string, tool Tool) {
	pendingMessageResources[name] = tool
}

// MsgTool handles outbound delivery to humans outside NeboLoop.
// Resources: owner (always available), sms and notify (platform-specific, registered via init).
// Named MsgTool to avoid collision with legacy MessageTool in message.go (to be deleted).
type MsgTool struct {
	// Owner notification callbacks (from notify_owner.go)
	appendToSession func(content string) error
	sendFrame       func(frame map[string]any) error

	// Platform-specific sub-tools registered via RegisterMessageResource
	platformTools map[string]Tool
}

// NewMsgTool creates a new message domain tool.
func NewMsgTool() *MsgTool {
	t := &MsgTool{
		platformTools: make(map[string]Tool),
	}
	// Pull in platform resources registered via init()
	for name, tool := range pendingMessageResources {
		t.platformTools[name] = tool
	}
	return t
}

// SetOwnerCallbacks sets the callbacks for owner notifications.
func (t *MsgTool) SetOwnerCallbacks(appendToSession func(content string) error, sendFrame func(frame map[string]any) error) {
	t.appendToSession = appendToSession
	t.sendFrame = sendFrame
}

// RegisterMessageResource registers a platform-specific resource (sms, notify).
func (t *MsgTool) RegisterMessageResource(name string, tool Tool) {
	t.platformTools[name] = tool
}

func (t *MsgTool) Name() string   { return "message" }
func (t *MsgTool) Domain() string { return "message" }

func (t *MsgTool) Resources() []string {
	resources := []string{"owner"}
	for name := range t.platformTools {
		resources = append(resources, name)
	}
	return resources
}

func (t *MsgTool) ActionsFor(resource string) []string {
	switch resource {
	case "owner":
		return []string{"notify"}
	case "sms":
		return []string{"send", "conversations", "read", "search"}
	case "notify":
		return []string{"send", "alert", "speak", "dnd_status"}
	default:
		if _, ok := t.platformTools[resource]; ok {
			return []string{"send"} // generic fallback
		}
		return nil
	}
}

func (t *MsgTool) RequiresApproval() bool { return false }

var msgResources = map[string]ResourceConfig{
	"owner":  {Name: "owner", Actions: []string{"notify"}, Description: "Send notification to the owner's Main Chat"},
	"sms":    {Name: "sms", Actions: []string{"send", "conversations", "read", "search"}, Description: "SMS messaging (platform-specific)"},
	"notify": {Name: "notify", Actions: []string{"send", "alert", "speak", "dnd_status"}, Description: "System notifications (platform-specific)"},
}

func (t *MsgTool) Description() string {
	return BuildDomainDescription(DomainSchemaConfig{
		Domain: "message",
		Description: `Outbound message delivery to humans.

Resources:
- owner: Send notifications to the owner's Main Chat from any lane (background tasks, events, loop channels)
- sms: SMS messaging (platform-specific, if available)
- notify: System notifications (platform-specific, if available)`,
		Resources: msgResources,
		Examples: []string{
			`message(resource: owner, action: notify, text: "Your report is ready!")`,
			`message(resource: sms, action: send, to: "+1234567890", text: "Reminder: meeting at 3pm")`,
			`message(resource: notify, action: send, text: "Download complete")`,
		},
	})
}

func (t *MsgTool) Schema() json.RawMessage {
	return BuildDomainSchema(DomainSchemaConfig{
		Domain:      "message",
		Description: t.Description(),
		Resources:   msgResources,
		Fields: []FieldConfig{
			{Name: "text", Type: "string", Description: "Message text / notification content"},
			{Name: "to", Type: "string", Description: "Recipient (phone number for sms)"},
			{Name: "query", Type: "string", Description: "Search query (for sms.search)"},
			{Name: "limit", Type: "integer", Description: "Max results to return"},
		},
	})
}

// MsgInput defines the input for the message domain tool.
type MsgInput struct {
	Resource string `json:"resource"`
	Action   string `json:"action"`
	Text     string `json:"text,omitempty"`
	Message  string `json:"message,omitempty"` // alias for text
	To       string `json:"to,omitempty"`
	Query    string `json:"query,omitempty"`
	Limit    int    `json:"limit,omitempty"`
}

// msgActionToResource maps actions unique to a resource for inference.
var msgActionToResource = map[string]string{
	"conversations": "sms",
	"read":          "sms",
	"alert":         "notify",
	"speak":         "notify",
	"dnd_status":    "notify",
}

func (t *MsgTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in MsgInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Infer resource from action when omitted
	if in.Resource == "" {
		if r, ok := msgActionToResource[in.Action]; ok {
			in.Resource = r
		}
	}

	if err := ValidateResourceAction(in.Resource, in.Action, msgResources); err != nil {
		return &ToolResult{Content: err.Error(), IsError: true}, nil
	}

	switch in.Resource {
	case "owner":
		return t.handleOwner(ctx, in)
	default:
		// Delegate to platform-specific tools (sms, notify)
		if tool, ok := t.platformTools[in.Resource]; ok {
			return tool.Execute(ctx, input)
		}
		return &ToolResult{Content: fmt.Sprintf("Resource %q not available on this platform", in.Resource), IsError: true}, nil
	}
}

// =============================================================================
// Owner notification handler
// =============================================================================

func (t *MsgTool) handleOwner(_ context.Context, in MsgInput) (*ToolResult, error) {
	text := in.Text
	if text == "" {
		text = in.Message
	}
	if text == "" {
		return &ToolResult{Content: "Error: 'text' (or 'message') is required", IsError: true}, nil
	}

	// Append to the companion chat session as an assistant message
	if t.appendToSession != nil {
		if err := t.appendToSession(text); err != nil {
			fmt.Printf("[message.owner] Failed to append to session: %v\n", err)
		}
	}

	// Fire a notification event to the web UI
	if t.sendFrame != nil {
		if err := t.sendFrame(map[string]any{
			"type":   "event",
			"method": "notification",
			"payload": map[string]any{
				"content": text,
			},
		}); err != nil {
			fmt.Printf("[message.owner] Failed to send frame: %v\n", err)
		}
	}

	return &ToolResult{Content: "Notification sent to owner"}, nil
}
