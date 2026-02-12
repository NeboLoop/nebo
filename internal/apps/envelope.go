package apps

import (
	"encoding/json"
	"time"

	"github.com/google/uuid"
)

// ChannelEnvelope is the v1 common message envelope used on the MQTT channel bridge.
// Both inbound (NeboLoop → Nebo) and outbound (Nebo → NeboLoop) messages use this format.
type ChannelEnvelope struct {
	MessageID    string          `json:"message_id"`              // UUID v7 (time-ordered)
	ChannelID    string          `json:"channel_id"`              // format: {type}:{platform_id}
	Sender       EnvelopeSender  `json:"sender"`                  // who sent this message
	Text         string          `json:"text"`                    // message body
	Attachments  []Attachment    `json:"attachments,omitempty"`   // files, images, etc.
	ReplyTo      string          `json:"reply_to,omitempty"`      // message_id for threading
	Actions      []Action        `json:"actions,omitempty"`       // buttons, keyboards
	PlatformData json.RawMessage `json:"platform_data,omitempty"` // opaque passthrough
	Timestamp    time.Time       `json:"timestamp"`               // set by publisher
}

// EnvelopeSender identifies the message author.
type EnvelopeSender struct {
	Name  string `json:"name"`
	Role  string `json:"role,omitempty"`
	BotID string `json:"bot_id,omitempty"`
}

// Attachment represents a file or media attachment.
type Attachment struct {
	Type     string `json:"type"`               // "image", "file", "audio", "video"
	URL      string `json:"url"`                // download URL
	Filename string `json:"filename,omitempty"` // original filename
	Size     int    `json:"size,omitempty"`     // bytes
}

// Action represents an interactive element (button, keyboard row).
type Action struct {
	Label      string `json:"label"`
	CallbackID string `json:"callback_id"`
}

// NewMessageID generates a UUID v7 (time-ordered) for message deduplication and ordering.
func NewMessageID() string {
	return uuid.Must(uuid.NewV7()).String()
}
