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
	"github.com/neboloop/nebo/internal/agent/advisors"
	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/embeddings"
	"github.com/neboloop/nebo/internal/agent/orchestrator"
	"github.com/neboloop/nebo/internal/agent/recovery"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/provider"
)

// BotTool consolidates the agent's self-management capabilities into a single
// STRAP domain tool: task, memory, session, profile, context, advisors, vision, ask.
type BotTool struct {
	// Task/orchestration
	orchestrator *orchestrator.Orchestrator

	// Memory storage
	memory *MemoryTool

	// Session management
	sessions      *session.Manager
	currentUserID string

	// Session querier (cross-session reads)
	sessionQuerier SessionQuerier

	// Identity sync (pushes name/role to NeboLoop)
	identitySyncer func(ctx context.Context, name, role string)

	// Interactive user prompts
	askCallback AskCallback

	// Vision analysis
	visionTool *VisionTool

	// Advisors
	advisorsTool *AdvisorsTool

	// Work task tracking (in-memory, session-scoped)
	workTasks sync.Map // sessionKey → *[]WorkTask

	// App hooks dispatcher
	hooks HookDispatcher
}

// BotToolConfig configures the bot domain tool.
type BotToolConfig struct {
	Sessions   *session.Manager
	MemoryTool *MemoryTool
}

// NewBotTool creates a new bot domain tool.
func NewBotTool(cfg BotToolConfig) *BotTool {
	return &BotTool{
		sessions: cfg.Sessions,
		memory:   cfg.MemoryTool,
	}
}

// --- Setters for late-bound dependencies ---

func (t *BotTool) SetOrchestrator(orch *orchestrator.Orchestrator)          { t.orchestrator = orch }
func (t *BotTool) SetRecoveryManager(mgr *recovery.Manager) {
	if t.orchestrator != nil {
		t.orchestrator.SetRecoveryManager(mgr)
	}
}
func (t *BotTool) SetIdentitySyncer(fn func(ctx context.Context, name, role string)) {
	t.identitySyncer = fn
}
func (t *BotTool) SetAskCallback(fn AskCallback)         { t.askCallback = fn }
func (t *BotTool) SetCurrentUser(userID string) {
	t.currentUserID = userID
	if t.memory != nil {
		t.memory.SetCurrentUser(userID)
	}
}
func (t *BotTool) SetSessionQuerier(q SessionQuerier)     { t.sessionQuerier = q }
func (t *BotTool) SetVisionTool(v *VisionTool)            { t.visionTool = v }
func (t *BotTool) SetAdvisorsTool(a *AdvisorsTool)        { t.advisorsTool = a }
func (t *BotTool) SetHookDispatcher(h HookDispatcher)     { t.hooks = h }

func (t *BotTool) GetCurrentUser() string                         { return t.currentUserID }
func (t *BotTool) GetMemoryTool() *MemoryTool                    { return t.memory }
func (t *BotTool) GetOrchestrator() *orchestrator.Orchestrator    { return t.orchestrator }
func (t *BotTool) GetAdvisorsTool() *AdvisorsTool                 { return t.advisorsTool }
func (t *BotTool) GetVisionTool() *VisionTool                    { return t.visionTool }

// CreateOrchestrator creates and sets a new orchestrator.
func (t *BotTool) CreateOrchestrator(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, registry *Registry) {
	adapter := &registryAdapter{registry: registry}
	t.orchestrator = orchestrator.NewOrchestrator(cfg, sessions, providers, adapter)
}

// RecoverSubagents restores pending subagent tasks from the database.
func (t *BotTool) RecoverSubagents(ctx context.Context) (int, error) {
	if t.orchestrator == nil {
		return 0, nil
	}
	return t.orchestrator.RecoverAgents(ctx)
}

// Close cleans up resources.
func (t *BotTool) Close() error {
	if t.memory != nil {
		t.memory.Close()
	}
	return nil
}

// --- Tool interface ---

func (t *BotTool) Name() string { return "bot" }

func (t *BotTool) Domain() string { return "bot" }

