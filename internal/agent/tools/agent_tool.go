package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/embeddings"
	"github.com/nebolabs/nebo/internal/agent/orchestrator"
	"github.com/nebolabs/nebo/internal/agent/recovery"
	"github.com/nebolabs/nebo/internal/agent/session"
	"github.com/nebolabs/nebo/internal/channels"
)

// CommService is the interface for inter-agent communication.
// Implemented by comm.CommHandler — defined here to avoid import cycles
// (tools → comm → runner → tools).
type CommService interface {
	// Send sends a message through the active comm plugin
	Send(ctx context.Context, to, topic, content string, msgType string) error
	// Subscribe subscribes to a comm topic
	Subscribe(ctx context.Context, topic string) error
	// Unsubscribe unsubscribes from a comm topic
	Unsubscribe(ctx context.Context, topic string) error
	// ListTopics returns currently subscribed topics
	ListTopics() []string
	// PluginName returns the active plugin name
	PluginName() string
	// IsConnected returns whether the active plugin is connected
	IsConnected() bool
	// CommAgentID returns this agent's ID in the comm network
	CommAgentID() string
}

// AgentDomainTool consolidates agent-related tools into a single domain tool
// following the STRAP (Single Tool Resource Action Pattern).
//
// Resources:
//   - task: Spawn and manage sub-agents for parallel work
//   - cron: Schedule recurring tasks with cron expressions
//   - memory: Persistent fact storage across sessions (3-tier system)
//   - message: Send messages to connected channels (Telegram, Discord, Slack)
//   - session: Query and manage conversation sessions
//   - comm: Inter-agent communication via comm lane plugins
type AgentDomainTool struct {
	// Task/orchestration
	orchestrator *orchestrator.Orchestrator

	// Cron scheduling
	cron *CronTool

	// Memory storage
	memory *MemoryTool

	// Message sending
	channelMgr *channels.Manager

	// Inter-agent communication
	commService CommService

	// Session management
	sessions      *session.Manager
	currentUserID string
}

// AgentDomainInput defines the input for the agent domain tool
type AgentDomainInput struct {
	// Required fields
	Resource string `json:"resource"` // task, cron, memory, message, session
	Action   string `json:"action"`   // varies by resource

	// Task fields
	Description string `json:"description,omitempty"` // Short task description
	Prompt      string `json:"prompt,omitempty"`      // Detailed task prompt
	Wait        *bool  `json:"wait,omitempty"`        // Wait for completion (default: true)
	Timeout     int    `json:"timeout,omitempty"`     // Timeout in seconds (default: 300)
	AgentType   string `json:"agent_type,omitempty"`  // explore, plan, general
	AgentID     string `json:"agent_id,omitempty"`    // For status/cancel operations

	// Cron fields
	Name     string `json:"name,omitempty"`      // Job name
	Schedule string `json:"schedule,omitempty"`  // Cron expression
	Command  string `json:"command,omitempty"`   // Shell command (for bash tasks)
	TaskType string `json:"task_type,omitempty"` // bash or agent
	Message  string `json:"message,omitempty"`   // Agent prompt (for agent tasks)
	Deliver  *struct {
		Channel string `json:"channel"` // telegram, discord, slack
		To      string `json:"to"`      // chat/channel ID
	} `json:"deliver,omitempty"` // Where to send result

	// Memory fields
	Key       string            `json:"key,omitempty"`       // Memory key
	Value     string            `json:"value,omitempty"`     // Memory value
	Tags      []string          `json:"tags,omitempty"`      // Tags for categorization
	Query     string            `json:"query,omitempty"`     // Search query
	Namespace string            `json:"namespace,omitempty"` // Memory namespace
	Layer     string            `json:"layer,omitempty"`     // tacit, daily, entity
	Metadata  map[string]string `json:"metadata,omitempty"`  // Additional metadata

	// Message fields
	Channel  string `json:"channel,omitempty"`   // Channel type: telegram, discord, slack
	To       string `json:"to,omitempty"`        // Destination chat/channel ID
	Text     string `json:"text,omitempty"`      // Message text
	ReplyTo  string `json:"reply_to,omitempty"`  // Message ID to reply to
	ThreadID string `json:"thread_id,omitempty"` // Thread ID for threaded messages

	// Session fields
	SessionKey string `json:"session_key,omitempty"` // Session key
	Limit      int    `json:"limit,omitempty"`       // Max messages to return

	// Comm fields (reuses To, Topic, Text from message fields)
	MsgType string `json:"msg_type,omitempty"` // message, mention, proposal, command, info
	Topic   string `json:"topic,omitempty"`    // Comm topic/channel name
}

