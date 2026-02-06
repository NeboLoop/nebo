package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/nebolabs/nebo/internal/channels"
)

// MessageTool allows the agent to send messages to connected channels
type MessageTool struct {
	mu       sync.RWMutex
	channels *channels.Manager
}

// NewMessageTool creates a new message tool
func NewMessageTool() *MessageTool {
	return &MessageTool{}
}

// SetChannels sets the channel manager (called after channels are initialized)
func (t *MessageTool) SetChannels(mgr *channels.Manager) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.channels = mgr
}

// Name returns the tool name
func (t *MessageTool) Name() string {
	return "message"
}

// Description returns the tool description
func (t *MessageTool) Description() string {
	return `Send messages proactively to connected messaging channels.

Actions:
- "send": Send a message to a channel (requires channel, to, text)
- "list": List all connected channels

Use this to send updates, reminders, or notifications to users on Telegram, Discord, Slack, etc.`
}

// Schema returns the JSON schema for the tool input
func (t *MessageTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["send", "list"],
				"description": "Action to perform"
			},
			"channel": {
				"type": "string",
				"description": "Channel type: telegram, discord, slack"
			},
			"to": {
				"type": "string",
				"description": "Destination: chat ID, channel ID, or user ID"
			},
			"text": {
				"type": "string",
				"description": "Message text to send"
			},
			"reply_to": {
				"type": "string",
				"description": "Optional message ID to reply to"
			},
			"thread_id": {
				"type": "string",
				"description": "Optional thread ID for threaded messages"
			}
		},
		"required": ["action"]
	}`)
}

// messageInput represents the tool input
type messageInput struct {
	Action   string `json:"action"`
	Channel  string `json:"channel"`
	To       string `json:"to"`
	Text     string `json:"text"`
	ReplyTo  string `json:"reply_to"`
	ThreadID string `json:"thread_id"`
}

// Execute runs the message tool
func (t *MessageTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in messageInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	t.mu.RLock()
	mgr := t.channels
	t.mu.RUnlock()

	if mgr == nil {
		return &ToolResult{
			Content: "Error: No channels configured. Connect channels in the UI first.",
			IsError: true,
		}, nil
	}

	switch in.Action {
	case "list":
		return t.listChannels(mgr)
	case "send":
		return t.sendMessage(ctx, mgr, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Error: Unknown action '%s'. Use 'send' or 'list'.", in.Action),
			IsError: true,
		}, nil
	}
}

// listChannels returns all connected channels
func (t *MessageTool) listChannels(mgr *channels.Manager) (*ToolResult, error) {
	ids := mgr.List()
	if len(ids) == 0 {
		return &ToolResult{
			Content: "No channels connected. Connect channels (Telegram, Discord, Slack) in the UI.",
		}, nil
	}

	result := "Connected channels:\n"
	for _, id := range ids {
		result += fmt.Sprintf("- %s\n", id)
	}
	return &ToolResult{Content: result}, nil
}

// sendMessage sends a message to a channel
func (t *MessageTool) sendMessage(ctx context.Context, mgr *channels.Manager, in messageInput) (*ToolResult, error) {
	if in.Channel == "" {
		return &ToolResult{
			Content: "Error: 'channel' is required (telegram, discord, slack)",
			IsError: true,
		}, nil
	}
	if in.To == "" {
		return &ToolResult{
			Content: "Error: 'to' is required (chat ID or channel ID)",
			IsError: true,
		}, nil
	}
	if in.Text == "" {
		return &ToolResult{
			Content: "Error: 'text' is required",
			IsError: true,
		}, nil
	}

	ch, ok := mgr.Get(in.Channel)
	if !ok {
		ids := mgr.List()
		return &ToolResult{
			Content: fmt.Sprintf("Error: Channel '%s' not found. Available: %v", in.Channel, ids),
			IsError: true,
		}, nil
	}

	msg := channels.OutboundMessage{
		ChannelID: in.To,
		Text:      in.Text,
		ReplyToID: in.ReplyTo,
		ThreadID:  in.ThreadID,
		ParseMode: "markdown",
	}

	if err := ch.Send(ctx, msg); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error sending message: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Message sent to %s:%s", in.Channel, in.To),
	}, nil
}

// RequiresApproval returns true - sending messages should be approved
func (t *MessageTool) RequiresApproval() bool {
	return true
}
