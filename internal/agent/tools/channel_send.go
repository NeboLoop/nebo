package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// ChannelSender is the interface for sending messages to channel apps.
// Implemented by apps.AppRegistry — defined here to avoid import cycles.
type ChannelSender interface {
	SendToChannel(ctx context.Context, channelType, channelID, text string) error
	ListChannels() []string
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
			Content: "No channels configured. Install channel apps first.",
			IsError: false,
		}, nil
	}

	ids := t.sender.ListChannels()
	if len(ids) == 0 {
		return &ToolResult{
			Content: "No channels available. Install channel apps from the app store.",
			IsError: false,
		}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Available Channels (%d):\n", len(ids)))
	for _, id := range ids {
		sb.WriteString(fmt.Sprintf("- %s\n", id))
	}
	sb.WriteString("\nTo send a message, use action='send' with channel_id='type:chat_id'.")

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

	// Parse "type:identifier" format (e.g., "telegram:123456")
	parts := strings.SplitN(channelID, ":", 2)
	if len(parts) != 2 {
		return &ToolResult{
			Content: "Error: channel_id must be 'type:identifier' (e.g., 'telegram:123456')",
			IsError: true,
		}, nil
	}

	if err := t.sender.SendToChannel(ctx, parts[0], parts[1], text); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to send message: %v", err),
			IsError: true,
		}, nil
	}

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