// AgentDomainConfig configures the agent domain tool
type AgentDomainConfig struct {
	DB           *sql.DB              // Shared database connection
	Sessions     *session.Manager     // Session manager
	ChannelMgr   *channels.Manager    // Channel manager (optional)
	Embedder     *embeddings.Service  // Embedding service (optional)
}

// NewAgentDomainTool creates a new agent domain tool
func NewAgentDomainTool(cfg AgentDomainConfig) (*AgentDomainTool, error) {
	tool := &AgentDomainTool{
		sessions:   cfg.Sessions,
		channelMgr: cfg.ChannelMgr,
	}

	// Initialize memory tool if DB is provided
	if cfg.DB != nil {
		memTool, err := NewMemoryTool(MemoryConfig{
			DB:       cfg.DB,
			Embedder: cfg.Embedder,
		})
		if err != nil {
			return nil, fmt.Errorf("failed to create memory tool: %w", err)
		}
		tool.memory = memTool

		// Initialize cron tool
		cronTool, err := NewCronTool(CronConfig{DB: cfg.DB})
		if err != nil {
			return nil, fmt.Errorf("failed to create cron tool: %w", err)
		}
		tool.cron = cronTool
	}

	return tool, nil
}

// SetOrchestrator sets the orchestrator for sub-agent spawning
func (t *AgentDomainTool) SetOrchestrator(orch *orchestrator.Orchestrator) {
	t.orchestrator = orch
}

// CreateOrchestrator creates and sets a new orchestrator
func (t *AgentDomainTool) CreateOrchestrator(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, registry *Registry) {
	adapter := &registryAdapter{registry: registry}
	t.orchestrator = orchestrator.NewOrchestrator(cfg, sessions, providers, adapter)
}

// GetOrchestrator returns the orchestrator for sharing with other tools
func (t *AgentDomainTool) GetOrchestrator() *orchestrator.Orchestrator {
	return t.orchestrator
}

// SetRecoveryManager sets the recovery manager for subagent persistence
func (t *AgentDomainTool) SetRecoveryManager(mgr *recovery.Manager) {
	if t.orchestrator != nil {
		t.orchestrator.SetRecoveryManager(mgr)
	}
}

// RecoverSubagents restores pending subagent tasks from the database
func (t *AgentDomainTool) RecoverSubagents(ctx context.Context) (int, error) {
	if t.orchestrator == nil {
		return 0, nil
	}
	return t.orchestrator.RecoverAgents(ctx)
}

// SetCommService sets the comm service for inter-agent communication
func (t *AgentDomainTool) SetCommService(svc CommService) {
	t.commService = svc
}

// SetChannels sets the channel manager for messaging
func (t *AgentDomainTool) SetChannels(mgr *channels.Manager) {
	t.channelMgr = mgr
}

// SetAgentCallback sets the callback for agent task execution in cron
func (t *AgentDomainTool) SetAgentCallback(cb AgentTaskCallback) {
	if t.cron != nil {
		t.cron.SetAgentCallback(cb)
	}
}

// SetCurrentUser sets the user ID for user-scoped operations
func (t *AgentDomainTool) SetCurrentUser(userID string) {
	t.currentUserID = userID
	if t.memory != nil {
		t.memory.SetCurrentUser(userID)
	}
}

// GetCurrentUser returns the current user ID
func (t *AgentDomainTool) GetCurrentUser() string {
	return t.currentUserID
}

// Close cleans up resources
func (t *AgentDomainTool) Close() error {
	if t.cron != nil {
		t.cron.Close()
	}
	if t.memory != nil {
		t.memory.Close()
	}
	return nil
}