func (t *BotTool) Resources() []string {
	return []string{"task", "memory", "session", "profile", "context", "advisors", "vision", "ask"}
}

func (t *BotTool) ActionsFor(resource string) []string {
	switch resource {
	case "task":
		return []string{"spawn", "status", "cancel", "list", "create", "update", "delete"}
	case "memory":
		return []string{"store", "recall", "search", "list", "delete", "clear"}
	case "session":
		return []string{"list", "history", "status", "clear", "query"}
	case "profile":
		return []string{"get", "update", "open_billing"}
	case "context":
		return []string{"reset", "compact", "summary"}
	case "advisors":
		return []string{"deliberate"}
	case "vision":
		return []string{"analyze"}
	case "ask":
		return []string{"prompt", "confirm", "select"}
	default:
		return nil
	}
}

func (t *BotTool) RequiresApproval() bool { return false }

var botResources = map[string]ResourceConfig{
	"task":     {Name: "task", Actions: []string{"spawn", "status", "cancel", "list", "create", "update", "delete"}, Description: "Sub-agent management + work tracking"},
	"memory":   {Name: "memory", Actions: []string{"store", "recall", "search", "list", "delete", "clear"}, Description: "Persistent storage (tacit/daily/entity)"},
	"session":  {Name: "session", Actions: []string{"list", "history", "status", "clear", "query"}, Description: "Conversation sessions"},
	"profile":  {Name: "profile", Actions: []string{"get", "update", "open_billing"}, Description: "Agent identity management"},
	"context":  {Name: "context", Actions: []string{"reset", "compact", "summary"}, Description: "Context window management"},
	"advisors": {Name: "advisors", Actions: []string{"deliberate"}, Description: "Internal deliberation system"},
	"vision":   {Name: "vision", Actions: []string{"analyze"}, Description: "Image analysis"},
	"ask":      {Name: "ask", Actions: []string{"prompt", "confirm", "select"}, Description: "Interactive user prompts"},
}

func (t *BotTool) Description() string {
	return BuildDomainDescription(DomainSchemaConfig{
		Domain: "bot",
		Description: `Self-management — identity, knowledge, reasoning, user input.

Resources:
- task: Sub-agents (spawn, status, cancel) + work tracking (create, update, delete, list)
- memory: Three-tier persistent storage (tacit/daily/entity layers)
- session: Manage conversation sessions + cross-session queries
- profile: Read and update your own identity (name, emoji, creature, vibe, personality)
- context: Manage your context window (reset, compact, summary)
- advisors: Consult internal advisors for deliberation on complex decisions
- vision: Analyze images using AI vision capabilities
- ask: Interactive prompts — ask the user questions with widgets (buttons, selects, confirms)`,
		Resources: botResources,
		Examples: []string{
			`bot(resource: "task", action: "spawn", prompt: "Find all Go files with errors", agent_type: "explore")`,
			`bot(resource: "task", action: "create", subject: "Read existing skill format")`,
			`bot(resource: "task", action: "update", task_id: "1", status: "completed")`,
			`bot(resource: "memory", action: "store", key: "user/name", value: "Alice", layer: "tacit")`,
			`bot(resource: "memory", action: "search", query: "preferences", layer: "tacit")`,
			`bot(resource: "session", action: "list")`,
			`bot(resource: "session", action: "query", session_key: "loop-channel-abc", limit: 10)`,
			`bot(resource: "profile", action: "get")`,
			`bot(resource: "profile", action: "update", key: "name", value: "Jarvis")`,
			`bot(resource: "advisors", action: "deliberate", task: "Should I use React or Svelte?")`,
			`bot(resource: "vision", action: "analyze", image: "/path/to/image.png", prompt: "What is this?")`,
			`bot(resource: "ask", action: "prompt", text: "Which framework?", widgets: [{"type": "buttons", "options": ["React", "Svelte"]}])`,
		},
	})
}

