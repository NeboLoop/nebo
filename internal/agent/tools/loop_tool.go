package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// LoopTool provides NeboLoop communication capabilities:
// DMs to other bots, loop channel messaging, group queries, and topic subscriptions.
type LoopTool struct {
	commService       CommService
	loopChannelLister func(ctx context.Context) ([]LoopChannelInfo, error)
	loopQuerier       LoopQuerier
}

// NewLoopTool creates a new loop domain tool.
func NewLoopTool() *LoopTool {
	return &LoopTool{}
}

// SetCommService sets the comm service for inter-agent communication.
func (t *LoopTool) SetCommService(svc CommService) {
	t.commService = svc
}

// SetLoopChannelLister sets the function for listing loop channels.
func (t *LoopTool) SetLoopChannelLister(fn func(ctx context.Context) ([]LoopChannelInfo, error)) {
	t.loopChannelLister = fn
}

// SetLoopQuerier sets the loop query provider for loop/member/message lookups.
func (t *LoopTool) SetLoopQuerier(q LoopQuerier) {
	t.loopQuerier = q
}

func (t *LoopTool) Name() string   { return "loop" }
func (t *LoopTool) Domain() string { return "loop" }

func (t *LoopTool) Resources() []string {
	return []string{"dm", "channel", "group", "topic"}
}

func (t *LoopTool) ActionsFor(resource string) []string {
	switch resource {
	case "dm":
		return []string{"send"}
	case "channel":
		return []string{"send", "messages", "members", "list"}
	case "group":
		return []string{"list", "get", "members"}
	case "topic":
		return []string{"subscribe", "unsubscribe", "list", "status"}
	default:
		return nil
	}
}

func (t *LoopTool) RequiresApproval() bool { return false }

var loopResources = map[string]ResourceConfig{
	"dm":      {Name: "dm", Actions: []string{"send"}, Description: "Direct messages to other bots"},
	"channel": {Name: "channel", Actions: []string{"send", "messages", "members", "list"}, Description: "Loop channel messaging and queries"},
	"group":   {Name: "group", Actions: []string{"list", "get", "members"}, Description: "Loop group queries"},
	"topic":   {Name: "topic", Actions: []string{"subscribe", "unsubscribe", "list", "status"}, Description: "Comm topic subscriptions"},
}

func (t *LoopTool) Description() string {
	return BuildDomainDescription(DomainSchemaConfig{
		Domain: "loop",
		Description: `NeboLoop communication — bot-to-bot messaging, loop channels, and topic subscriptions.

Resources:
- dm: Direct messages to other bots (send)
- channel: Loop channel messaging (send, list, messages, members)
- group: Loop group queries (list, get, members)
- topic: Comm topic subscriptions (subscribe, unsubscribe, list, status)`,
		Resources: loopResources,
		Examples: []string{
			`loop(resource: dm, action: send, to: "dev-bot", topic: "project-alpha", text: "Review this PR")`,
			`loop(resource: channel, action: list)`,
			`loop(resource: channel, action: send, channel_id: "channel-uuid", text: "Hello from the loop!")`,
			`loop(resource: channel, action: messages, channel_id: "channel-uuid", limit: 50)`,
			`loop(resource: channel, action: members, channel_id: "channel-uuid")`,
			`loop(resource: group, action: list)`,
			`loop(resource: group, action: get, loop_id: "loop-uuid")`,
			`loop(resource: group, action: members, loop_id: "loop-uuid")`,
			`loop(resource: topic, action: subscribe, topic: "announcements")`,
			`loop(resource: topic, action: unsubscribe, topic: "announcements")`,
			`loop(resource: topic, action: list)`,
			`loop(resource: topic, action: status)`,
		},
	})
}

func (t *LoopTool) Schema() json.RawMessage {
	return BuildDomainSchema(DomainSchemaConfig{
		Domain:      "loop",
		Description: t.Description(),
		Resources:   loopResources,
		Fields: []FieldConfig{
			{Name: "to", Type: "string", Description: "Target agent ID (for dm.send)"},
			{Name: "topic", Type: "string", Description: "Comm topic name (for dm.send, topic.subscribe/unsubscribe)"},
			{Name: "text", Type: "string", Description: "Message text to send"},
			{Name: "msg_type", Type: "string", Description: "Message type", Enum: []string{"message", "mention", "proposal", "command", "info"}},
			{Name: "channel_id", Type: "string", Description: "Loop channel ID (for channel.send/messages/members)"},
			{Name: "loop_id", Type: "string", Description: "Loop ID (for group.get/members)"},
			{Name: "limit", Type: "integer", Description: "Max messages to return (default: 50)"},
		},
	})
}

