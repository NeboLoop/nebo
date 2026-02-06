package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// ChannelSender is an interface for sending messages to channels
type ChannelSender interface {
	Send(ctx context.Context, channelID, text string) error
	ListChannels() []ChannelInfo
}

// ChannelInfo describes an available channel
type ChannelInfo struct {
	ID       string `json:"id"`
	Type     string `json:"type"`     // "telegram", "discord", "slack", etc.
	Name     string `json:"name"`
	Status   string `json:"status"`   // "connected", "disconnected"
}

// ChannelSendTool allows AI to send messages to channels
type ChannelSendTool struct {
	sender ChannelSender
}

// ChannelSendInput defines the input for the channel send tool
type ChannelSendInput struct {
	Action    string `json:"action"`              // "send", "list"
	ChannelID string `json:"channel_id,omitempty"` // Channel to send to
	Text      string `json:"text,omitempty"`       // Message text
}

// NewChannelSendTool creates a new channel send tool
func NewChannelSendTool(sender ChannelSender) *ChannelSendTool {
	return &ChannelSendTool{
		sender: sender,
	}
}

// Name returns the tool name
func (t *ChannelSendTool) Name() string {
	return "message_send"
}

// Description returns the tool description
func (t *ChannelSendTool) Description() string {
	return "Send messages to communication channels (Telegram, Discord, Slack, etc.). Use 'list' action to see available channels."
}

// Schema returns the JSON schema
func (t *ChannelSendTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action to perform: 'send' (send message) or 'list' (list available channels)",
				"enum": ["send", "list"]
			},
			"channel_id": {
				"type": "string",
				"description": "Channel ID to send to (required for 'send' action). Format: 'type:identifier' (e.g., 'telegram:123456', 'discord:channel_id', 'slack:#general')"
			},
			"text": {
				"type": "string",
				"description": "Message text to send (required for 'send' action)"
			}
		},
		"required": ["action"]
	}`)
}

// RequiresApproval returns true - sending messages is sensitive
func (t *ChannelSendTool) RequiresApproval() bool {
	return true
}

// Execute performs the channel operation
func (t *ChannelSendTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params ChannelSendInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid input: %v", err),
			IsError: true,
		}, nil
	}

	switch params.Action {
	case "list":
		return t.listChannels()
	case "send":
		if params.ChannelID == "" {
			return &ToolResult{
				Content: "Error: 'channel_id' is required for send action",
				IsError: true,
			}, nil
		}
		if params.Text == "" {
			return &ToolResult{
				Content: "Error: 'text' is required for send action",
				IsError: true,
			}, nil
		}
		return t.sendMessage(ctx, params.ChannelID, params.Text)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s. Use 'send' or 'list'", params.Action),
			IsError: true,
		}, nil
	}
}

// listChannels returns available channels
func (t *ChannelSendTool) listChannels() (*ToolResult, error) {
	if t.sender == nil {
		return &ToolResult{
			Content: "No channels configured. Channel adapters must be set up first.",
			IsError: false,
		}, nil
	}

	channels := t.sender.ListChannels()
	if len(channels) == 0 {
		return &ToolResult{
			Content: "No channels available. Configure channels in the settings.",
			IsError: false,
		}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Available Channels (%d):\n\n", len(channels)))
	sb.WriteString("| ID | Type | Name | Status |\n")
	sb.WriteString("|-----|------|------|--------|\n")

	for _, ch := range channels {
		sb.WriteString(fmt.Sprintf("| %s | %s | %s | %s |\n",
			ch.ID, ch.Type, ch.Name, ch.Status))
	}

	sb.WriteString("\nTo send a message, use action='send' with the channel ID.")

	return &ToolResult{
		Content: sb.String(),
		IsError: false,
	}, nil
}

// sendMessage sends a message to a channel
func (t *ChannelSendTool) sendMessage(ctx context.Context, channelID, text string) (*ToolResult, error) {
	if t.sender == nil {
		return &ToolResult{
			Content: "Error: No channel sender configured",
			IsError: true,
		}, nil
	}

	if err := t.sender.Send(ctx, channelID, text); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to send message: %v", err),
			IsError: true,
		}, nil
	}

	// Truncate for display
	preview := text
	if len(preview) > 100 {
		preview = preview[:97] + "..."
	}

	return &ToolResult{
		Content: fmt.Sprintf("Message sent to %s:\n%s", channelID, preview),
		IsError: false,
	}, nil
}

// SetSender sets the channel sender (for late binding)
func (t *ChannelSendTool) SetSender(sender ChannelSender) {
	t.sender = sender
}