// Name returns the tool name
func (t *AgentDomainTool) Name() string {
	return "agent"
}

// Domain returns the domain name
func (t *AgentDomainTool) Domain() string {
	return "agent"
}

// Resources returns available resources in this domain
func (t *AgentDomainTool) Resources() []string {
	return []string{"task", "cron", "memory", "message", "session", "comm"}
}

// ActionsFor returns available actions for a given resource
func (t *AgentDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "task":
		return []string{"spawn", "status", "cancel", "list"}
	case "cron":
		return []string{"create", "list", "delete", "pause", "resume", "run", "history"}
	case "memory":
		return []string{"store", "recall", "search", "list", "delete", "clear"}
	case "message":
		return []string{"send", "list"}
	case "session":
		return []string{"list", "history", "status", "clear"}
	case "comm":
		return []string{"send", "subscribe", "unsubscribe", "list_topics", "status"}
	default:
		return nil
	}
}

var agentResources = map[string]ResourceConfig{
	"task":    {Name: "task", Actions: []string{"spawn", "status", "cancel", "list"}, Description: "Sub-agent management"},
	"cron":    {Name: "cron", Actions: []string{"create", "list", "delete", "pause", "resume", "run", "history"}, Description: "Scheduled tasks"},
	"memory":  {Name: "memory", Actions: []string{"store", "recall", "search", "list", "delete", "clear"}, Description: "Persistent storage"},
	"message": {Name: "message", Actions: []string{"send", "list"}, Description: "Channel messaging"},
	"session": {Name: "session", Actions: []string{"list", "history", "status", "clear"}, Description: "Conversation sessions"},
	"comm":    {Name: "comm", Actions: []string{"send", "subscribe", "unsubscribe", "list_topics", "status"}, Description: "Inter-agent communication"},
}

// Description returns the tool description
func (t *AgentDomainTool) Description() string {
	return BuildDomainDescription(DomainSchemaConfig{
		Domain: "agent",
		Description: `Agent orchestration and state management.

Resources:
- task: Spawn sub-agents for parallel work (spawn, status, cancel, list)
- cron: Schedule recurring tasks with cron expressions
- memory: Three-tier persistent storage (tacit/daily/entity layers)
- message: Send messages to Telegram, Discord, Slack
- session: Manage conversation sessions
- comm: Inter-agent communication via comm lane (send, subscribe, unsubscribe, list_topics, status)`,
		Resources: agentResources,
		Examples: []string{
			`agent(resource: task, action: spawn, prompt: "Find all Go files with errors", agent_type: "explore")`,
			`agent(resource: cron, action: create, name: "daily-backup", schedule: "0 0 2 * * *", command: "backup.sh")`,
			`agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit")`,
			`agent(resource: memory, action: search, query: "preferences", layer: "tacit")`,
			`agent(resource: message, action: send, channel: "telegram", to: "123456", text: "Task complete!")`,
			`agent(resource: session, action: list)`,
			`agent(resource: comm, action: send, to: "dev-bot", topic: "project-alpha", text: "Review this PR")`,
			`agent(resource: comm, action: subscribe, topic: "announcements")`,
			`agent(resource: comm, action: status)`,
		},
	})
}