func (t *BotTool) Schema() json.RawMessage {
	return BuildDomainSchema(DomainSchemaConfig{
		Domain:      "bot",
		Description: t.Description(),
		Resources:   botResources,
		Fields: []FieldConfig{
			// Task fields
			{Name: "description", Type: "string", Description: "Short task description (3-5 words)"},
			{Name: "prompt", Type: "string", Description: "Detailed task prompt for sub-agent, or ask prompt"},
			{Name: "wait", Type: "boolean", Description: "Wait for task completion (default: true)", Default: true},
			{Name: "timeout", Type: "integer", Description: "Timeout in seconds (default: 300)", Default: 300},
			{Name: "agent_type", Type: "string", Description: "Agent type: explore, plan, general", Enum: []string{"explore", "plan", "general"}},
			{Name: "agent_id", Type: "string", Description: "Agent ID for status/cancel operations"},
			{Name: "subject", Type: "string", Description: "Work task subject (for task.create)"},
			{Name: "task_id", Type: "string", Description: "Work task ID (for task.update/delete)"},
			{Name: "status", Type: "string", Description: "Work task status (for task.update)", Enum: []string{"in_progress", "completed"}},
			// Memory fields
			{Name: "key", Type: "string", Description: "Memory key (path-like: 'user/name', 'project/nebo')"},
			{Name: "value", Type: "string", Description: "Value to store, or profile value to set"},
			{Name: "tags", Type: "array", Description: "Tags for categorization"},
			{Name: "query", Type: "string", Description: "Search query for memory search"},
			{Name: "namespace", Type: "string", Description: "Namespace for organization (default: 'default')"},
			{Name: "layer", Type: "string", Description: "Memory layer: tacit, daily, entity", Enum: []string{"tacit", "daily", "entity"}},
			{Name: "metadata", Type: "object", Description: "Additional metadata"},
			// Session fields
			{Name: "session_key", Type: "string", Description: "Session key identifier"},
			{Name: "limit", Type: "integer", Description: "Max messages to return (default: 20)"},
			// Advisors fields
			{Name: "task", Type: "string", Description: "Task to deliberate on (for advisors)"},
			{Name: "advisors_list", Type: "array", Description: "Specific advisor names to consult"},
			// Vision fields
			{Name: "image", Type: "string", Description: "Image source: file path, URL, or base64 data"},
			// Ask fields
			{Name: "text", Type: "string", Description: "Prompt text for ask action"},
			{Name: "widgets", Type: "array", Description: "Interactive widgets: [{type, label, options, default}]", ItemSchema: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"type":    map[string]any{"type": "string", "description": "Widget type: buttons, select, confirm, checkbox"},
					"label":   map[string]any{"type": "string", "description": "Widget label"},
					"options": map[string]any{"type": "array", "items": map[string]any{"type": "string"}, "description": "Options for buttons/select/checkbox"},
					"default": map[string]any{"type": "string", "description": "Default value"},
				},
				"required": []string{"type"},
			}},
		},
	})
}

// BotInput defines the input for the bot domain tool.
type BotInput struct {
	Resource string `json:"resource"`
	Action   string `json:"action"`

	// Task fields
	Description string `json:"description,omitempty"`
	Prompt      string `json:"prompt,omitempty"`
	Wait        *bool  `json:"wait,omitempty"`
	Timeout     int    `json:"timeout,omitempty"`
	AgentType   string `json:"agent_type,omitempty"`
	AgentID     string `json:"agent_id,omitempty"`
	Subject     string `json:"subject,omitempty"`
	TaskID      string `json:"task_id,omitempty"`
	Status      string `json:"status,omitempty"`

	// Memory fields
	Key       string            `json:"key,omitempty"`
	Value     string            `json:"value,omitempty"`
	Tags      []string          `json:"tags,omitempty"`
	Query     string            `json:"query,omitempty"`
	Namespace string            `json:"namespace,omitempty"`
	Layer     string            `json:"layer,omitempty"`
	Metadata  map[string]string `json:"metadata,omitempty"`

	// Session fields
	SessionKey string `json:"session_key,omitempty"`
	Limit      int    `json:"limit,omitempty"`

	// Advisors fields
	Task         string   `json:"task,omitempty"`
	AdvisorsList []string `json:"advisors_list,omitempty"`
	SessionID    string   `json:"session_id,omitempty"`

	// Vision fields
	Image string `json:"image,omitempty"`

	// Ask fields
	Text    string      `json:"text,omitempty"`
	Widgets []AskWidget `json:"widgets,omitempty"`
}