// LoopInput defines the input for the loop domain tool.
type LoopInput struct {
	Resource  string `json:"resource"`
	Action    string `json:"action"`
	To        string `json:"to,omitempty"`
	Topic     string `json:"topic,omitempty"`
	Text      string `json:"text,omitempty"`
	MsgType   string `json:"msg_type,omitempty"`
	ChannelID string `json:"channel_id,omitempty"`
	LoopID    string `json:"loop_id,omitempty"`
	Limit     int    `json:"limit,omitempty"`
	Message   string `json:"message,omitempty"` // alias for text
}

// loopActionToResource maps actions unique to a resource for inference.
var loopActionToResource = map[string]string{
	"subscribe":   "topic",
	"unsubscribe": "topic",
	"messages":    "channel",
}

func (t *LoopTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in LoopInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Infer resource from action when omitted
	if in.Resource == "" {
		if r, ok := loopActionToResource[in.Action]; ok {
			in.Resource = r
		}
	}

	if err := ValidateResourceAction(in.Resource, in.Action, loopResources); err != nil {
		return &ToolResult{Content: err.Error(), IsError: true}, nil
	}

	if t.commService == nil {
		return &ToolResult{
			Content: "Error: Comm service not configured. Enable comm in config.yaml.",
			IsError: true,
		}, nil
	}

	switch in.Resource {
	case "dm":
		return t.handleDM(ctx, in)
	case "channel":
		return t.handleChannel(ctx, in)
	case "group":
		return t.handleGroup(ctx, in)
	case "topic":
		return t.handleTopic(ctx, in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown resource: %s", in.Resource), IsError: true}, nil
	}
}

// =============================================================================
// DM handlers
// =============================================================================

func (t *LoopTool) handleDM(ctx context.Context, in LoopInput) (*ToolResult, error) {
	if in.To == "" {
		return &ToolResult{Content: "Error: 'to' is required (target agent ID)", IsError: true}, nil
	}
	if in.Topic == "" {
		return &ToolResult{Content: "Error: 'topic' is required", IsError: true}, nil
	}
	text := in.Text
	if text == "" {
		text = in.Message
	}
	if text == "" {
		return &ToolResult{Content: "Error: 'text' is required (message content)", IsError: true}, nil
	}
	msgType := in.MsgType
	if msgType == "" {
		msgType = "message"
	}

	if err := t.commService.Send(ctx, in.To, in.Topic, text, msgType); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error sending DM: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("DM sent to %s on topic %s", in.To, in.Topic)}, nil
}

// =============================================================================
// Channel handlers
// =============================================================================