// Schema returns the JSON schema for the tool
func (t *AgentDomainTool) Schema() json.RawMessage {
	return BuildDomainSchema(DomainSchemaConfig{
		Domain:      "agent",
		Description: t.Description(),
		Resources:   agentResources,
		Fields: []FieldConfig{
			// Task fields
			{Name: "description", Type: "string", Description: "Short task description (3-5 words)", RequiredFor: []string{"spawn"}},
			{Name: "prompt", Type: "string", Description: "Detailed task prompt for sub-agent", RequiredFor: []string{"spawn"}},
			{Name: "wait", Type: "boolean", Description: "Wait for task completion (default: true)", Default: true},
			{Name: "timeout", Type: "integer", Description: "Timeout in seconds (default: 300)", Default: 300},
			{Name: "agent_type", Type: "string", Description: "Agent type: explore, plan, general", Enum: []string{"explore", "plan", "general"}},
			{Name: "agent_id", Type: "string", Description: "Agent ID for status/cancel operations"},

			// Cron fields
			{Name: "name", Type: "string", Description: "Unique job name/identifier"},
			{Name: "schedule", Type: "string", Description: "Cron expression: 'second minute hour day-of-month month day-of-week'"},
			{Name: "command", Type: "string", Description: "Shell command (for bash tasks)"},
			{Name: "task_type", Type: "string", Description: "Task type: bash or agent", Enum: []string{"bash", "agent"}},

			// Memory fields
			{Name: "key", Type: "string", Description: "Memory key (path-like: 'user/name', 'project/nebo')"},
			{Name: "value", Type: "string", Description: "Value to store"},
			{Name: "tags", Type: "array", Description: "Tags for categorization"},
			{Name: "query", Type: "string", Description: "Search query for memory search"},
			{Name: "namespace", Type: "string", Description: "Namespace for organization (default: 'default')"},
			{Name: "layer", Type: "string", Description: "Memory layer: tacit (long-term), daily (day-specific), entity (people/places/things)", Enum: []string{"tacit", "daily", "entity"}},

			// Message fields
			{Name: "channel", Type: "string", Description: "Channel type: telegram, discord, slack"},
			{Name: "to", Type: "string", Description: "Destination chat/channel ID"},
			{Name: "text", Type: "string", Description: "Message text to send"},
			{Name: "reply_to", Type: "string", Description: "Message ID to reply to"},
			{Name: "thread_id", Type: "string", Description: "Thread ID for threaded messages"},

			// Session fields
			{Name: "session_key", Type: "string", Description: "Session key identifier"},
			{Name: "limit", Type: "integer", Description: "Max messages to return (default: 20)"},

			// Comm fields
			{Name: "topic", Type: "string", Description: "Comm topic/channel name for subscribe/send"},
			{Name: "msg_type", Type: "string", Description: "Comm message type", Enum: []string{"message", "mention", "proposal", "command", "info"}},
		},
	})
}

// RequiresApproval returns true for dangerous operations
func (t *AgentDomainTool) RequiresApproval() bool {
	// Handled per-action in Execute
	return false
}

// Execute runs the agent domain tool
func (t *AgentDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in AgentDomainInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to parse input: %v", err),
			IsError: true,
		}, nil
	}

	// Validate resource and action
	if err := ValidateResourceAction(in.Resource, in.Action, agentResources); err != nil {
		return &ToolResult{
			Content: err.Error(),
			IsError: true,
		}, nil
	}

	// Route to appropriate handler
	switch in.Resource {
	case "task":
		return t.handleTask(ctx, in)
	case "cron":
		return t.handleCron(ctx, in)
	case "memory":
		return t.handleMemory(ctx, in)
	case "message":
		return t.handleMessage(ctx, in)
	case "session":
		return t.handleSession(ctx, in)
	case "comm":
		return t.handleComm(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown resource: %s", in.Resource),
			IsError: true,
		}, nil
	}
}

// =============================================================================
// Task handlers (sub-agent management)
// =============================================================================

