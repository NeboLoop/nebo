package tools

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/google/uuid"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/orchestrator"
	"github.com/neboloop/nebo/internal/agent/recovery"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/provider"
)

// CommService is the interface for inter-agent communication.
// Implemented by comm.CommHandler — defined here to avoid import cycles
// (tools → comm → runner → tools).
// LoopChannelInfo describes a loop channel the bot is a member of.
type LoopChannelInfo struct {
	ChannelID   string `json:"channel_id"`
	ChannelName string `json:"channel_name"`
	LoopID      string `json:"loop_id"`
	LoopName    string `json:"loop_name"`
}

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

// LoopQuerier provides read access to loops, members, and channel history.
// Implemented by the NeboLoop comm plugin — defined here to avoid import cycles.
type LoopQuerier interface {
	ListLoops(ctx context.Context) ([]LoopInfo, error)
	GetLoop(ctx context.Context, loopID string) (*LoopInfo, error)
	ListLoopMembers(ctx context.Context, loopID string) ([]MemberInfo, error)
	ListChannelMembers(ctx context.Context, channelID string) ([]MemberInfo, error)
	ListChannelMessages(ctx context.Context, channelID string, limit int) ([]MessageInfo, error)
}

// LoopInfo describes a loop the bot belongs to.
type LoopInfo struct {
	ID          string `json:"id"`
	Name        string `json:"name"`
	Description string `json:"description,omitempty"`
	MemberCount int    `json:"member_count,omitempty"`
}

// MemberInfo describes a bot member with online presence.
type MemberInfo struct {
	BotID    string `json:"bot_id"`
	BotName  string `json:"bot_name,omitempty"`
	Role     string `json:"role,omitempty"`
	IsOnline bool   `json:"is_online"`
}

// MessageInfo describes a message from channel history.
type MessageInfo struct {
	ID        string `json:"id"`
	From      string `json:"from"`
	Content   string `json:"content"`
	CreatedAt string `json:"created_at"`
}

// AgentDomainTool consolidates agent-related tools into a single domain tool
// following the STRAP (Single Tool Resource Action Pattern).
//
// Resources:
//   - task: Spawn and manage sub-agents for parallel work
//   - reminder: Schedule recurring tasks (aliases: routine, schedule, job, cron, event)
//   - memory: Persistent fact storage across sessions (3-tier system)
//   - message: Send messages to connected channels (provided by installed apps)
//   - session: Query and manage conversation sessions
//   - comm: Inter-agent communication via comm lane plugins
// AskWidget defines an interactive widget for inline user prompts.
type AskWidget struct {
	Type    string   `json:"type"`              // "buttons", "select", "confirm", "radio", "checkbox"
	Label   string   `json:"label,omitempty"`
	Options []string `json:"options,omitempty"` // for buttons/select
	Default string   `json:"default,omitempty"` // pre-filled value
}

// AskCallback blocks until the user responds to an inline prompt.
// Mirrors ApprovalCallback (policy.go:30-32).
type AskCallback func(ctx context.Context, requestID string, prompt string, widgets []AskWidget) (string, error)

// WorkTask is an in-memory work tracking item created by the agent to track progress
// on its current objective. Ephemeral — does not survive restart.
type WorkTask struct {
	ID        string    `json:"id"`
	Subject   string    `json:"subject"`
	Status    string    `json:"status"` // pending, in_progress, completed
	CreatedAt time.Time `json:"created_at"`
}

type AgentDomainTool struct {
	// Task/orchestration
	orchestrator *orchestrator.Orchestrator

	// Scheduling (built-in CronTool or app-provided via SchedulerManager)
	scheduler Scheduler

	// Memory storage
	memory *MemoryTool

	// Message sending
	channelSender ChannelSender

	// Inter-agent communication
	commService       CommService
	loopChannelLister func(ctx context.Context) ([]LoopChannelInfo, error)
	loopQuerier       LoopQuerier

	// Session management
	sessions      *session.Manager
	currentUserID string

	// Identity sync (pushes name/role to NeboLoop)
	identitySyncer func(ctx context.Context, name, role string)

	// Interactive user prompts
	askCallback AskCallback

	// Work task tracking (in-memory, session-scoped)
	workTasks sync.Map // sessionKey → []WorkTask
}

