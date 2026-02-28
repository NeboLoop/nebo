package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// NotifyOwnerTool sends a notification to the owner's Main Chat from any lane.
// This allows loop channel skills, events, and background tasks to surface
// important information in the owner's primary conversation.
type NotifyOwnerTool struct {
	// AppendToSession appends a message to the companion chat session.
	AppendToSession func(content string) error
	// SendFrame sends a WebSocket frame to the agent hub for broadcast.
	SendFrame func(frame map[string]any) error
}

type notifyOwnerInput struct {
	Message string `json:"message"`
}

func (t *NotifyOwnerTool) Name() string        { return "notify_owner" }
func (t *NotifyOwnerTool) RequiresApproval() bool { return false }

func (t *NotifyOwnerTool) Description() string {
	return "Send a notification to the owner's Main Chat. Use this to surface important information from loop channels, background tasks, or events to the owner."
}

func (t *NotifyOwnerTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"message": {
				"type": "string",
				"description": "The notification message to send to the owner"
			}
		},
		"required": ["message"]
	}`)
}

func (t *NotifyOwnerTool) Execute(_ context.Context, input json.RawMessage) (*ToolResult, error) {
	var in notifyOwnerInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: "Invalid input: " + err.Error(), IsError: true}, nil
	}
	if in.Message == "" {
		return &ToolResult{Content: "Message is required", IsError: true}, nil
	}

	// Append to the companion chat session as an assistant message
	if t.AppendToSession != nil {
		if err := t.AppendToSession(in.Message); err != nil {
			fmt.Printf("[notify_owner] Failed to append to session: %v\n", err)
		}
	}

	// Fire a notification event to the web UI
	if t.SendFrame != nil {
		if err := t.SendFrame(map[string]any{
			"type":   "event",
			"method": "notification",
			"payload": map[string]any{
				"content": in.Message,
			},
		}); err != nil {
			fmt.Printf("[notify_owner] Failed to send frame: %v\n", err)
		}
	}

	return &ToolResult{Content: "Notification sent to owner"}, nil
}