// botActionToResource maps actions unique to a single resource.
var botActionToResource = map[string]string{
	"store":       "memory",
	"recall":      "memory",
	"spawn":       "task",
	"deliberate":  "advisors",
	"analyze":     "vision",
	"get":         "profile",
	"update":      "profile",
	"open_billing": "profile",
	"reset":       "context",
	"compact":     "context",
	"summary":     "context",
}

func (t *BotTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in BotInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Infer resource from action when omitted
	if in.Resource == "" {
		if r, ok := botActionToResource[in.Action]; ok {
			in.Resource = r
		}
	}

	if err := ValidateResourceAction(in.Resource, in.Action, botResources); err != nil {
		return &ToolResult{Content: err.Error(), IsError: true}, nil
	}

	switch in.Resource {
	case "task":
		return t.handleTask(ctx, in)
	case "memory":
		return t.handleMemory(ctx, in)
	case "session":
		return t.handleSession(ctx, in)
	case "profile":
		return t.handleProfile(ctx, in)
	case "context":
		return t.handleContext(ctx, in)
	case "advisors":
		return t.handleAdvisors(ctx, in)
	case "vision":
		return t.handleVision(ctx, in)
	case "ask":
		return t.handleAsk(ctx, in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown resource: %s", in.Resource), IsError: true}, nil
	}
}

// --- Memory convenience methods for programmatic access ---

func (t *BotTool) StoreEntry(layer, namespace, key, value string, tags []string) error {
	if t.memory == nil {
		return fmt.Errorf("memory storage not configured")
	}
	return t.memory.StoreEntry(layer, namespace, key, value, tags)
}

func (t *BotTool) StoreEntryForUser(layer, namespace, key, value string, tags []string, userID string, confidence float64) error {
	if t.memory == nil {
		return fmt.Errorf("memory storage not configured")
	}
	return t.memory.StoreEntryForUser(layer, namespace, key, value, tags, userID, confidence)
}

// ListWorkTasks returns work tasks for a session (used by steering pipeline).
func (t *BotTool) ListWorkTasks(sessionKey string) []WorkTask {
	if val, ok := t.workTasks.Load(sessionKey); ok {
		return *val.(*[]WorkTask)
	}
	return nil
}

// ClearWorkTasks removes all work tasks for a session.
func (t *BotTool) ClearWorkTasks(sessionKey string) {
	t.workTasks.Delete(sessionKey)
}

// =============================================================================
// Task handlers
// =============================================================================

func (t *BotTool) handleTask(ctx context.Context, in BotInput) (*ToolResult, error) {
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

	if t.orchestrator == nil {
		return &ToolResult{Content: "Error: Task orchestrator not configured", IsError: true}, nil
	}

	switch in.Action {
	case "spawn":
		return t.taskSpawn(ctx, in)
	case "status":
		return t.taskStatus(in)
	case "cancel":
		return t.taskCancel(in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown task action: %s", in.Action), IsError: true}, nil
	}
}

func (t *BotTool) taskSpawn(ctx context.Context, in BotInput) (*ToolResult, error) {
	if in.Prompt == "" {
		return &ToolResult{Content: "Error: 'prompt' is required for spawn action", IsError: true}, nil
	}
	if in.Description == "" {
		in.Description = truncateForDescription(in.Prompt)
	}
	wait := true
	if in.Wait != nil {
		wait = *in.Wait
	}
	timeout := 300
	if in.Timeout > 0 {
		timeout = in.Timeout
	}

	systemPrompt := buildAgentSystemPrompt(in.AgentType, in.Prompt)

	var subagentModel string
	if cfg := provider.GetModelsConfig(); cfg != nil && cfg.LaneRouting != nil && cfg.LaneRouting.Subagent != "" {
		subagentModel = cfg.LaneRouting.Subagent
	}

	agent, err := t.orchestrator.Spawn(ctx, &orchestrator.SpawnRequest{
		Task:          in.Prompt,
		Description:   in.Description,
		Wait:          wait,
		Timeout:       time.Duration(timeout) * time.Second,
		SystemPrompt:  systemPrompt,
		ModelOverride: subagentModel,
	})
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to spawn sub-agent: %v", err), IsError: true}, nil
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
		return &ToolResult{Content: result.String(), IsError: agent.Status == orchestrator.StatusFailed}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Sub-agent spawned: %s\nAgent ID: %s\nDescription: %s\n\nThe agent is running in the background.",
			in.Description, agent.ID, in.Prompt),
	}, nil
}