func (t *AgentDomainTool) handleTask(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.orchestrator == nil {
		return &ToolResult{
			Content: "Error: Task orchestrator not configured",
			IsError: true,
		}, nil
	}

	switch in.Action {
	case "spawn":
		return t.taskSpawn(ctx, in)
	case "status":
		return t.taskStatus(in)
	case "cancel":
		return t.taskCancel(in)
	case "list":
		return t.taskList()
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown task action: %s", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *AgentDomainTool) taskSpawn(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.Prompt == "" {
		return &ToolResult{
			Content: "Error: 'prompt' is required for spawn action",
			IsError: true,
		}, nil
	}

	if in.Description == "" {
		in.Description = truncateForDescription(in.Prompt)
	}

	// Default wait to true
	wait := true
	if in.Wait != nil {
		wait = *in.Wait
	}

	// Default timeout to 5 minutes
	timeout := 300
	if in.Timeout > 0 {
		timeout = in.Timeout
	}

	// Build system prompt based on agent type
	systemPrompt := buildAgentSystemPrompt(in.AgentType, in.Prompt)

	// Spawn the sub-agent
	agent, err := t.orchestrator.Spawn(ctx, &orchestrator.SpawnRequest{
		Task:         in.Prompt,
		Description:  in.Description,
		Wait:         wait,
		Timeout:      time.Duration(timeout) * time.Second,
		SystemPrompt: systemPrompt,
	})

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to spawn sub-agent: %v", err),
			IsError: true,
		}, nil
	}

	if wait {
		var result strings.Builder
		result.WriteString(fmt.Sprintf("Sub-agent completed: %s\n", in.Description))
		result.WriteString(fmt.Sprintf("Status: %s\n", agent.Status))
		result.WriteString(fmt.Sprintf("Duration: %s\n\n", agent.CompletedAt.Sub(agent.StartedAt).Round(time.Second)))

		if agent.Error != nil {
			result.WriteString(fmt.Sprintf("Error: %v\n\n", agent.Error))
		}

		if agent.Result != "" {
			result.WriteString("Result:\n")
			result.WriteString(agent.Result)
		}

		return &ToolResult{
			Content: result.String(),
			IsError: agent.Status == orchestrator.StatusFailed,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Sub-agent spawned: %s\nAgent ID: %s\nDescription: %s\n\nThe agent is running in the background.",
			in.Description, agent.ID, in.Prompt),
		IsError: false,
	}, nil
}

func (t *AgentDomainTool) taskStatus(in AgentDomainInput) (*ToolResult, error) {
	if in.AgentID == "" {
		return &ToolResult{
			Content: "Error: 'agent_id' is required for status action",
			IsError: true,
		}, nil
	}

	agent, exists := t.orchestrator.GetAgent(in.AgentID)
	if !exists {
		return &ToolResult{
			Content: fmt.Sprintf("Agent not found: %s", in.AgentID),
			IsError: true,
		}, nil
	}

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Agent: %s\n", agent.ID))
	result.WriteString(fmt.Sprintf("Description: %s\n", agent.Description))
	result.WriteString(fmt.Sprintf("Status: %s\n", agent.Status))
	result.WriteString(fmt.Sprintf("Started: %s\n", agent.StartedAt.Format(time.RFC3339)))

	if !agent.CompletedAt.IsZero() {
		result.WriteString(fmt.Sprintf("Completed: %s\n", agent.CompletedAt.Format(time.RFC3339)))
		result.WriteString(fmt.Sprintf("Duration: %s\n", agent.CompletedAt.Sub(agent.StartedAt).Round(time.Second)))
	}

	if agent.Error != nil {
		result.WriteString(fmt.Sprintf("\nError: %v\n", agent.Error))
	}

	if agent.Result != "" {
		result.WriteString(fmt.Sprintf("\nResult:\n%s\n", agent.Result))
	}

	return &ToolResult{Content: result.String()}, nil
}

func (t *AgentDomainTool) taskCancel(in AgentDomainInput) (*ToolResult, error) {
	if in.AgentID == "" {
		return &ToolResult{
			Content: "Error: 'agent_id' is required for cancel action",
			IsError: true,
		}, nil
	}

	if err := t.orchestrator.CancelAgent(in.AgentID); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Agent %s cancelled", in.AgentID)}, nil
}

func (t *AgentDomainTool) taskList() (*ToolResult, error) {
	agents := t.orchestrator.ListAgents()
	if len(agents) == 0 {
		return &ToolResult{Content: "No sub-agents running"}, nil
	}

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Sub-agents (%d):\n\n", len(agents)))
	for _, agent := range agents {
		result.WriteString(fmt.Sprintf("ID: %s\n", agent.ID))
		result.WriteString(fmt.Sprintf("  Description: %s\n", agent.Description))
		result.WriteString(fmt.Sprintf("  Status: %s\n", agent.Status))
		result.WriteString(fmt.Sprintf("  Started: %s\n", agent.StartedAt.Format(time.RFC3339)))
		if !agent.CompletedAt.IsZero() {
			result.WriteString(fmt.Sprintf("  Completed: %s\n", agent.CompletedAt.Format(time.RFC3339)))
		}
		result.WriteString("\n")
	}

	return &ToolResult{Content: result.String()}, nil
}