func (t *LoopTool) handleChannel(ctx context.Context, in LoopInput) (*ToolResult, error) {
	switch in.Action {
	case "send":
		return t.channelSend(ctx, in)
	case "list":
		return t.channelList(ctx)
	case "messages":
		return t.channelMessages(ctx, in)
	case "members":
		return t.channelMembers(ctx, in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown channel action: %s", in.Action), IsError: true}, nil
	}
}

func (t *LoopTool) channelSend(ctx context.Context, in LoopInput) (*ToolResult, error) {
	channelID := in.ChannelID
	if channelID == "" {
		channelID = in.To
	}
	if channelID == "" {
		return &ToolResult{Content: "Error: 'channel_id' is required (loop channel ID)", IsError: true}, nil
	}
	text := in.Text
	if text == "" {
		text = in.Message
	}
	if text == "" {
		return &ToolResult{Content: "Error: 'text' is required (message content)", IsError: true}, nil
	}

	if err := t.commService.Send(ctx, channelID, "", text, "loop_channel"); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error sending loop channel message: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Message sent to loop channel %s", channelID)}, nil
}

func (t *LoopTool) channelList(ctx context.Context) (*ToolResult, error) {
	if t.loopChannelLister == nil {
		return &ToolResult{Content: "Loop channel listing not available (not connected to NeboLoop)", IsError: true}, nil
	}
	channels, err := t.loopChannelLister(ctx)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error listing loop channels: %v", err), IsError: true}, nil
	}
	if len(channels) == 0 {
		return &ToolResult{Content: "No loop channels found."}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Loop channels (%d):\n", len(channels)))
	for _, ch := range channels {
		sb.WriteString(fmt.Sprintf("  - %s (channel: %s, loop: %s / %s)\n",
			ch.ChannelName, ch.ChannelID, ch.LoopName, ch.LoopID))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *LoopTool) channelMessages(ctx context.Context, in LoopInput) (*ToolResult, error) {
	channelID := in.ChannelID
	if channelID == "" {
		return &ToolResult{Content: "Error: 'channel_id' is required", IsError: true}, nil
	}
	limit := in.Limit
	if limit <= 0 {
		limit = 50
	}
	if limit > 200 {
		limit = 200
	}
	if t.loopQuerier == nil {
		return &ToolResult{Content: "Loop queries not available (not connected to NeboLoop)", IsError: true}, nil
	}
	messages, err := t.loopQuerier.ListChannelMessages(ctx, channelID, limit)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error fetching channel messages: %v", err), IsError: true}, nil
	}
	if len(messages) == 0 {
		return &ToolResult{Content: "No messages in this channel."}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Recent Messages (%d):\n\n", len(messages)))
	for i, msg := range messages {
		sb.WriteString(fmt.Sprintf("%d. [%s] %s:\n", i+1, msg.CreatedAt, msg.From))
		content := msg.Content
		if len(content) > 300 {
			content = content[:297] + "..."
		}
		sb.WriteString(fmt.Sprintf("   %s\n\n", strings.ReplaceAll(content, "\n", "\n   ")))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *LoopTool) channelMembers(ctx context.Context, in LoopInput) (*ToolResult, error) {
	channelID := in.ChannelID
	if channelID == "" {
		return &ToolResult{Content: "Error: 'channel_id' is required", IsError: true}, nil
	}
	if t.loopQuerier == nil {
		return &ToolResult{Content: "Loop queries not available (not connected to NeboLoop)", IsError: true}, nil
	}
	members, err := t.loopQuerier.ListChannelMembers(ctx, channelID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error listing channel members: %v", err), IsError: true}, nil
	}
	if len(members) == 0 {
		return &ToolResult{Content: "No members in this channel."}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Channel Members (%d):\n", len(members)))
	for _, m := range members {
		status := "offline"
		if m.IsOnline {
			status = "online"
		}
		role := m.Role
		if role == "" {
			role = "member"
		}
		sb.WriteString(fmt.Sprintf("  - %s (%s) [%s] role: %s\n", m.BotName, m.BotID, status, role))
	}
	return &ToolResult{Content: sb.String()}, nil
}

// =============================================================================
// Group handlers
// =============================================================================

func (t *LoopTool) handleGroup(ctx context.Context, in LoopInput) (*ToolResult, error) {
	switch in.Action {
	case "list":
		return t.groupList(ctx)
	case "get":
		return t.groupGet(ctx, in)
	case "members":
		return t.groupMembers(ctx, in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown group action: %s", in.Action), IsError: true}, nil
	}
}

func (t *LoopTool) groupList(ctx context.Context) (*ToolResult, error) {
	if t.loopQuerier == nil {
		return &ToolResult{Content: "Loop queries not available (not connected to NeboLoop)", IsError: true}, nil
	}
	loops, err := t.loopQuerier.ListLoops(ctx)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error listing loops: %v", err), IsError: true}, nil
	}
	if len(loops) == 0 {
		return &ToolResult{Content: "No loops found."}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Loops (%d):\n", len(loops)))
	for _, l := range loops {
		sb.WriteString(fmt.Sprintf("  - %s (ID: %s)", l.Name, l.ID))
		if l.MemberCount > 0 {
			sb.WriteString(fmt.Sprintf(" [%d members]", l.MemberCount))
		}
		sb.WriteString("\n")
		if l.Description != "" {
			sb.WriteString(fmt.Sprintf("    %s\n", l.Description))
		}
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *LoopTool) groupGet(ctx context.Context, in LoopInput) (*ToolResult, error) {
	loopID := in.LoopID
	if loopID == "" {
		return &ToolResult{Content: "Error: 'loop_id' is required", IsError: true}, nil
	}
	if t.loopQuerier == nil {
		return &ToolResult{Content: "Loop queries not available (not connected to NeboLoop)", IsError: true}, nil
	}
	loop, err := t.loopQuerier.GetLoop(ctx, loopID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error fetching loop: %v", err), IsError: true}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Loop: %s\n", loop.Name))
	sb.WriteString(fmt.Sprintf("ID: %s\n", loop.ID))
	if loop.Description != "" {
		sb.WriteString(fmt.Sprintf("Description: %s\n", loop.Description))
	}
	if loop.MemberCount > 0 {
		sb.WriteString(fmt.Sprintf("Members: %d\n", loop.MemberCount))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *LoopTool) groupMembers(ctx context.Context, in LoopInput) (*ToolResult, error) {
	loopID := in.LoopID
	if loopID == "" {
		return &ToolResult{Content: "Error: 'loop_id' is required", IsError: true}, nil
	}
	if t.loopQuerier == nil {
		return &ToolResult{Content: "Loop queries not available (not connected to NeboLoop)", IsError: true}, nil
	}
	members, err := t.loopQuerier.ListLoopMembers(ctx, loopID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error listing loop members: %v", err), IsError: true}, nil
	}
	if len(members) == 0 {
		return &ToolResult{Content: "No members in this loop."}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Loop Members (%d):\n", len(members)))
	for _, m := range members {
		status := "offline"
		if m.IsOnline {
			status = "online"
		}
		role := m.Role
		if role == "" {
			role = "member"
		}
		sb.WriteString(fmt.Sprintf("  - %s (%s) [%s] role: %s\n", m.BotName, m.BotID, status, role))
	}
	return &ToolResult{Content: sb.String()}, nil
}

// =============================================================================
// Topic handlers
// =============================================================================

func (t *LoopTool) handleTopic(ctx context.Context, in LoopInput) (*ToolResult, error) {
	switch in.Action {
	case "subscribe":
		return t.topicSubscribe(ctx, in)
	case "unsubscribe":
		return t.topicUnsubscribe(ctx, in)
	case "list":
		return t.topicList()
	case "status":
		return t.topicStatus()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown topic action: %s", in.Action), IsError: true}, nil
	}
}

func (t *LoopTool) topicSubscribe(ctx context.Context, in LoopInput) (*ToolResult, error) {
	if in.Topic == "" {
		return &ToolResult{Content: "Error: 'topic' is required", IsError: true}, nil
	}
	if err := t.commService.Subscribe(ctx, in.Topic); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error subscribing to topic: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Subscribed to comm topic: %s", in.Topic)}, nil
}

func (t *LoopTool) topicUnsubscribe(ctx context.Context, in LoopInput) (*ToolResult, error) {
	if in.Topic == "" {
		return &ToolResult{Content: "Error: 'topic' is required", IsError: true}, nil
	}
	if err := t.commService.Unsubscribe(ctx, in.Topic); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error unsubscribing from topic: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Unsubscribed from comm topic: %s", in.Topic)}, nil
}

func (t *LoopTool) topicList() (*ToolResult, error) {
	topics := t.commService.ListTopics()
	if len(topics) == 0 {
		return &ToolResult{Content: "No comm topics subscribed."}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Subscribed comm topics (%d):\n", len(topics)))
	for _, topic := range topics {
		sb.WriteString(fmt.Sprintf("- %s\n", topic))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *LoopTool) topicStatus() (*ToolResult, error) {
	topics := t.commService.ListTopics()

	var sb strings.Builder
	sb.WriteString("Comm Status:\n")
	sb.WriteString(fmt.Sprintf("  Plugin: %s\n", t.commService.PluginName()))
	sb.WriteString(fmt.Sprintf("  Connected: %v\n", t.commService.IsConnected()))
	sb.WriteString(fmt.Sprintf("  Agent ID: %s\n", t.commService.CommAgentID()))
	if len(topics) > 0 {
		sb.WriteString(fmt.Sprintf("  Topics (%d):\n", len(topics)))
		for _, topic := range topics {
			sb.WriteString(fmt.Sprintf("    - %s\n", topic))
		}
	} else {
		sb.WriteString("  Topics: none\n")
	}
	return &ToolResult{Content: sb.String()}, nil
}
