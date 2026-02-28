package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
)

// ChannelSender is the interface for sending messages to channel apps.
// Implemented by apps.AppRegistry — defined here to avoid import cycles.
type ChannelSender interface {
	SendToChannel(ctx context.Context, channelType, channelID, text string) error
	ListChannels() []string
}

// MessageTool allows the agent to send messages to connected channels
type MessageTool struct {
	mu     sync.RWMutex
	sender ChannelSender
}

// NewMessageTool creates a new message tool
func NewMessageTool() *MessageTool {
	return &MessageTool{}
}

// SetChannelSender sets the channel sender (called after channels are initialized)
func (t *MessageTool) SetChannelSender(sender ChannelSender) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.sender = sender
}

// Name returns the tool name
func (t *MessageTool) Name() string {
	return "message"
}

// Description returns the tool description
func (t *MessageTool) Description() string {
	return `Send messages proactively to connected channels.

Actions:
- "send": Send a message to a channel (requires channel, to, text)
- "list": List all connected channels

Channels are provided by installed apps. Use 'list' to see what's available.`
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
				"description": "Channel type (provided by installed apps — use 'list' to see available)"
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
	sender := t.sender
	t.mu.RUnlock()

	if sender == nil {
		return &ToolResult{
			Content: "Error: No channels configured. Install channel apps first.",
			IsError: true,
		}, nil
	}

	switch in.Action {
	case "list":
		ids := sender.ListChannels()
		if len(ids) == 0 {
			return &ToolResult{
				Content: "No channels connected. Install channel apps from the app store.",
			}, nil
		}
		result := "Connected channels:\n"
		for _, id := range ids {
			result += fmt.Sprintf("- %s\n", id)
		}
		return &ToolResult{Content: result}, nil

	case "send":
		if in.Channel == "" {
			return &ToolResult{Content: "Error: 'channel' is required", IsError: true}, nil
		}
		if in.To == "" {
			return &ToolResult{Content: "Error: 'to' is required", IsError: true}, nil
		}
		if in.Text == "" {
			return &ToolResult{Content: "Error: 'text' is required", IsError: true}, nil
		}
		if err := sender.SendToChannel(ctx, in.Channel, in.To, in.Text); err != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Error sending message: %v", err),
				IsError: true,
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Message sent to %s:%s", in.Channel, in.To),
		}, nil

	default:
		return &ToolResult{
			Content: fmt.Sprintf("Error: Unknown action '%s'. Use 'send' or 'list'.", in.Action),
			IsError: true,
		}, nil
	}
}

// RequiresApproval returns true - sending messages should be approved
func (t *MessageTool) RequiresApproval() bool {
	return true
}