// =============================================================================
// Cron handlers (scheduled tasks)
// =============================================================================

func (t *AgentDomainTool) handleCron(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.cron == nil {
		return &ToolResult{
			Content: "Error: Cron scheduler not configured",
			IsError: true,
		}, nil
	}

	// Convert domain input to cron input
	cronIn := cronInput{
		Action:   in.Action,
		Name:     in.Name,
		Schedule: in.Schedule,
		Command:  in.Command,
		TaskType: in.TaskType,
		Message:  in.Message,
	}

	if in.Deliver != nil {
		cronIn.Deliver = &struct {
			Channel string `json:"channel"`
			To      string `json:"to"`
		}{
			Channel: in.Deliver.Channel,
			To:      in.Deliver.To,
		}
	}

	// Marshal and pass to cron tool
	cronJSON, _ := json.Marshal(cronIn)
	return t.cron.Execute(ctx, cronJSON)
}

// =============================================================================
// Memory handlers (persistent storage)
// =============================================================================

func (t *AgentDomainTool) handleMemory(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.memory == nil {
		return &ToolResult{
			Content: "Error: Memory storage not configured",
			IsError: true,
		}, nil
	}

	// Convert domain input to memory input
	memIn := memoryInput{
		Action:    in.Action,
		Key:       in.Key,
		Value:     in.Value,
		Tags:      in.Tags,
		Query:     in.Query,
		Namespace: in.Namespace,
		Layer:     in.Layer,
		Metadata:  in.Metadata,
	}

	// Marshal and pass to memory tool
	memJSON, _ := json.Marshal(memIn)
	return t.memory.Execute(ctx, memJSON)
}

// =============================================================================
// Message handlers (channel messaging)
// =============================================================================