// AgentDomainInput defines the input for the agent domain tool
type AgentDomainInput struct {
	// Required fields
	Resource string `json:"resource"` // task, reminder, memory, message, session
	Action   string `json:"action"`   // varies by resource

	// Task fields
	Description string `json:"description,omitempty"` // Short task description
	Prompt      string `json:"prompt,omitempty"`      // Detailed task prompt
	Wait        *bool  `json:"wait,omitempty"`        // Wait for completion (default: true)
	Timeout     int    `json:"timeout,omitempty"`     // Timeout in seconds (default: 300)
	AgentType   string `json:"agent_type,omitempty"`  // explore, plan, general
	AgentID     string `json:"agent_id,omitempty"`    // For status/cancel operations
	Subject     string `json:"subject,omitempty"`     // Work task subject (for create)
	TaskID      string `json:"task_id,omitempty"`     // Work task ID (for update)
	Status      string `json:"status,omitempty"`      // Work task status (for update)

	// Reminder (scheduling) fields
	Name     string `json:"name,omitempty"`      // Job name
	At       string `json:"at,omitempty"`        // Human-friendly time: "in 3 minutes", "7:30pm"
	Schedule string `json:"schedule,omitempty"`  // Cron expression (recurring only)
	Command  string `json:"command,omitempty"`   // Shell command (for bash tasks)
	TaskType string `json:"task_type,omitempty"` // bash or agent
	Message  string `json:"message,omitempty"`   // Agent prompt (for agent tasks)
	Deliver  *struct {
		Channel string `json:"channel"` // channel type (from installed apps)
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
	Channel  string      `json:"channel,omitempty"`   // Channel type (from installed apps)
	To       string      `json:"to,omitempty"`        // Destination chat/channel ID
	Text     string      `json:"text,omitempty"`      // Message text
	ReplyTo  string      `json:"reply_to,omitempty"`  // Message ID to reply to
	ThreadID string      `json:"thread_id,omitempty"` // Thread ID for threaded messages
	Widgets  []AskWidget `json:"widgets,omitempty"`   // Interactive widgets for ask action

	// Session fields
	SessionKey string `json:"session_key,omitempty"` // Session key
	Limit      int    `json:"limit,omitempty"`       // Max messages to return

	// Comm fields (reuses To, Topic, Text from message fields)
	MsgType   string `json:"msg_type,omitempty"`    // message, mention, proposal, command, info
	Topic     string `json:"topic,omitempty"`       // Comm topic/channel name
	ChannelID string `json:"channel_id,omitempty"`  // Loop channel ID
	LoopID    string `json:"loop_id,omitempty"`     // Loop ID (for loop queries)
}

// AgentDomainConfig configures the agent domain tool
type AgentDomainConfig struct {
	Sessions      *session.Manager // Session manager
	ChannelSender ChannelSender    // Channel sender (optional, set later via SetChannelSender)
	MemoryTool    *MemoryTool      // Shared memory tool instance (created externally)
	Scheduler     Scheduler         // Scheduler implementation (SchedulerManager, CronScheduler, or nil)
}

// NewAgentDomainTool creates a new agent domain tool
func NewAgentDomainTool(cfg AgentDomainConfig) (*AgentDomainTool, error) {
	tool := &AgentDomainTool{
		sessions:      cfg.Sessions,
		channelSender: cfg.ChannelSender,
		memory:        cfg.MemoryTool,
		scheduler:     cfg.Scheduler,
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

// SetLoopChannelLister sets the function for listing loop channels.
// Injected from agent.go to avoid import cycles (tools → comm).
func (t *AgentDomainTool) SetLoopChannelLister(fn func(ctx context.Context) ([]LoopChannelInfo, error)) {
	t.loopChannelLister = fn
}

// SetLoopQuerier sets the loop query provider for loop/member/message lookups.
func (t *AgentDomainTool) SetLoopQuerier(q LoopQuerier) {
	t.loopQuerier = q
}

// SetIdentitySyncer sets the callback for syncing identity (name/role) to NeboLoop.
func (t *AgentDomainTool) SetIdentitySyncer(fn func(ctx context.Context, name, role string)) {
	t.identitySyncer = fn
}

// SetChannelSender sets the channel sender for messaging
func (t *AgentDomainTool) SetChannelSender(sender ChannelSender) {
	t.channelSender = sender
}

// SetAskCallback sets the callback for interactive user prompts.
func (t *AgentDomainTool) SetAskCallback(fn AskCallback) {
	t.askCallback = fn
}

// SetAgentCallback sets the callback for agent task execution in cron.
// Only works when the underlying scheduler is the built-in CronTool (via CronScheduler or SchedulerManager).
func (t *AgentDomainTool) SetAgentCallback(cb AgentTaskCallback) {
	// Try CronScheduler directly
	if cs, ok := t.scheduler.(*CronScheduler); ok {
		cs.cron.SetAgentCallback(cb)
		return
	}
	// Try SchedulerManager wrapping a CronScheduler
	if sm, ok := t.scheduler.(*SchedulerManager); ok {
		if cs, ok := sm.builtin.(*CronScheduler); ok {
			cs.cron.SetAgentCallback(cb)
		}
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
	if t.scheduler != nil {
		t.scheduler.Close()
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
	return []string{"task", "reminder", "memory", "message", "session", "comm", "profile"}
}

// ActionsFor returns available actions for a given resource
func (t *AgentDomainTool) ActionsFor(resource string) []string {
	switch resource {
	case "task":
		return []string{"spawn", "status", "cancel", "list", "create", "update", "delete"}
	case "reminder":
		return []string{"create", "list", "delete", "pause", "resume", "run", "history"}
	case "memory":
		return []string{"store", "recall", "search", "list", "delete", "clear"}
	case "message":
		return []string{"send", "list", "ask"}
	case "session":
		return []string{"list", "history", "status", "clear"}
	case "comm":
		return []string{"send", "subscribe", "unsubscribe", "list_topics", "status", "send_loop", "list_channels", "list_loops", "get_loop", "loop_members", "channel_members", "channel_messages"}
	case "profile":
		return []string{"get", "update", "open_billing"}
	default:
		return nil
	}
}

var agentResources = map[string]ResourceConfig{
	"task":    {Name: "task", Actions: []string{"spawn", "status", "cancel", "list", "create", "update", "delete"}, Description: "Sub-agent management + work tracking"},
	"reminder": {Name: "reminder", Actions: []string{"create", "list", "delete", "pause", "resume", "run", "history"}, Description: "Scheduled reminders and recurring tasks"},
	"memory":  {Name: "memory", Actions: []string{"store", "recall", "search", "list", "delete", "clear"}, Description: "Persistent storage"},
	"message": {Name: "message", Actions: []string{"send", "list", "ask"}, Description: "Channel messaging and interactive user prompts"},
	"session": {Name: "session", Actions: []string{"list", "history", "status", "clear"}, Description: "Conversation sessions"},
	"comm":    {Name: "comm", Actions: []string{"send", "subscribe", "unsubscribe", "list_topics", "status", "send_loop", "list_channels", "list_loops", "get_loop", "loop_members", "channel_members", "channel_messages"}, Description: "Inter-agent communication, loop channels, and loop queries"},
	"profile": {Name: "profile", Actions: []string{"get", "update", "open_billing"}, Description: "Read and update agent identity, or open NeboLoop billing page"},
}

// Description returns the tool description
func (t *AgentDomainTool) Description() string {
	return BuildDomainDescription(DomainSchemaConfig{
		Domain: "agent",
		Description: `Agent orchestration and state management.

Resources:
- task: Sub-agents (spawn, status, cancel) + work tracking (create, update, delete, list)
- reminder: Schedule reminders and recurring tasks (aliases: routine, schedule, job, cron, event, remind)
- memory: Three-tier persistent storage (tacit/daily/entity layers)
- message: Send messages to connected channels, or ask the user interactive questions inline (ask action)
- session: Manage conversation sessions
- comm: Inter-agent communication, loop channels, and loop queries
  - Direct messaging: send (to agent by ID, requires topic), subscribe, unsubscribe, list_topics, status
  - Loop channels: list_channels (discover channels), send_loop (send to a channel by channel_id)
  - Loop queries: list_loops, get_loop (by loop_id), loop_members (by loop_id), channel_members (by channel_id), channel_messages (by channel_id, optional limit)
- profile: Read and update your own identity (name, emoji, creature, vibe, personality)`,
		Resources: agentResources,
		Examples: []string{
			`agent(resource: task, action: spawn, prompt: "Find all Go files with errors", agent_type: "explore")`,
			`agent(resource: task, action: create, subject: "Read existing skill format")`,
			`agent(resource: task, action: update, task_id: "1", status: "completed")`,
			`agent(resource: task, action: delete, task_id: "1")`,
			`agent(resource: reminder, action: create, name: "morning-brief", schedule: "0 0 8 * * 1-5", task_type: "agent", message: "Check today's calendar and send me a summary")`,
			`agent(resource: reminder, action: create, name: "call-kristi", at: "in 10 minutes", task_type: "agent", message: "Remind user to call Kristi about haircuts")`,
			`agent(resource: memory, action: store, key: "user/name", value: "Alice", layer: "tacit")`,
			`agent(resource: memory, action: search, query: "preferences", layer: "tacit")`,
			`agent(resource: message, action: send, channel: "voice", to: "default", text: "Task complete!")`,
			`agent(resource: message, action: ask, prompt: "Which framework?", widgets: [{type: "buttons", options: ["React", "Svelte", "Vue"]}])`,
			`agent(resource: message, action: ask, prompt: "Pick any that sound useful:", widgets: [{type: "checkbox", options: ["Research Assistant", "Email Drafter", "Calendar Manager"]}])`,
			`agent(resource: session, action: list)`,
			`agent(resource: comm, action: send, to: "dev-bot", topic: "project-alpha", text: "Review this PR")`,
			`agent(resource: comm, action: subscribe, topic: "announcements")`,
			`agent(resource: comm, action: status)`,
			`agent(resource: comm, action: send_loop, channel_id: "channel-uuid", text: "Hello from the loop!")`,
			`agent(resource: comm, action: list_channels)`,
			`agent(resource: comm, action: list_loops)`,
			`agent(resource: comm, action: get_loop, loop_id: "loop-uuid")`,
			`agent(resource: comm, action: loop_members, loop_id: "loop-uuid")`,
			`agent(resource: comm, action: channel_members, channel_id: "channel-uuid")`,
			`agent(resource: comm, action: channel_messages, channel_id: "channel-uuid", limit: 50)`,
			`agent(resource: profile, action: get)`,
			`agent(resource: profile, action: update, key: "name", value: "Jarvis")`,
			`agent(resource: profile, action: update, key: "role", value: "Marketing Lead")`,
			`agent(resource: profile, action: update, key: "emoji", value: "🤖")`,
			`agent(resource: profile, action: update, key: "creature", value: "Rogue Diplomat")`,
			`agent(resource: profile, action: update, key: "vibe", value: "chill but opinionated")`,
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
			{Name: "subject", Type: "string", Description: "Work task subject (for task.create)"},
			{Name: "task_id", Type: "string", Description: "Work task ID (for task.update/delete)"},
			{Name: "status", Type: "string", Description: "Work task status (for task.update)", Enum: []string{"in_progress", "completed"}},

			// Reminder (scheduling) fields
			{Name: "name", Type: "string", Description: "Unique reminder name"},
			{Name: "at", Type: "string", Description: "PREFERRED for one-time reminders: 'in 5 minutes', 'in 1 hour', '7:30pm', '19:30'"},
			{Name: "schedule", Type: "string", Description: "For recurring schedules only: 'second minute hour day-of-month month day-of-week'"},
			{Name: "instructions", Type: "string", Description: "How to accomplish the task: which tools, steps, constraints. Injected as context when the reminder fires."},
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
			{Name: "channel", Type: "string", Description: "Channel type (from installed apps — use list action to see available)"},
			{Name: "to", Type: "string", Description: "Destination chat/channel ID"},
			{Name: "text", Type: "string", Description: "Message text to send"},
			{Name: "reply_to", Type: "string", Description: "Message ID to reply to"},
			{Name: "thread_id", Type: "string", Description: "Thread ID for threaded messages"},
			{Name: "widgets", Type: "array", Description: "Interactive widgets for ask action: [{type, label, options, default}]. Types: buttons, select, text_input, confirm, radio, checkbox"},

			// Session fields
			{Name: "session_key", Type: "string", Description: "Session key identifier"},
			{Name: "limit", Type: "integer", Description: "Max messages to return (default: 20)"},

			// Comm fields
			{Name: "topic", Type: "string", Description: "Comm topic/channel name for subscribe/send"},
			{Name: "msg_type", Type: "string", Description: "Comm message type", Enum: []string{"message", "mention", "proposal", "command", "info"}},
			{Name: "channel_id", Type: "string", Description: "Loop channel ID (for send_loop, channel_members, channel_messages)"},
			{Name: "loop_id", Type: "string", Description: "Loop ID (for get_loop, loop_members)"},
		},
	})
}

// RequiresApproval returns true for dangerous operations
func (t *AgentDomainTool) RequiresApproval() bool {
	// Handled per-action in Execute
	return false
}

// resourceAliases maps common synonyms to their canonical resource name.
// The LLM echoes whatever word the user said ("remind me", "schedule a job", etc.)
// so we normalize before routing.
var resourceAliases = map[string]string{
	"routine":   "reminder",
	"routines":  "reminder",
	"remind":    "reminder",
	"reminders": "reminder",
	"schedule":  "reminder",
	"schedules": "reminder",
	"job":       "reminder",
	"jobs":      "reminder",
	"cron":      "reminder",
	"event":     "reminder",
	"events":    "reminder",
	"calendar":  "reminder",
}

// normalizeAgentResource maps synonym resource names to their canonical form.
func normalizeAgentResource(resource string) string {
	if canonical, ok := resourceAliases[strings.ToLower(resource)]; ok {
		return canonical
	}
	return resource
}

// actionToResource maps actions that are unique to a single resource.
// When the model omits "resource" but provides "action", we infer the resource.
var actionToResource = map[string]string{
	// memory-only
	"store":  "memory",
	"recall": "memory",
	// task-only
	"spawn": "task",
	// reminder-only
	"pause":  "reminder",
	"resume": "reminder",
	"run":    "reminder",
	// comm-only
	"subscribe":        "comm",
	"unsubscribe":      "comm",
	"list_topics":      "comm",
	"send_loop":        "comm",
	"list_channels":    "comm",
	"list_loops":       "comm",
	"get_loop":         "comm",
	"loop_members":     "comm",
	"channel_members":  "comm",
	"channel_messages": "comm",
	// message-only
	"ask": "message",
	// profile-only
	"update": "profile",
	"get":    "profile",
}

// inferResource fills in a missing resource field from the action when unambiguous.
func inferResource(resource, action string) string {
	if resource != "" {
		return resource
	}
	if r, ok := actionToResource[action]; ok {
		return r
	}
	return resource
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

	// Normalize resource synonyms — the LLM picks up whatever word the user used
	in.Resource = normalizeAgentResource(in.Resource)

	// Auto-infer resource from action when the model omits it
	in.Resource = inferResource(in.Resource, in.Action)

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
	case "reminder":
		return t.handleCron(ctx, in)
	case "memory":
		return t.handleMemory(ctx, in)
	case "message":
		return t.handleMessage(ctx, in)
	case "session":
		return t.handleSession(ctx, in)
	case "comm":
		return t.handleComm(ctx, in)
	case "profile":
		return t.handleProfile(ctx, in)
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
	// Work tracking actions don't need the orchestrator
	switch in.Action {
	case "create":
		return t.taskCreate(ctx, in)
	case "update":
		return t.taskUpdate(ctx, in)
	case "delete":
		return t.taskDelete(ctx, in)
	case "list":
		return t.taskList(ctx)
	}

	// Sub-agent actions require orchestrator
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

	// Resolve subagent lane model override from config
	var subagentModel string
	if cfg := provider.GetModelsConfig(); cfg != nil && cfg.LaneRouting != nil && cfg.LaneRouting.Subagent != "" {
		subagentModel = cfg.LaneRouting.Subagent
	}

	// Spawn the sub-agent
	agent, err := t.orchestrator.Spawn(ctx, &orchestrator.SpawnRequest{
		Task:          in.Prompt,
		Description:   in.Description,
		Wait:          wait,
		Timeout:       time.Duration(timeout) * time.Second,
		SystemPrompt:  systemPrompt,
		ModelOverride: subagentModel,
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

// workTaskCounter generates short numeric IDs for work tasks.
var workTaskCounter atomic.Int64

func (t *AgentDomainTool) taskCreate(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.Subject == "" {
		return &ToolResult{
			Content: "Error: 'subject' is required for create action",
			IsError: true,
		}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}

	id := strconv.FormatInt(workTaskCounter.Add(1), 10)
	task := WorkTask{
		ID:        id,
		Subject:   in.Subject,
		Status:    "pending",
		CreatedAt: time.Now(),
	}

	// Append to session's work task list (stored as *[]WorkTask for comparability)
	initial := []WorkTask{task}
	for {
		existing, loaded := t.workTasks.LoadOrStore(sessionKey, &initial)
		if !loaded {
			break // freshly stored
		}
		ptr := existing.(*[]WorkTask)
		updated := append(*ptr, task)
		if t.workTasks.CompareAndSwap(sessionKey, ptr, &updated) {
			break
		}
		// retry on race
	}

	return &ToolResult{Content: fmt.Sprintf("Task [%s] created: %s", id, in.Subject)}, nil
}

func (t *AgentDomainTool) taskUpdate(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.TaskID == "" {
		return &ToolResult{
			Content: "Error: 'task_id' is required for update action",
			IsError: true,
		}, nil
	}
	if in.Status == "" {
		return &ToolResult{
			Content: "Error: 'status' is required for update action (in_progress, completed)",
			IsError: true,
		}, nil
	}
	if in.Status != "pending" && in.Status != "in_progress" && in.Status != "completed" {
		return &ToolResult{
			Content: fmt.Sprintf("Error: invalid status '%s' — must be pending, in_progress, or completed", in.Status),
			IsError: true,
		}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}

	val, ok := t.workTasks.Load(sessionKey)
	if !ok {
		return &ToolResult{
			Content: fmt.Sprintf("Error: task %s not found", in.TaskID),
			IsError: true,
		}, nil
	}

	tasks := val.(*[]WorkTask)
	for i := range *tasks {
		if (*tasks)[i].ID == in.TaskID {
			(*tasks)[i].Status = in.Status
			return &ToolResult{Content: fmt.Sprintf("Task [%s] → %s", in.TaskID, in.Status)}, nil
		}
	}

	return &ToolResult{
		Content: fmt.Sprintf("Error: task %s not found", in.TaskID),
		IsError: true,
	}, nil
}

func (t *AgentDomainTool) taskDelete(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.TaskID == "" {
		return &ToolResult{
			Content: "Error: 'task_id' is required for delete action",
			IsError: true,
		}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}

	val, ok := t.workTasks.Load(sessionKey)
	if !ok {
		return &ToolResult{
			Content: fmt.Sprintf("Error: task %s not found", in.TaskID),
			IsError: true,
		}, nil
	}

	ptr := val.(*[]WorkTask)
	tasks := *ptr
	for i, wt := range tasks {
		if wt.ID == in.TaskID {
			updated := append(tasks[:i], tasks[i+1:]...)
			t.workTasks.Store(sessionKey, &updated)
			return &ToolResult{Content: fmt.Sprintf("Task [%s] deleted: %s", wt.ID, wt.Subject)}, nil
		}
	}

	return &ToolResult{
		Content: fmt.Sprintf("Error: task %s not found", in.TaskID),
		IsError: true,
	}, nil
}

func (t *AgentDomainTool) taskList(ctx context.Context) (*ToolResult, error) {
	var result strings.Builder
	hasContent := false

	// Work tasks
	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}
	if val, ok := t.workTasks.Load(sessionKey); ok {
		tasks := *val.(*[]WorkTask)
		if len(tasks) > 0 {
			result.WriteString(fmt.Sprintf("Work tasks (%d):\n", len(tasks)))
			for _, wt := range tasks {
				icon := "[ ]"
				switch wt.Status {
				case "in_progress":
					icon = "[→]"
				case "completed":
					icon = "[✓]"
				}
				result.WriteString(fmt.Sprintf("  %s [%s] %s\n", icon, wt.ID, wt.Subject))
			}
			hasContent = true
		}
	}

	// Sub-agents
	if t.orchestrator != nil {
		agents := t.orchestrator.ListAgents()
		if len(agents) > 0 {
			if hasContent {
				result.WriteString("\n")
			}
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
			hasContent = true
		}
	}

	if !hasContent {
		return &ToolResult{Content: "No tasks or sub-agents"}, nil
	}

	return &ToolResult{Content: result.String()}, nil
}

// ListWorkTasks returns work tasks for a session (used by steering pipeline).
func (t *AgentDomainTool) ListWorkTasks(sessionKey string) []WorkTask {
	if val, ok := t.workTasks.Load(sessionKey); ok {
		return *val.(*[]WorkTask)
	}
	return nil
}

// ClearWorkTasks removes all work tasks for a session (called when objective changes).
func (t *AgentDomainTool) ClearWorkTasks(sessionKey string) {
	t.workTasks.Delete(sessionKey)
}

// =============================================================================
// Routine handlers (scheduled tasks)
// =============================================================================

func (t *AgentDomainTool) handleCron(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.scheduler == nil {
		return &ToolResult{
			Content: "Error: Scheduler not configured",
			IsError: true,
		}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "create":
		var deliver string
		if in.Deliver != nil {
			d, _ := json.Marshal(in.Deliver)
			deliver = string(d)
		}
		item, createErr := t.scheduler.Create(ctx, ScheduleItem{
			Name:       in.Name,
			Expression: in.Schedule,
			TaskType:   in.TaskType,
			Command:    in.Command,
			Message:    in.Message,
			Deliver:    deliver,
		})
		if createErr != nil {
			err = createErr
		} else {
			result = fmt.Sprintf("Created schedule %q (expression: %s, type: %s)", item.Name, item.Expression, item.TaskType)
		}

	case "list":
		items, total, listErr := t.scheduler.List(ctx, 50, 0, false)
		if listErr != nil {
			err = listErr
		} else if len(items) == 0 {
			result = "No scheduled tasks found."
		} else {
			var sb strings.Builder
			sb.WriteString(fmt.Sprintf("Scheduled tasks (%d total):\n\n", total))
			for _, item := range items {
				status := "disabled"
				if item.Enabled {
					status = "enabled"
				}
				sb.WriteString(fmt.Sprintf("- %s [%s] (%s) — %s\n", item.Name, item.Expression, item.TaskType, status))
			}
			result = sb.String()
		}

	case "delete":
		if delErr := t.scheduler.Delete(ctx, in.Name); delErr != nil {
			err = delErr
		} else {
			result = fmt.Sprintf("Deleted schedule %q", in.Name)
		}

	case "pause":
		if _, disErr := t.scheduler.Disable(ctx, in.Name); disErr != nil {
			err = disErr
		} else {
			result = fmt.Sprintf("Paused schedule %q", in.Name)
		}

	case "resume":
		if _, enErr := t.scheduler.Enable(ctx, in.Name); enErr != nil {
			err = enErr
		} else {
			result = fmt.Sprintf("Resumed schedule %q", in.Name)
		}

	case "run":
		output, runErr := t.scheduler.Trigger(ctx, in.Name)
		if runErr != nil {
			err = runErr
		} else {
			result = fmt.Sprintf("Triggered schedule %q: %s", in.Name, output)
		}

	case "history":
		entries, _, histErr := t.scheduler.History(ctx, in.Name, 10, 0)
		if histErr != nil {
			err = histErr
		} else if len(entries) == 0 {
			result = fmt.Sprintf("No history for schedule %q", in.Name)
		} else {
			var sb strings.Builder
			sb.WriteString(fmt.Sprintf("History for %q:\n\n", in.Name))
			for _, e := range entries {
				status := "success"
				if !e.Success {
					status = "failed"
				}
				sb.WriteString(fmt.Sprintf("- %s [%s] %s\n", e.StartedAt.Format(time.RFC3339), status, e.Output))
			}
			result = sb.String()
		}

	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown reminder action: %s", in.Action),
			IsError: true,
		}, nil
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Schedule action failed: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: result,
		IsError: false,
	}, nil
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
	// Ask doesn't need channelSender — it goes through the web UI
	if in.Action == "ask" {
		return t.messageAsk(ctx, in)
	}

	if t.channelSender == nil {
		return &ToolResult{
			Content: "Error: No channels configured. Install channel apps first.",
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
	ids := t.channelSender.ListChannels()
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
}

func (t *AgentDomainTool) messageSend(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.Channel == "" {
		return &ToolResult{
			Content: "Error: 'channel' is required — use list action to see available channels",
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

	if err := t.channelSender.SendToChannel(ctx, in.Channel, in.To, in.Text); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error sending message: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Message sent to %s:%s", in.Channel, in.To),
	}, nil
}

func (t *AgentDomainTool) messageAsk(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if t.askCallback == nil {
		return &ToolResult{
			Content: "Error: Interactive prompts require the web UI",
			IsError: true,
		}, nil
	}

	// Prompt can come from the prompt or text field
	prompt := in.Prompt
	if prompt == "" {
		prompt = in.Text
	}
	if prompt == "" {
		return &ToolResult{
			Content: "Error: 'prompt' (or 'text') is required for ask action",
			IsError: true,
		}, nil
	}

	// Default to confirm (yes/no) when no widgets specified
	widgets := in.Widgets
	if len(widgets) == 0 {
		widgets = []AskWidget{{
			Type:    "confirm",
			Options: []string{"Yes", "No"},
		}}
	}

	requestID := uuid.New().String()
	response, err := t.askCallback(ctx, requestID, prompt, widgets)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error waiting for user response: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: response,
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
	case "send_loop":
		return t.commSendLoop(ctx, in)
	case "list_channels":
		return t.commListChannels(ctx)
	case "list_loops":
		return t.commListLoops(ctx)
	case "get_loop":
		return t.commGetLoop(ctx, in)
	case "loop_members":
		return t.commLoopMembers(ctx, in)
	case "channel_members":
		return t.commChannelMembers(ctx, in)
	case "channel_messages":
		return t.commChannelMessages(ctx, in)
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

func (t *AgentDomainTool) commSendLoop(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	channelID := in.ChannelID
	if channelID == "" {
		channelID = in.To
	}
	if channelID == "" {
		return &ToolResult{
			Content: "Error: 'channel_id' is required (loop channel ID)",
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

	if err := t.commService.Send(ctx, channelID, "", text, "loop_channel"); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error sending loop channel message: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Message sent to loop channel %s", channelID),
	}, nil
}

func (t *AgentDomainTool) commListChannels(ctx context.Context) (*ToolResult, error) {
	if t.loopChannelLister == nil {
		return &ToolResult{
			Content: "Loop channel listing not available (not connected to NeboLoop)",
			IsError: true,
		}, nil
	}

	channels, err := t.loopChannelLister(ctx)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error listing loop channels: %v", err),
			IsError: true,
		}, nil
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

// =============================================================================
// Loop query handlers (Bot Query System)
// =============================================================================

func (t *AgentDomainTool) commListLoops(ctx context.Context) (*ToolResult, error) {
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

func (t *AgentDomainTool) commGetLoop(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
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

func (t *AgentDomainTool) commLoopMembers(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
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

func (t *AgentDomainTool) commChannelMembers(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
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

func (t *AgentDomainTool) commChannelMessages(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
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

// =============================================================================
// Profile management
// =============================================================================

// handleProfile handles agent profile reads and updates.
func (t *AgentDomainTool) handleProfile(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	switch in.Action {
	case "get":
		return t.handleProfileGet(ctx)
	case "update":
		return t.handleProfileUpdate(ctx, in)
	case "open_billing":
		return t.handleProfileOpenBilling(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown profile action: %s. Available: get, update, open_billing", in.Action)}, nil
	}
}

func (t *AgentDomainTool) handleProfileGet(ctx context.Context) (*ToolResult, error) {
	if t.sessions == nil {
		return &ToolResult{Content: "Profile unavailable: no database connection"}, nil
	}
	rawDB := t.sessions.GetDB()
	if rawDB == nil {
		return &ToolResult{Content: "Profile unavailable: no database connection"}, nil
	}
	queries := db.New(rawDB)
	profile, err := queries.GetAgentProfile(ctx)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to read profile: %v", err)}, nil
	}
	var sb strings.Builder
	sb.WriteString("Agent Profile:\n")
	sb.WriteString(fmt.Sprintf("  Name: %s\n", profile.Name))
	if profile.Emoji.Valid && profile.Emoji.String != "" {
		sb.WriteString(fmt.Sprintf("  Emoji: %s\n", profile.Emoji.String))
	}
	if profile.Creature.Valid && profile.Creature.String != "" {
		sb.WriteString(fmt.Sprintf("  Creature: %s\n", profile.Creature.String))
	}
	if profile.Vibe.Valid && profile.Vibe.String != "" {
		sb.WriteString(fmt.Sprintf("  Vibe: %s\n", profile.Vibe.String))
	}
	if profile.Role.Valid && profile.Role.String != "" {
		sb.WriteString(fmt.Sprintf("  Role: %s\n", profile.Role.String))
	}
	if profile.PersonalityPreset.Valid {
		sb.WriteString(fmt.Sprintf("  Personality Preset: %s\n", profile.PersonalityPreset.String))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *AgentDomainTool) handleProfileUpdate(ctx context.Context, in AgentDomainInput) (*ToolResult, error) {
	if in.Key == "" {
		return &ToolResult{Content: "Profile update requires key and value. Supported keys: name, role, emoji, creature, vibe, custom_personality, quiet_hours_start, quiet_hours_end"}, nil
	}
	// Allow empty value for clearing quiet hours
	if in.Value == "" && in.Key != "quiet_hours_start" && in.Key != "quiet_hours_end" {
		return &ToolResult{Content: "Profile update requires key and value. Supported keys: name, role, emoji, creature, vibe, custom_personality, quiet_hours_start, quiet_hours_end"}, nil
	}

	if t.sessions == nil {
		return &ToolResult{Content: "Profile update unavailable: no database connection"}, nil
	}

	rawDB := t.sessions.GetDB()
	if rawDB == nil {
		return &ToolResult{Content: "Profile update unavailable: no database connection"}, nil
	}

	queries := db.New(rawDB)
	params := db.UpdateAgentProfileParams{}

	switch in.Key {
	case "name":
		if len(in.Value) > 50 {
			return &ToolResult{Content: "Name too long (max 50 characters)"}, nil
		}
		params.Name = sql.NullString{String: in.Value, Valid: true}
	case "role":
		if len(in.Value) > 100 {
			return &ToolResult{Content: "Role too long (max 100 characters)"}, nil
		}
		params.Role = sql.NullString{String: in.Value, Valid: true}
	case "emoji":
		params.Emoji = sql.NullString{String: in.Value, Valid: true}
	case "creature":
		if len(in.Value) > 100 {
			return &ToolResult{Content: "Creature too long (max 100 characters)"}, nil
		}
		params.Creature = sql.NullString{String: in.Value, Valid: true}
	case "vibe":
		if len(in.Value) > 200 {
			return &ToolResult{Content: "Vibe too long (max 200 characters)"}, nil
		}
		params.Vibe = sql.NullString{String: in.Value, Valid: true}
	case "custom_personality":
		params.CustomPersonality = sql.NullString{String: in.Value, Valid: true}
	case "quiet_hours_start", "quiet_hours_end":
		// Validate HH:MM format (or empty to clear)
		if in.Value != "" {
			parts := strings.SplitN(in.Value, ":", 2)
			if len(parts) != 2 {
				return &ToolResult{Content: "Quiet hours must be in HH:MM format (e.g., \"22:00\") or empty to clear"}, nil
			}
		}
		if in.Key == "quiet_hours_start" {
			params.QuietHoursStart = sql.NullString{String: in.Value, Valid: true}
		} else {
			params.QuietHoursEnd = sql.NullString{String: in.Value, Valid: true}
		}
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown profile key: %s. Supported keys: name, role, emoji, creature, vibe, custom_personality, quiet_hours_start, quiet_hours_end", in.Key)}, nil
	}

	err := queries.UpdateAgentProfile(ctx, params)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to update %s: %v", in.Key, err)}, nil
	}

	// Sync name/role changes to NeboLoop so other bots see the update
	if (in.Key == "name" || in.Key == "role") && t.identitySyncer != nil {
		profile, profileErr := queries.GetAgentProfile(ctx)
		if profileErr == nil {
			name := profile.Name
			if name == "" {
				name = "Nebo"
			}
			role := ""
			if profile.Role.Valid {
				role = profile.Role.String
			}
			t.identitySyncer(ctx, name, role)
		}
	}

	if in.Key == "name" {
		return &ToolResult{Content: fmt.Sprintf("Updated agent name to %q. I will now refer to myself as %s.", in.Value, in.Value)}, nil
	}
	if in.Key == "role" {
		return &ToolResult{Content: fmt.Sprintf("Updated role to %q. This has been synced to NeboLoop.", in.Value)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Updated agent %s to %q", in.Key, in.Value)}, nil
}

func (t *AgentDomainTool) handleProfileOpenBilling(ctx context.Context) (*ToolResult, error) {
	if t.sessions == nil {
		return &ToolResult{Content: "Billing page unavailable: no database connection"}, nil
	}
	rawDB := t.sessions.GetDB()
	if rawDB == nil {
		return &ToolResult{Content: "Billing page unavailable: no database connection"}, nil
	}
	queries := db.New(rawDB)
	profiles, err := queries.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
	if err != nil || len(profiles) == 0 {
		return &ToolResult{Content: "No NeboLoop account connected. The user needs to connect via Settings > NeboLoop first."}, nil
	}
	token := profiles[0].ApiKey
	billingURL := "https://app.neboloop.com/billing?token=" + token
	openBrowserURL(billingURL)
	return &ToolResult{Content: "Opened NeboLoop billing page in your browser."}, nil
}

// openBrowserURL opens a URL in the user's default system browser.
func openBrowserURL(targetURL string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", targetURL)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", targetURL)
	default:
		cmd = exec.Command("xdg-open", targetURL)
	}
	cmd.Stdin = strings.NewReader("")
	cmd.Stdout = nil
	cmd.Stderr = nil
	if err := cmd.Start(); err != nil {
		fmt.Printf("[Agent] Failed to open browser: %v\n", err)
	}
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
func (t *AgentDomainTool) StoreEntryForUser(layer, namespace, key, value string, tags []string, userID string, confidence float64) error {
	if t.memory == nil {
		return fmt.Errorf("memory storage not configured")
	}
	return t.memory.StoreEntryForUser(layer, namespace, key, value, tags, userID, confidence)
}

// GetMemoryTool returns the underlying memory tool for direct access
func (t *AgentDomainTool) GetMemoryTool() *MemoryTool {
	return t.memory
}

// GetScheduler returns the underlying scheduler for direct access
func (t *AgentDomainTool) GetScheduler() Scheduler {
	return t.scheduler
}