func (t *BotTool) taskStatus(in BotInput) (*ToolResult, error) {
	if in.AgentID == "" {
		return &ToolResult{Content: "Error: 'agent_id' is required for status action", IsError: true}, nil
	}
	agent, exists := t.orchestrator.GetAgent(in.AgentID)
	if !exists {
		return &ToolResult{Content: fmt.Sprintf("Agent not found: %s", in.AgentID), IsError: true}, nil
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

func (t *BotTool) taskCancel(in BotInput) (*ToolResult, error) {
	if in.AgentID == "" {
		return &ToolResult{Content: "Error: 'agent_id' is required for cancel action", IsError: true}, nil
	}
	if err := t.orchestrator.CancelAgent(in.AgentID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Agent %s cancelled", in.AgentID)}, nil
}

// botWorkTaskCounter generates short numeric IDs for work tasks.
var botWorkTaskCounter atomic.Int64

func (t *BotTool) taskCreate(ctx context.Context, in BotInput) (*ToolResult, error) {
	if in.Subject == "" {
		return &ToolResult{Content: "Error: 'subject' is required for create action", IsError: true}, nil
	}
	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}
	id := strconv.FormatInt(botWorkTaskCounter.Add(1), 10)
	task := WorkTask{
		ID:        id,
		Subject:   in.Subject,
		Status:    "pending",
		CreatedAt: time.Now(),
	}
	initial := []WorkTask{task}
	for {
		existing, loaded := t.workTasks.LoadOrStore(sessionKey, &initial)
		if !loaded {
			break
		}
		ptr := existing.(*[]WorkTask)
		updated := append(*ptr, task)
		if t.workTasks.CompareAndSwap(sessionKey, ptr, &updated) {
			break
		}
	}
	return &ToolResult{Content: fmt.Sprintf("Task [%s] created: %s", id, in.Subject)}, nil
}

func (t *BotTool) taskUpdate(ctx context.Context, in BotInput) (*ToolResult, error) {
	if in.TaskID == "" {
		return &ToolResult{Content: "Error: 'task_id' is required for update action", IsError: true}, nil
	}
	if in.Status == "" {
		return &ToolResult{Content: "Error: 'status' is required for update action (in_progress, completed)", IsError: true}, nil
	}
	if in.Status != "pending" && in.Status != "in_progress" && in.Status != "completed" {
		return &ToolResult{Content: fmt.Sprintf("Error: invalid status '%s' — must be pending, in_progress, or completed", in.Status), IsError: true}, nil
	}
	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}
	val, ok := t.workTasks.Load(sessionKey)
	if !ok {
		return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
	}
	tasks := val.(*[]WorkTask)
	for i := range *tasks {
		if (*tasks)[i].ID == in.TaskID {
			(*tasks)[i].Status = in.Status
			return &ToolResult{Content: fmt.Sprintf("Task [%s] → %s", in.TaskID, in.Status)}, nil
		}
	}
	return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
}

func (t *BotTool) taskDelete(ctx context.Context, in BotInput) (*ToolResult, error) {
	if in.TaskID == "" {
		return &ToolResult{Content: "Error: 'task_id' is required for delete action", IsError: true}, nil
	}
	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		sessionKey = "default"
	}
	val, ok := t.workTasks.Load(sessionKey)
	if !ok {
		return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
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
	return &ToolResult{Content: fmt.Sprintf("Error: task %s not found", in.TaskID), IsError: true}, nil
}

func (t *BotTool) taskList(ctx context.Context) (*ToolResult, error) {
	var result strings.Builder
	hasContent := false

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

// =============================================================================
// Memory handlers
// =============================================================================

func (t *BotTool) handleMemory(ctx context.Context, in BotInput) (*ToolResult, error) {
	if t.memory == nil {
		return &ToolResult{Content: "Error: Memory storage not configured", IsError: true}, nil
	}

	// --- HOOK: memory.pre_store ---
	if in.Action == "store" && t.hooks != nil && t.hooks.HasSubscribers("memory.pre_store") {
		payload, _ := json.Marshal(map[string]any{"key": in.Key, "value": in.Value, "layer": in.Layer})
		modified, handled := t.hooks.ApplyFilter(ctx, "memory.pre_store", payload)
		if handled {
			return &ToolResult{Content: "Memory stored (handled by app hook)"}, nil
		}
		var mod struct {
			Key   string `json:"key"`
			Value string `json:"value"`
			Layer string `json:"layer"`
		}
		if json.Unmarshal(modified, &mod) == nil {
			if mod.Key != "" {
				in.Key = mod.Key
			}
			if mod.Value != "" {
				in.Value = mod.Value
			}
			if mod.Layer != "" {
				in.Layer = mod.Layer
			}
		}
	}

	// --- HOOK: memory.pre_recall ---
	if in.Action == "recall" && t.hooks != nil && t.hooks.HasSubscribers("memory.pre_recall") {
		payload, _ := json.Marshal(map[string]any{"query": in.Query, "key": in.Key})
		modified, handled := t.hooks.ApplyFilter(ctx, "memory.pre_recall", payload)
		if handled {
			// App provided its own recall results
			var mod struct {
				Results json.RawMessage `json:"results"`
			}
			if json.Unmarshal(modified, &mod) == nil && mod.Results != nil {
				return &ToolResult{Content: string(mod.Results)}, nil
			}
			return &ToolResult{Content: string(modified)}, nil
		}
	}

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
	memJSON, _ := json.Marshal(memIn)
	return t.memory.Execute(ctx, memJSON)
}

// =============================================================================
// Session handlers
// =============================================================================

func (t *BotTool) handleSession(ctx context.Context, in BotInput) (*ToolResult, error) {
	// query action uses the SessionQuerier
	if in.Action == "query" {
		return t.sessionQuery(ctx, in)
	}

	if t.sessions == nil {
		return &ToolResult{Content: "Error: Session manager not configured", IsError: true}, nil
	}

	if in.SessionKey == "" {
		in.SessionKey = GetSessionKey(ctx)
		if in.SessionKey == "" {
			in.SessionKey = "default"
		}
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
		return &ToolResult{Content: fmt.Sprintf("Unknown session action: %s", in.Action), IsError: true}, nil
	}
}

func (t *BotTool) sessionList() (*ToolResult, error) {
	sessions, err := t.sessions.ListSessions(t.currentUserID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error listing sessions: %v", err), IsError: true}, nil
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

func (t *BotTool) sessionHistory(in BotInput) (*ToolResult, error) {
	limit := in.Limit
	if limit <= 0 {
		limit = 20
	}
	if limit > 100 {
		limit = 100
	}
	sess, err := t.sessions.GetOrCreate(in.SessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error getting session: %v", err), IsError: true}, nil
	}
	messages, err := t.sessions.GetMessages(sess.ID, limit)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error getting messages: %v", err), IsError: true}, nil
	}
	if len(messages) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No messages in session: %s", in.SessionKey)}, nil
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

func (t *BotTool) sessionStatus(in BotInput) (*ToolResult, error) {
	sess, err := t.sessions.GetOrCreate(in.SessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error getting session: %v", err), IsError: true}, nil
	}
	messages, err := t.sessions.GetMessages(sess.ID, 1000)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error getting messages: %v", err), IsError: true}, nil
	}
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

func (t *BotTool) sessionClear(in BotInput) (*ToolResult, error) {
	sess, err := t.sessions.GetOrCreate(in.SessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error getting session: %v", err), IsError: true}, nil
	}
	if err := t.sessions.Reset(sess.ID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error clearing session: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Session cleared: %s", in.SessionKey)}, nil
}

func (t *BotTool) sessionQuery(_ context.Context, in BotInput) (*ToolResult, error) {
	if t.sessionQuerier == nil {
		return &ToolResult{Content: "Session querier not configured", IsError: true}, nil
	}
	// Delegate to the same logic as QuerySessionsTool
	if in.SessionKey == "" {
		// list sessions
		sessions, err := t.sessionQuerier.ListSessions("")
		if err != nil {
			return &ToolResult{Content: "Failed to list sessions: " + err.Error(), IsError: true}, nil
		}
		if len(sessions) == 0 {
			return &ToolResult{Content: "No sessions found"}, nil
		}
		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("Sessions (%d):\n", len(sessions)))
		for _, s := range sessions {
			sb.WriteString(fmt.Sprintf("- %s (updated: %s)\n", s.SessionKey, s.UpdatedAt.Format("2006-01-02 15:04")))
		}
		return &ToolResult{Content: sb.String()}, nil
	}
	// read session messages
	limit := in.Limit
	if limit <= 0 {
		limit = 10
	}
	if limit > 50 {
		limit = 50
	}
	sess, err := t.sessionQuerier.GetOrCreate(in.SessionKey, "")
	if err != nil {
		return &ToolResult{Content: "Session not found: " + err.Error(), IsError: true}, nil
	}
	messages, err := t.sessionQuerier.GetMessages(sess.ID, limit)
	if err != nil {
		return &ToolResult{Content: "Failed to read messages: " + err.Error(), IsError: true}, nil
	}
	if len(messages) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No messages in session '%s'", in.SessionKey)}, nil
	}
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Messages from '%s' (last %d):\n\n", in.SessionKey, len(messages)))
	for _, m := range messages {
		content := m.Content
		if len(content) > 500 {
			content = content[:497] + "..."
		}
		sb.WriteString(fmt.Sprintf("[%s] %s\n\n", m.Role, content))
	}
	return &ToolResult{Content: sb.String()}, nil
}

// =============================================================================
// Profile handlers
// =============================================================================

func (t *BotTool) handleProfile(ctx context.Context, in BotInput) (*ToolResult, error) {
	switch in.Action {
	case "get":
		return t.profileGet(ctx)
	case "update":
		return t.profileUpdate(ctx, in)
	case "open_billing":
		return t.profileOpenBilling(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown profile action: %s", in.Action)}, nil
	}
}

func (t *BotTool) profileGet(ctx context.Context) (*ToolResult, error) {
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

func (t *BotTool) profileUpdate(ctx context.Context, in BotInput) (*ToolResult, error) {
	if in.Key == "" {
		return &ToolResult{Content: "Profile update requires key and value. Supported keys: name, role, emoji, creature, vibe, custom_personality, quiet_hours_start, quiet_hours_end"}, nil
	}
	if in.Value == "" && in.Key != "quiet_hours_start" && in.Key != "quiet_hours_end" {
		return &ToolResult{Content: "Profile update requires key and value."}, nil
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
		if in.Value != "" {
			parts := strings.SplitN(in.Value, ":", 2)
			if len(parts) != 2 {
				return &ToolResult{Content: "Quiet hours must be in HH:MM format or empty to clear"}, nil
			}
		}
		if in.Key == "quiet_hours_start" {
			params.QuietHoursStart = sql.NullString{String: in.Value, Valid: true}
		} else {
			params.QuietHoursEnd = sql.NullString{String: in.Value, Valid: true}
		}
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown profile key: %s", in.Key)}, nil
	}

	err := queries.UpdateAgentProfile(ctx, params)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to update %s: %v", in.Key, err)}, nil
	}

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

func (t *BotTool) profileOpenBilling(ctx context.Context) (*ToolResult, error) {
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
		return &ToolResult{Content: "No NeboLoop account connected. Connect via Settings > NeboLoop first."}, nil
	}
	token := profiles[0].ApiKey
	billingURL := "https://app.neboloop.com/billing?token=" + token
	openBrowserURL(billingURL)
	return &ToolResult{Content: "Opened NeboLoop billing page in your browser."}, nil
}

// =============================================================================
// Context handlers
// =============================================================================

func (t *BotTool) handleContext(_ context.Context, in BotInput) (*ToolResult, error) {
	// Context management requires runner integration — placeholder for now
	switch in.Action {
	case "reset":
		return &ToolResult{Content: "Context reset is handled automatically by the runner. Use session clear to clear conversation history."}, nil
	case "compact":
		return &ToolResult{Content: "Context compaction is handled automatically when the context window fills up."}, nil
	case "summary":
		return &ToolResult{Content: "Context summary is available via the steering pipeline. Check session status for message counts."}, nil
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown context action: %s", in.Action), IsError: true}, nil
	}
}

// =============================================================================
// Advisors handler
// =============================================================================

func (t *BotTool) handleAdvisors(ctx context.Context, in BotInput) (*ToolResult, error) {
	if t.advisorsTool == nil {
		return &ToolResult{Content: "Advisors not configured", IsError: true}, nil
	}
	// Delegate to the existing AdvisorsTool
	advisorsInput := AdvisorsInput{
		Task:      in.Task,
		SessionID: in.SessionID,
		Advisors:  in.AdvisorsList,
	}
	if advisorsInput.Task == "" {
		advisorsInput.Task = in.Prompt
	}
	data, _ := json.Marshal(advisorsInput)
	return t.advisorsTool.Execute(ctx, data)
}

// =============================================================================
// Vision handler
// =============================================================================

func (t *BotTool) handleVision(ctx context.Context, in BotInput) (*ToolResult, error) {
	if t.visionTool == nil {
		return &ToolResult{Content: "Vision not configured. Add an AI provider with vision support.", IsError: true}, nil
	}
	visionIn := visionInput{
		Image:  in.Image,
		Prompt: in.Prompt,
	}
	if visionIn.Prompt == "" {
		visionIn.Prompt = in.Text
	}
	data, _ := json.Marshal(visionIn)
	return t.visionTool.Execute(ctx, data)
}

// =============================================================================
// Ask handler
// =============================================================================

func (t *BotTool) handleAsk(ctx context.Context, in BotInput) (*ToolResult, error) {
	if t.askCallback == nil {
		return &ToolResult{Content: "Error: Interactive prompts require the web UI", IsError: true}, nil
	}

	prompt := in.Prompt
	if prompt == "" {
		prompt = in.Text
	}
	if prompt == "" {
		return &ToolResult{Content: "Error: 'prompt' or 'text' is required for ask", IsError: true}, nil
	}

	widgets := in.Widgets
	if len(widgets) == 0 {
		switch in.Action {
		case "confirm":
			widgets = []AskWidget{{Type: "confirm", Options: []string{"Yes", "No"}}}
		case "select":
			return &ToolResult{Content: "Error: 'widgets' with options required for select action", IsError: true}, nil
		default:
			widgets = []AskWidget{{Type: "confirm", Options: []string{"Yes", "No"}}}
		}
	}

	requestID := uuid.New().String()
	response, err := t.askCallback(ctx, requestID, prompt, widgets)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error waiting for user response: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: response}, nil
}

// Ensure unused imports are consumed.
var (
	_ = advisors.Loader{}
	_ = embeddings.SearchOptions{}
	_ = (*exec.Cmd)(nil)
	_ = runtime.GOOS
)