func (t *AgentDomainTool) handleMessage(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.channelMgr == nil {
		return &ToolResult{
			Content: "Error: No channels configured. Connect channels in the UI first.",
			IsError: true,
		}, nil
	}

	switch in.Action {
	case "list":
		return t.messageList()
	case "send":
		return t.messageSend(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown message action: %s", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *AgentDomainTool) messageList() (*ToolResult, error) {
	ids := t.channelMgr.List()
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

func (t *AgentDomainTool) messageSend(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
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

	ch, ok := t.channelMgr.Get(in.Channel)
	if !ok {
		ids := t.channelMgr.List()
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

// =============================================================================
// Session handlers (conversation management)
// =============================================================================

func (t *AgentDomainTool) handleSession(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.sessions == nil {
		return &ToolResult{
			Content: "Error: Session manager not configured",
			IsError: true,
		}, nil
	}

	switch in.Action {
	case "list":
		return t.sessionList()
	case "history":
		return t.sessionHistory(in)
	case "status":
		return t.sessionStatus(in)
	case "clear":
		return t.sessionClear(in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown session action: %s", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *AgentDomainTool) sessionList() (*ToolResult, error) {
	sessions, err := t.sessions.ListSessions(t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error listing sessions: %v", err),
			IsError: true,
		}, nil
	}

	if len(sessions) == 0 {
		return &ToolResult{Content: "No sessions found."}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Sessions (%d total):\n\n", len(sessions)))
	sb.WriteString("| Session Key | Created | Updated |\n")
	sb.WriteString("|------------|---------|----------|\n")

	for _, sess := range sessions {
		created := sess.CreatedAt.Format("2006-01-02 15:04")
		updated := sess.UpdatedAt.Format("2006-01-02 15:04")
		sb.WriteString(fmt.Sprintf("| %s | %s | %s |\n", sess.SessionKey, created, updated))
	}

	return &ToolResult{Content: sb.String()}, nil
}

func (t *AgentDomainTool) sessionHistory(in AgentDomainInput) (*ToolResult, error) {
	if in.SessionKey == "" {
		return &ToolResult{
			Content: "Error: 'session_key' is required for history action",
			IsError: true,
		}, nil
	}

	limit := in.Limit
	if limit <= 0 {
		limit = 20
	}
	if limit > 100 {
		limit = 100
	}

	sess, err := t.sessions.GetOrCreate(in.SessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting session: %v", err),
			IsError: true,
		}, nil
	}

	messages, err := t.sessions.GetMessages(sess.ID, limit)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting messages: %v", err),
			IsError: true,
		}, nil
	}

	if len(messages) == 0 {
		return &ToolResult{
			Content: fmt.Sprintf("No messages in session: %s", in.SessionKey),
		}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Session: %s (showing %d messages)\n", in.SessionKey, len(messages)))
	sb.WriteString(strings.Repeat("=", 60) + "\n\n")

	for i, msg := range messages {
		roleIcon := "👤"
		switch msg.Role {
		case "assistant":
			roleIcon = "🤖"
		case "system":
			roleIcon = "⚙️"
		case "tool":
			roleIcon = "🔧"
		}

		sb.WriteString(fmt.Sprintf("%d. %s [%s]\n", i+1, roleIcon, msg.Role))

		if msg.Content != "" {
			content := msg.Content
			if len(content) > 500 {
				content = content[:497] + "..."
			}
			sb.WriteString(fmt.Sprintf("   %s\n", strings.ReplaceAll(content, "\n", "\n   ")))
		}

		if len(msg.ToolCalls) > 0 {
			var calls []session.ToolCall
			json.Unmarshal(msg.ToolCalls, &calls)
			for _, tc := range calls {
				sb.WriteString(fmt.Sprintf("   → Tool: %s\n", tc.Name))
			}
		}

		if len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			json.Unmarshal(msg.ToolResults, &results)
			for _, tr := range results {
				status := "✓"
				if tr.IsError {
					status = "✗"
				}
				content := tr.Content
				if len(content) > 200 {
					content = content[:197] + "..."
				}
				sb.WriteString(fmt.Sprintf("   %s Result: %s\n", status, content))
			}
		}

		sb.WriteString("\n")
	}

	return &ToolResult{Content: sb.String()}, nil
}

func (t *AgentDomainTool) sessionStatus(in AgentDomainInput) (*ToolResult, error) {
	if in.SessionKey == "" {
		return &ToolResult{
			Content: "Error: 'session_key' is required for status action",
			IsError: true,
		}, nil
	}

	sess, err := t.sessions.GetOrCreate(in.SessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting session: %v", err),
			IsError: true,
		}, nil
	}

	messages, err := t.sessions.GetMessages(sess.ID, 1000)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting messages: %v", err),
			IsError: true,
		}, nil
	}

	// Count by role
	userCount := 0
	assistantCount := 0
	toolCount := 0
	totalToolCalls := 0

	for _, msg := range messages {
		switch msg.Role {
		case "user":
			userCount++
		case "assistant":
			assistantCount++
			if len(msg.ToolCalls) > 0 {
				var calls []session.ToolCall
				json.Unmarshal(msg.ToolCalls, &calls)
				totalToolCalls += len(calls)
			}
		case "tool":
			toolCount++
		}
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Session Status: %s\n", in.SessionKey))
	sb.WriteString(strings.Repeat("=", 40) + "\n\n")
	sb.WriteString(fmt.Sprintf("ID: %s\n", sess.ID))
	sb.WriteString(fmt.Sprintf("Created: %s\n", sess.CreatedAt.Format(time.RFC3339)))
	sb.WriteString(fmt.Sprintf("Updated: %s\n", sess.UpdatedAt.Format(time.RFC3339)))
	sb.WriteString(fmt.Sprintf("Age: %s\n", time.Since(sess.CreatedAt).Round(time.Minute)))
	sb.WriteString("\n")
	sb.WriteString(fmt.Sprintf("Total Messages: %d\n", len(messages)))
	sb.WriteString(fmt.Sprintf("  User: %d\n", userCount))
	sb.WriteString(fmt.Sprintf("  Assistant: %d\n", assistantCount))
	sb.WriteString(fmt.Sprintf("  Tool Results: %d\n", toolCount))
	sb.WriteString(fmt.Sprintf("Total Tool Calls: %d\n", totalToolCalls))

	return &ToolResult{Content: sb.String()}, nil
}

func (t *AgentDomainTool) sessionClear(in AgentDomainInput) (*ToolResult, error) {
	if in.SessionKey == "" {
		return &ToolResult{
			Content: "Error: 'session_key' is required for clear action",
			IsError: true,
		}, nil
	}

	sess, err := t.sessions.GetOrCreate(in.SessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting session: %v", err),
			IsError: true,
		}, nil
	}

	if err := t.sessions.Reset(sess.ID); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error clearing session: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Session cleared: %s", in.SessionKey)}, nil
}

// =============================================================================
// Comm handlers (inter-agent communication)
// =============================================================================

func (t *AgentDomainTool) handleComm(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.commService == nil {
		return &ToolResult{
			Content: "Error: Comm service not configured. Enable comm in config.yaml.",
			IsError: true,
		}, nil
	}

	switch in.Action {
	case "send":
		return t.commSend(ctx, in)
	case "subscribe":
		return t.commSubscribe(ctx, in)
	case "unsubscribe":
		return t.commUnsubscribe(ctx, in)
	case "list_topics":
		return t.commListTopics()
	case "status":
		return t.commStatus()
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown comm action: %s", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *AgentDomainTool) commSend(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.To == "" {
		return &ToolResult{
			Content: "Error: 'to' is required (target agent ID)",
			IsError: true,
		}, nil
	}
	if in.Topic == "" {
		return &ToolResult{
			Content: "Error: 'topic' is required",
			IsError: true,
		}, nil
	}
	text := in.Text
	if text == "" {
		text = in.Message
	}
	if text == "" {
		return &ToolResult{
			Content: "Error: 'text' is required (message content)",
			IsError: true,
		}, nil
	}

	msgType := in.MsgType
	if msgType == "" {
		msgType = "message"
	}

	if err := t.commService.Send(ctx, in.To, in.Topic, text, msgType); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error sending comm message: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Comm message sent to %s on topic %s", in.To, in.Topic),
	}, nil
}

