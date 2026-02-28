// Package comm provides inter-agent communication via a plugin-based transport layer.
// Messages flow through the comm lane and run the full agentic loop via Runner.Run().
package comm

import "context"

// CommMessageType represents the type of comm message
type CommMessageType string

const (
	CommTypeMessage    CommMessageType = "message"     // General message
	CommTypeMention    CommMessageType = "mention"     // Direct mention, needs response
	CommTypeProposal   CommMessageType = "proposal"    // Vote request
	CommTypeCommand    CommMessageType = "command"     // Direct command (still goes through LLM)
	CommTypeInfo       CommMessageType = "info"        // Informational, may not need response
	CommTypeTask           CommMessageType = "task"         // Incoming A2A task request
	CommTypeTaskResult     CommMessageType = "task_result"  // Completed A2A task result
	CommTypeTaskStatus     CommMessageType = "task_status"  // Intermediate status update (working, failed)
	CommTypeLoopChannel    CommMessageType = "loop_channel" // Loop channel message (bot-to-bot within a Loop)
)

// TaskStatus represents the lifecycle state of an A2A task.
// Values match the NeboLoop A2A spec: submitted → working → completed/failed/canceled/input-required
type TaskStatus string

const (
	TaskStatusSubmitted     TaskStatus = "submitted"
	TaskStatusWorking       TaskStatus = "working"
	TaskStatusCompleted     TaskStatus = "completed"
	TaskStatusFailed        TaskStatus = "failed"
	TaskStatusCanceled      TaskStatus = "canceled"       // NB: one 'l' per A2A spec
	TaskStatusInputRequired TaskStatus = "input-required" // Bot needs more info from requester
)

// ArtifactPart is one part of a task artifact (text or binary data).
type ArtifactPart struct {
	Type string `json:"type"`           // "text", "data"
	Text string `json:"text,omitempty"`
	Data []byte `json:"data,omitempty"`
}

// TaskArtifact is a structured result from a completed A2A task.
type TaskArtifact struct {
	Parts []ArtifactPart `json:"parts"`
}

// CommMessage represents a message in the comm layer
type CommMessage struct {
	ID             string            `json:"id"`
	From           string            `json:"from"`            // Agent ID or bot ID
	To             string            `json:"to"`              // Target agent (or "*" for broadcast)
	Topic          string            `json:"topic"`           // Discussion/channel name
	ConversationID string            `json:"conversation_id"` // Thread/conversation grouping
	Type           CommMessageType   `json:"type"`
	Content        string            `json:"content"`
	Metadata       map[string]string `json:"metadata,omitempty"`
	Timestamp      int64             `json:"timestamp"`
	HumanInjected  bool              `json:"human_injected,omitempty"` // Marks human-injected messages
	HumanID        string            `json:"human_id,omitempty"`      // Who injected (for audit)

	// A2A task lifecycle fields (only populated for task-type messages)
	TaskID        string         `json:"task_id,omitempty"`
	CorrelationID string         `json:"correlation_id,omitempty"`
	TaskStatus    TaskStatus     `json:"task_status,omitempty"`
	Artifacts     []TaskArtifact `json:"artifacts,omitempty"`
	Error         string         `json:"error,omitempty"` // Error description for failed tasks
}

// AgentCardSkill describes a skill for A2A Agent Card discovery.
// Follows the A2A Agent Card spec.
type AgentCardSkill struct {
	ID          string   `json:"id"`
	Name        string   `json:"name"`
	Description string   `json:"description"`
	Tags        []string `json:"tags,omitempty"`
}

// AgentCardProvider identifies the organization behind the bot.
type AgentCardProvider struct {
	Organization string `json:"organization"`
}

// AgentCard holds structured registration metadata for A2A discovery.
// Published as a retained MQTT message so other agents can discover capabilities.
// Follows the A2A Agent Card spec: https://github.com/a2aproject/a2a-spec
type AgentCard struct {
	Name               string            `json:"name"`
	Description        string            `json:"description,omitempty"`
	URL                string            `json:"url,omitempty"`
	PreferredTransport string            `json:"preferredTransport,omitempty"`
	ProtocolVersion    string            `json:"protocolVersion,omitempty"`
	DefaultInputModes  []string          `json:"defaultInputModes,omitempty"`
	DefaultOutputModes []string          `json:"defaultOutputModes,omitempty"`
	Capabilities       map[string]any    `json:"capabilities,omitempty"`
	Skills             []AgentCardSkill  `json:"skills,omitempty"`
	Provider           *AgentCardProvider `json:"provider,omitempty"`
}

// LoopChannelInfo describes a loop channel the bot is a member of.
type LoopChannelInfo struct {
	ChannelID   string `json:"channel_id"`
	ChannelName string `json:"channel_name"`
	LoopID      string `json:"loop_id"`
	LoopName    string `json:"loop_name"`
}

// LoopChannelLister is an optional interface for plugins that support loop channel discovery.
type LoopChannelLister interface {
	ListLoopChannels(ctx context.Context) ([]LoopChannelInfo, error)
}

// LoopInfo describes a loop the bot is a member of.
type LoopInfo struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
}

// LoopLister is an optional interface for plugins that can list loops.
type LoopLister interface {
	ListLoops(ctx context.Context) ([]LoopInfo, error)
}

// LoopGetter is an optional interface for plugins that can fetch a single loop by ID.
type LoopGetter interface {
	GetLoopInfo(ctx context.Context, loopID string) (*LoopInfo, error)
}

// ChannelMessageItem describes a single message from a loop channel.
type ChannelMessageItem struct {
	ID        string `json:"id"`
	From      string `json:"from"`
	Content   string `json:"content"`
	CreatedAt string `json:"created_at"`
}

// ChannelMessageLister is an optional interface for plugins that can fetch channel messages.
type ChannelMessageLister interface {
	ListChannelMessages(ctx context.Context, channelID string, limit int) ([]ChannelMessageItem, error)
}

// ChannelMemberLister is an optional interface for plugins that can fetch channel members.
type ChannelMemberLister interface {
	ListChannelMembers(ctx context.Context, channelID string) ([]ChannelMemberItem, error)
}

// ChannelMemberItem describes a bot that is a member of a channel.
type ChannelMemberItem struct {
	BotID    string `json:"bot_id"`
	BotName  string `json:"bot_name"`
	Role     string `json:"role,omitempty"`
	IsOnline bool   `json:"is_online"`
}

// ManagerStatus holds the status of the comm plugin manager
type ManagerStatus struct {
	PluginName string   `json:"plugin_name"`
	Connected  bool     `json:"connected"`
	Topics     []string `json:"topics"`
	AgentID    string   `json:"agent_id"`
}
