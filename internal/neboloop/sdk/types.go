package sdk

import "encoding/json"

// Config holds connection parameters for the NeboLoop gateway.
type Config struct {
	Gateway  string // WebSocket URL, e.g. "wss://comms.neboloop.com"
	BotID    string // Bot UUID
	APIKey   string // Bot API key
	DeviceID string // Optional device/session identifier
}

// --- Wire payload types (JSON-encoded frame payloads) ---

// connectPayload is the CONNECT frame payload.
type connectPayload struct {
	BotID    string `json:"bot_id"`
	APIKey   string `json:"api_key"`
	DeviceID string `json:"device_id,omitempty"`
}

// authOKPayload is the AUTH_OK frame payload.
type authOKPayload struct {
	OK        bool   `json:"ok"`
	BotID     string `json:"bot_id,omitempty"`
	SessionID string `json:"session_id,omitempty"`
}

// authFailPayload is the AUTH_FAIL frame payload.
type authFailPayload struct {
	Reason string `json:"reason"`
	Code   uint32 `json:"code"`
}

// deliveryPayload is the MESSAGE_DELIVERY frame payload.
type deliveryPayload struct {
	SenderID    string          `json:"sender_id"`
	ContentType string          `json:"content_type"`
	Content     json.RawMessage `json:"content"`
	Stream      string          `json:"stream,omitempty"`
}

// sendPayload is the SEND_MESSAGE frame payload.
type sendPayload struct {
	ContentType   string          `json:"content_type"`
	Content       json.RawMessage `json:"content"`
	Stream        string          `json:"stream,omitempty"`
	CorrelationID string          `json:"correlation_id,omitempty"`
}

// joinPayload is the JOIN_CONVERSATION frame payload.
type joinPayload struct {
	LastAckedSeqs map[string]uint64 `json:"last_acked_seqs,omitempty"`
}

// leavePayload is the LEAVE_CONVERSATION frame payload (empty).
type leavePayload struct{}

// ackPayload is the ACK frame payload.
type ackPayload struct {
	ConversationID string `json:"conversation_id,omitempty"`
	AckedSeq       uint64 `json:"acked_seq"`
}

// presencePayload is the PRESENCE frame payload.
type presencePayload struct {
	Status string `json:"status"`
}

// resumeSummaryPayload is the RESUME_SUMMARY frame payload.
type resumeSummaryPayload struct {
	Gaps []conversationGap `json:"gaps"`
}

// conversationGap describes a gap in message delivery for a conversation.
type conversationGap struct {
	ConversationID string `json:"conversation_id"`
	LastSeq        uint64 `json:"last_seq"`
	CurrentSeq     uint64 `json:"current_seq"`
	GapSize        uint32 `json:"gap_size"`
}

// Ack is a delivery confirmation with conversation-scoped sequence number.
type Ack struct {
	ConversationID string
	Seq            uint64
}

// InstallEvent is delivered when an app is installed, updated, or removed.
type InstallEvent struct {
	Type        string `json:"type"`                  // "installed", "updated", "uninstalled", "revoked"
	AppID       string `json:"app_id"`
	Version     string `json:"version,omitempty"`
	DownloadURL string `json:"download_url,omitempty"`
}

// ChannelMessage is a message from an external channel (Telegram, Discord, etc.).
type ChannelMessage struct {
	ChannelType  string              `json:"channel_type"`
	SenderName   string              `json:"sender_name"`
	Text         string              `json:"text"`
	Attachments  []ChannelAttachment `json:"attachments,omitempty"`
	ReplyTo      string              `json:"reply_to,omitempty"`
	PlatformData json.RawMessage     `json:"platform_data,omitempty"`
	// Set by SDK on receive (not serialized to wire):
	ConversationID string `json:"-"`
	MessageID      string `json:"-"`
}

// ChannelAttachment is a file/image attached to a channel message.
type ChannelAttachment struct {
	Type     string `json:"type"` // "image", "file", "audio", "video"
	URL      string `json:"url"`
	Name     string `json:"name,omitempty"`
	MimeType string `json:"mime_type,omitempty"`
	Size     int64  `json:"size,omitempty"`
}

// TaskSubmission is an A2A task request from another bot.
type TaskSubmission struct {
	From          string `json:"from"`
	Input         string `json:"input"`
	CorrelationID string `json:"correlation_id"`
}

// TaskResult is an A2A task completion from another bot.
type TaskResult struct {
	CorrelationID string `json:"correlation_id"`
	Status        string `json:"status"` // "completed", "failed", "canceled", "working", "input-required"
	Output        string `json:"output,omitempty"`
	Error         string `json:"error,omitempty"`
}

// DirectMessage is an A2A direct message from another bot.
type DirectMessage struct {
	From    string `json:"from"`
	Type    string `json:"type"`    // "message", "mention", "proposal", "command", "info"
	Content string `json:"content"`
}

// AgentCard is the bot's identity published via REST API.
type AgentCard struct {
	Name         string       `json:"name"`
	Description  string       `json:"description,omitempty"`
	Skills       []AgentSkill `json:"skills,omitempty"`
	Capabilities []string     `json:"capabilities,omitempty"`
}

// AgentSkill describes a capability for A2A discovery.
type AgentSkill struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Description string   `json:"description,omitempty"`
	Tags        []string `json:"tags,omitempty"`
}