func (t *AgentDomainTool) commSubscribe(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.Topic == "" {
		return &ToolResult{
			Content: "Error: 'topic' is required",
			IsError: true,
		}, nil
	}

	if err := t.commService.Subscribe(ctx, in.Topic); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error subscribing to topic: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Subscribed to comm topic: %s", in.Topic),
	}, nil
}

func (t *AgentDomainTool) commUnsubscribe(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.Topic == "" {
		return &ToolResult{
			Content: "Error: 'topic' is required",
			IsError: true,
		}, nil
	}

	if err := t.commService.Unsubscribe(ctx, in.Topic); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error unsubscribing from topic: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Unsubscribed from comm topic: %s", in.Topic),
	}, nil
}

func (t *AgentDomainTool) commListTopics() (*ToolResult, error) {
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

func (t *AgentDomainTool) commStatus() (*ToolResult, error) {
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

// =============================================================================
// Memory convenience methods for programmatic access
// =============================================================================

// StoreEntry stores a memory entry directly (for programmatic use)
func (t *AgentDomainTool) StoreEntry(layer, namespace, key, value string, tags []string) error {
	if t.memory == nil {
		return fmt.Errorf("memory storage not configured")
	}
	return t.memory.StoreEntry(layer, namespace, key, value, tags)
}

// StoreEntryForUser stores a memory entry for a specific user
func (t *AgentDomainTool) StoreEntryForUser(layer, namespace, key, value string, tags []string, userID string) error {
	if t.memory == nil {
		return fmt.Errorf("memory storage not configured")
	}
	return t.memory.StoreEntryForUser(layer, namespace, key, value, tags, userID)
}

// GetMemoryTool returns the underlying memory tool for direct access
func (t *AgentDomainTool) GetMemoryTool() *MemoryTool {
	return t.memory
}

// GetCronTool returns the underlying cron tool for direct access
func (t *AgentDomainTool) GetCronTool() *CronTool {
	return t.cron
}
