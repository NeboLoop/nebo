package orchestrator

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/recovery"
	"github.com/neboloop/nebo/internal/agent/session"
	"github.com/neboloop/nebo/internal/webview"
)

// ToolExecutor is an interface for executing tools (avoids circular import)
type ToolExecutor interface {
	Execute(ctx context.Context, call *ai.ToolCall) *ToolExecResult
	List() []ai.ToolDefinition
}

// ToolExecResult matches tools.ToolResult
type ToolExecResult struct {
	Content string
	IsError bool
}

// AgentStatus represents the current state of a sub-agent
type AgentStatus string

const (
	StatusPending   AgentStatus = "pending"
	StatusRunning   AgentStatus = "running"
	StatusCompleted AgentStatus = "completed"
	StatusFailed    AgentStatus = "failed"
	StatusCancelled AgentStatus = "cancelled"
)

// SubAgent represents a spawned sub-agent
type SubAgent struct {
	ID            string
	TaskID        string // recovery.PendingTask ID for persistence
	Task          string
	Description   string
	Lane          string // Lane this agent runs in (default: LaneSubagent)
	ModelOverride string // Override model for this sub-agent
	Status        AgentStatus
	Result        string
	Error         error
	StartedAt     time.Time
	CompletedAt   time.Time
	Events        []ai.StreamEvent
	cancel        context.CancelFunc
}

// Lane constants for task queue
const (
	LaneMain      = "main"
	LaneEvents    = "events"    // Scheduled/triggered tasks (renamed from cron)
	LaneSubagent  = "subagent"
	LaneNested    = "nested"
	LaneHeartbeat = "heartbeat" // Proactive heartbeat ticks
	LaneComm      = "comm"      // Inter-agent communication messages
)

// Orchestrator manages multiple concurrent sub-agents
type Orchestrator struct {
	mu        sync.RWMutex
	agents    map[string]*SubAgent
	sessions  *session.Manager
	providers []ai.Provider
	tools     ToolExecutor
	config    *config.Config
	recovery  *recovery.Manager // For persisting subagent runs across restarts

	// Limits
	maxConcurrent int
	maxPerParent  int

	// Channels for coordination
	results chan AgentResult
}

// GetMaxConcurrent returns the max concurrent sub-agents limit
func (o *Orchestrator) GetMaxConcurrent() int {
	return o.maxConcurrent
}

// SetMaxConcurrent updates the max concurrent sub-agents limit
func (o *Orchestrator) SetMaxConcurrent(max int) {
	if max < 1 {
		max = 1
	}
	o.mu.Lock()
	o.maxConcurrent = max
	o.mu.Unlock()
}

// SetRecoveryManager sets the recovery manager for persisting subagent runs
func (o *Orchestrator) SetRecoveryManager(mgr *recovery.Manager) {
	o.mu.Lock()
	o.recovery = mgr
	o.mu.Unlock()
}

// RunningCount returns the number of currently running sub-agents
func (o *Orchestrator) RunningCount() int {
	o.mu.RLock()
	defer o.mu.RUnlock()
	count := 0
	for _, agent := range o.agents {
		if agent.Status == StatusRunning {
			count++
		}
	}
	return count
}

// AgentResult is sent when a sub-agent completes
type AgentResult struct {
	AgentID string
	Success bool
	Result  string
	Error   error
}

// NewOrchestrator creates a new orchestrator
func NewOrchestrator(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, toolExecutor ToolExecutor) *Orchestrator {
	return &Orchestrator{
		agents:        make(map[string]*SubAgent),
		sessions:      sessions,
		providers:     providers,
		tools:         toolExecutor,
		config:        cfg,
		maxConcurrent: 5,  // Max 5 concurrent sub-agents
		maxPerParent:  0,  // 0 = unlimited per parent session
		results:       make(chan AgentResult, 100),
	}
}

// SpawnRequest contains parameters for spawning a sub-agent
type SpawnRequest struct {
	ParentSessionKey string // Parent session for context inheritance
	Task             string // Task description for the sub-agent
	Description      string // Short description for tracking
	Lane             string // Lane to run in (default: LaneSubagent)
	Wait             bool   // Wait for completion before returning
	Timeout          time.Duration
	SystemPrompt     string // Optional custom system prompt
	ModelOverride    string // Override model for this sub-agent (e.g., "anthropic/claude-haiku-4-5")
}

// Spawn creates and starts a new sub-agent
func (o *Orchestrator) Spawn(ctx context.Context, req *SpawnRequest) (*SubAgent, error) {
	o.mu.Lock()

	// Check limits (0 = unlimited)
	if o.maxConcurrent > 0 {
		runningCount := 0
		for _, agent := range o.agents {
			if agent.Status == StatusRunning {
				runningCount++
			}
		}
		if runningCount >= o.maxConcurrent {
			o.mu.Unlock()
			return nil, fmt.Errorf("maximum concurrent agents reached (%d)", o.maxConcurrent)
		}
	}

	// Generate unique ID
	agentID := fmt.Sprintf("agent-%d-%d", time.Now().UnixNano(), len(o.agents))

	// Create sub-agent
	agentCtx, cancel := context.WithCancel(ctx)
	if req.Timeout > 0 {
		agentCtx, cancel = context.WithTimeout(ctx, req.Timeout)
	}

	lane := req.Lane
	if lane == "" {
		lane = LaneSubagent
	}

	// Generate session key for this sub-agent
	sessionKey := fmt.Sprintf("subagent-%s", agentID)

	agent := &SubAgent{
		ID:            agentID,
		Task:          req.Task,
		Description:   req.Description,
		Lane:          lane,
		ModelOverride: req.ModelOverride,
		Status:        StatusPending,
		StartedAt:     time.Now(),
		cancel:        cancel,
	}

	// Persist to database BEFORE spawning to survive restarts
	if o.recovery != nil {
		task := &recovery.PendingTask{
			TaskType:     recovery.TaskTypeSubagent,
			Status:       recovery.StatusPending,
			SessionKey:   sessionKey,
			Prompt:       req.Task,
			SystemPrompt: req.SystemPrompt,
			Description:  req.Description,
			Lane:         lane,
		}
		if err := o.recovery.CreateTask(ctx, task); err != nil {
			o.mu.Unlock()
			cancel() // Clean up context
			return nil, fmt.Errorf("failed to persist subagent task: %w", err)
		}
		agent.TaskID = task.ID
	}

	o.agents[agentID] = agent
	o.mu.Unlock()

	// Start the agent in a goroutine
	go o.runAgent(agentCtx, agent, req, sessionKey)

	// If wait requested, block until completion
	// NOTE: Use Background context, not parent context. This decouples the wait from
	// parent cancellation — we want to wait for the agent to finish even if the parent
	// context is cancelled (e.g., user switches conversations, closes UI, etc.)
	if req.Wait {
		return o.waitForAgent(context.Background(), agentID)
	}

	return agent, nil
}

// runAgent executes the sub-agent's task
func (o *Orchestrator) runAgent(ctx context.Context, agent *SubAgent, req *SpawnRequest, sessionKey string) {
	// Panic recovery — a sub-agent must never crash the process
	defer func() {
		if r := recover(); r != nil {
			fmt.Printf("[Orchestrator] PANIC in sub-agent %s (%s): %v\n", agent.ID, agent.Description, r)
			o.mu.Lock()
			agent.Status = StatusFailed
			agent.Error = fmt.Errorf("panic: %v", r)
			agent.CompletedAt = time.Now()
			o.mu.Unlock()

			// Mark failed in recovery DB
			if o.recovery != nil && agent.TaskID != "" {
				if err := o.recovery.MarkFailed(context.Background(), agent.TaskID, fmt.Sprintf("panic: %v", r)); err != nil {
					fmt.Printf("[Orchestrator] Warning: failed to mark panicked task as failed: %v\n", err)
				}
			}

			// Still send result so waiters don't hang
			o.results <- AgentResult{
				AgentID: agent.ID,
				Success: false,
				Error:   agent.Error,
			}
		}
	}()

	fmt.Printf("[Orchestrator] Starting sub-agent %s: %s\n", agent.ID, agent.Description)

	// Update status
	o.mu.Lock()
	agent.Status = StatusRunning
	o.mu.Unlock()

	// Mark task as running in recovery manager
	if o.recovery != nil && agent.TaskID != "" {
		if err := o.recovery.MarkRunning(ctx, agent.TaskID); err != nil {
			fmt.Printf("[Orchestrator] Warning: failed to mark task running: %v\n", err)
		}
	}

	defer func() {
		// Close any native browser windows opened by this sub-agent
		if closed := webview.GetManager().CloseWindowsByOwner(sessionKey); closed > 0 {
			fmt.Printf("[Orchestrator] Cleaned up %d browser window(s) for sub-agent %s\n", closed, agent.ID)
		}

		o.mu.Lock()
		agent.CompletedAt = time.Now()
		if agent.Status == StatusRunning {
			if agent.Error != nil {
				agent.Status = StatusFailed
			} else {
				agent.Status = StatusCompleted
			}
		}
		finalStatus := agent.Status
		finalError := agent.Error
		o.mu.Unlock()

		fmt.Printf("[Orchestrator] Sub-agent %s finished: status=%s\n", agent.ID, finalStatus)

		// Update recovery status to persist completion
		// Use Background context because the agent's ctx may be cancelled
		if o.recovery != nil && agent.TaskID != "" {
			dbCtx := context.Background()
			if finalStatus == StatusCompleted {
				if err := o.recovery.MarkCompleted(dbCtx, agent.TaskID); err != nil {
					fmt.Printf("[Orchestrator] Warning: failed to mark task completed: %v\n", err)
				}
			} else if finalStatus == StatusFailed && finalError != nil {
				if err := o.recovery.MarkFailed(dbCtx, agent.TaskID, finalError.Error()); err != nil {
					fmt.Printf("[Orchestrator] Warning: failed to mark task failed: %v\n", err)
				}
			} else if finalStatus == StatusCancelled {
				if err := o.recovery.MarkFailed(dbCtx, agent.TaskID, "cancelled"); err != nil {
					fmt.Printf("[Orchestrator] Warning: failed to mark cancelled task: %v\n", err)
				}
			}
		}

		// Send result
		o.results <- AgentResult{
			AgentID: agent.ID,
			Success: agent.Status == StatusCompleted,
			Result:  agent.Result,
			Error:   agent.Error,
		}
	}()

	// Use the session key passed in (already includes agent ID)
	// Subagent sessions use agent scope (empty userID) since they're task-specific
	sess, err := o.sessions.GetOrCreate(sessionKey, "")
	if err != nil {
		agent.Error = fmt.Errorf("failed to create session: %w", err)
		return
	}

	// Build system prompt for sub-agent
	systemPrompt := req.SystemPrompt
	if systemPrompt == "" {
		systemPrompt = o.buildSubAgentPrompt(req.Task)
	}

	// Add the task as user message
	err = o.sessions.AppendMessage(sess.ID, session.Message{
		SessionID: sess.ID,
		Role:      "user",
		Content:   req.Task,
	})
	if err != nil {
		agent.Error = fmt.Errorf("failed to save task message: %w", err)
		return
	}

	// Run the agentic loop
	result, err := o.executeLoop(ctx, sess.ID, systemPrompt, agent.ModelOverride, agent)
	if err != nil {
		agent.Error = err
		return
	}

	agent.Result = result
}

// executeLoop runs the agentic loop for a sub-agent
func (o *Orchestrator) executeLoop(ctx context.Context, sessionID, systemPrompt, modelOverride string, agent *SubAgent) (string, error) {
	if len(o.providers) == 0 {
		return "", fmt.Errorf("no providers configured")
	}

	maxIterations := o.config.MaxIterations
	if maxIterations <= 0 {
		maxIterations = 50 // Lower limit for sub-agents
	}

	var finalResult strings.Builder

	for iteration := 0; iteration < maxIterations; iteration++ {
		select {
		case <-ctx.Done():
			o.mu.Lock()
			agent.Status = StatusCancelled
			o.mu.Unlock()
			return finalResult.String(), ctx.Err()
		default:
		}

		// Get session messages
		messages, err := o.sessions.GetMessages(sessionID, o.config.MaxContext)
		if err != nil {
			return "", err
		}

		// Try providers in order
		provider := o.providers[0]
		events, err := provider.Stream(ctx, &ai.ChatRequest{
			Messages: messages,
			Tools:    o.tools.List(),
			System:   systemPrompt,
			Model:    modelOverride,
		})

		if err != nil {
			return "", err
		}

		// Process events
		hasToolCalls := false
		var assistantContent strings.Builder
		var toolCalls []session.ToolCall

		for event := range events {
			// Store events for tracking
			o.mu.Lock()
			agent.Events = append(agent.Events, event)
			o.mu.Unlock()

			switch event.Type {
			case ai.EventTypeText:
				assistantContent.WriteString(event.Text)
				finalResult.WriteString(event.Text)

			case ai.EventTypeToolCall:
				hasToolCalls = true
				toolCalls = append(toolCalls, session.ToolCall{
					ID:    event.ToolCall.ID,
					Name:  event.ToolCall.Name,
					Input: event.ToolCall.Input,
				})

			case ai.EventTypeError:
				return finalResult.String(), event.Error
			}
		}

		// Save assistant message
		if assistantContent.Len() > 0 || len(toolCalls) > 0 {
			var toolCallsJSON []byte
			if len(toolCalls) > 0 {
				toolCallsJSON, _ = json.Marshal(toolCalls)
			}

			o.sessions.AppendMessage(sessionID, session.Message{
				SessionID: sessionID,
				Role:      "assistant",
				Content:   assistantContent.String(),
				ToolCalls: toolCallsJSON,
			})
		}

		// Execute tool calls
		if hasToolCalls {
			var toolResults []session.ToolResult

			for _, tc := range toolCalls {
				result := o.tools.Execute(ctx, &ai.ToolCall{
					ID:    tc.ID,
					Name:  tc.Name,
					Input: tc.Input,
				})

				toolResults = append(toolResults, session.ToolResult{
					ToolCallID: tc.ID,
					Content:    result.Content,
					IsError:    result.IsError,
				})
			}

			// Save tool results
			toolResultsJSON, _ := json.Marshal(toolResults)
			o.sessions.AppendMessage(sessionID, session.Message{
				SessionID:   sessionID,
				Role:        "tool",
				ToolResults: toolResultsJSON,
			})

			continue
		}

		// No tool calls - task complete
		break
	}

	return finalResult.String(), nil
}

// buildSubAgentPrompt creates a system prompt for sub-agents
func (o *Orchestrator) buildSubAgentPrompt(task string) string {
	return fmt.Sprintf(`You are a focused sub-agent working on a specific task.

Your task: %s

Guidelines:
1. Focus ONLY on the assigned task
2. Work efficiently and complete the task as quickly as possible
3. Use tools as needed to accomplish the task
4. When the task is complete, provide a clear summary of what was done
5. Do not ask for clarification - make reasonable assumptions
6. Do not engage in conversation - just complete the task

When you have completed the task, provide your final response summarizing what was accomplished.`, task)
}

// waitForAgent blocks until the agent completes
func (o *Orchestrator) waitForAgent(ctx context.Context, agentID string) (*SubAgent, error) {
	for {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case result := <-o.results:
			if result.AgentID == agentID {
				o.mu.RLock()
				agent := o.agents[agentID]
				o.mu.RUnlock()
				return agent, result.Error
			}
			// Put back results for other agents
			go func(r AgentResult) { o.results <- r }(result)
		case <-time.After(100 * time.Millisecond):
			// Check if agent is done
			o.mu.RLock()
			agent, exists := o.agents[agentID]
			o.mu.RUnlock()
			if !exists {
				return nil, fmt.Errorf("agent not found: %s", agentID)
			}
			if agent.Status == StatusCompleted || agent.Status == StatusFailed || agent.Status == StatusCancelled {
				return agent, agent.Error
			}
		}
	}
}

// GetAgent returns a sub-agent by ID
func (o *Orchestrator) GetAgent(agentID string) (*SubAgent, bool) {
	o.mu.RLock()
	defer o.mu.RUnlock()
	agent, exists := o.agents[agentID]
	return agent, exists
}

// ListAgents returns all sub-agents
func (o *Orchestrator) ListAgents() []*SubAgent {
	o.mu.RLock()
	defer o.mu.RUnlock()

	agents := make([]*SubAgent, 0, len(o.agents))
	for _, agent := range o.agents {
		agents = append(agents, agent)
	}
	return agents
}

// CancelAgent cancels a running sub-agent
func (o *Orchestrator) CancelAgent(agentID string) error {
	o.mu.Lock()

	agent, exists := o.agents[agentID]
	if !exists {
		o.mu.Unlock()
		return fmt.Errorf("agent not found: %s", agentID)
	}

	if agent.Status != StatusRunning && agent.Status != StatusPending {
		o.mu.Unlock()
		return fmt.Errorf("agent is not running: %s", agent.Status)
	}

	fmt.Printf("[Orchestrator] Cancelling sub-agent %s (%s)\n", agentID, agent.Description)
	agent.Status = StatusCancelled
	agent.CompletedAt = time.Now()
	taskID := agent.TaskID
	cancelFn := agent.cancel
	o.mu.Unlock()

	// Mark cancelled in recovery DB so it won't be recovered on next restart
	if o.recovery != nil && taskID != "" {
		if err := o.recovery.MarkCancelled(context.Background(), taskID); err != nil {
			fmt.Printf("[Orchestrator] Warning: failed to mark task %s as cancelled in DB: %v\n", taskID, err)
		}
	}

	// Cancel the context last — this triggers the goroutine to wind down
	if cancelFn != nil {
		cancelFn()
	}

	return nil
}

// Results returns the results channel for monitoring
func (o *Orchestrator) Results() <-chan AgentResult {
	return o.results
}

// Shutdown cancels all running/pending sub-agents and marks them cancelled in
// the recovery DB so they won't be re-spawned on the next startup.
func (o *Orchestrator) Shutdown(ctx context.Context) {
	o.mu.Lock()
	var running []*SubAgent
	for _, agent := range o.agents {
		if agent.Status == StatusRunning || agent.Status == StatusPending {
			running = append(running, agent)
		}
	}
	o.mu.Unlock()

	if len(running) == 0 {
		return
	}

	fmt.Printf("[Orchestrator] Shutting down %d sub-agents\n", len(running))

	for _, agent := range running {
		o.mu.Lock()
		agent.Status = StatusCancelled
		agent.CompletedAt = time.Now()
		cancelFn := agent.cancel
		taskID := agent.TaskID
		o.mu.Unlock()

		// Mark cancelled in recovery DB so it won't be recovered
		if o.recovery != nil && taskID != "" {
			if err := o.recovery.MarkCancelled(ctx, taskID); err != nil {
				fmt.Printf("[Orchestrator] Warning: failed to mark task %s cancelled: %v\n", taskID, err)
			}
		}

		// Cancel the context to stop the goroutine
		if cancelFn != nil {
			cancelFn()
		}
	}
}

// Cleanup removes completed agents older than the given duration
func (o *Orchestrator) Cleanup(maxAge time.Duration) int {
	o.mu.Lock()
	defer o.mu.Unlock()

	cutoff := time.Now().Add(-maxAge)
	removed := 0

	for id, agent := range o.agents {
		if agent.Status != StatusRunning && agent.Status != StatusPending {
			if agent.CompletedAt.Before(cutoff) {
				delete(o.agents, id)
				removed++
			}
		}
	}

	return removed
}

// RecoverAgents restores pending subagent tasks from the database after restart.
// This should be called after SetRecoveryManager during agent startup.
//
// Recovery rules:
// 1. Tasks with session messages containing assistant responses are considered complete.
// 2. Tasks older than maxRecoveryAge are marked stale and skipped.
// 3. Tasks that have exhausted retry attempts are marked failed.
// 4. Only genuinely incomplete, recent tasks are re-spawned.
func (o *Orchestrator) RecoverAgents(ctx context.Context) (int, error) {
	if o.recovery == nil {
		return 0, nil
	}

	const maxRecoveryAge = 2 * time.Hour // Don't recover tasks older than this

	tasks, err := o.recovery.GetRecoverableTasks(ctx)
	if err != nil {
		return 0, fmt.Errorf("failed to get recoverable tasks: %w", err)
	}

	fmt.Printf("[Recovery] Found %d recoverable tasks\n", len(tasks))

	recovered := 0
	for _, task := range tasks {
		// Only recover subagent tasks
		if task.TaskType != recovery.TaskTypeSubagent {
			continue
		}

		taskAge := time.Since(task.CreatedAt)
		fmt.Printf("[Recovery] Evaluating task %s (%s): age=%s, attempts=%d/%d, status=%s\n",
			task.ID[:8], task.Description, taskAge.Round(time.Second), task.Attempts, task.MaxAttempts, task.Status)

		// Rule 1: Too old — mark stale and skip
		if taskAge > maxRecoveryAge {
			fmt.Printf("[Recovery] Task %s is too old (%s > %s), marking as failed\n",
				task.ID[:8], taskAge.Round(time.Second), maxRecoveryAge)
			if err := o.recovery.MarkFailed(ctx, task.ID, "stale: exceeded max recovery age"); err != nil {
				fmt.Printf("[Recovery] Warning: failed to mark stale task %s: %v\n", task.ID[:8], err)
			}
			continue
		}

		// Rule 2: Too many attempts — mark failed
		if task.Attempts >= task.MaxAttempts {
			fmt.Printf("[Recovery] Task %s exhausted retries (%d/%d), marking failed\n",
				task.ID[:8], task.Attempts, task.MaxAttempts)
			if err := o.recovery.MarkFailed(ctx, task.ID, "exhausted retry attempts"); err != nil {
				fmt.Printf("[Recovery] Warning: failed to mark exhausted task %s: %v\n", task.ID[:8], err)
			}
			continue
		}

		// Rule 3: Check if session shows completion (assistant produced output)
		completed, err := o.recovery.CheckTaskCompletion(ctx, task)
		if err != nil {
			fmt.Printf("[Recovery] Warning: completion check failed for task %s: %v\n", task.ID[:8], err)
			// On error, err on the side of NOT re-running
			if err := o.recovery.MarkFailed(ctx, task.ID, fmt.Sprintf("completion check error: %v", err)); err != nil {
				fmt.Printf("[Recovery] Warning: failed to mark errored task %s: %v\n", task.ID[:8], err)
			}
			continue
		}
		if completed {
			fmt.Printf("[Recovery] Task %s already completed (found in session), marking done\n", task.ID[:8])
			if err := o.recovery.MarkCompleted(ctx, task.ID); err != nil {
				fmt.Printf("[Recovery] Warning: failed to mark completed task %s: %v\n", task.ID[:8], err)
			}
			continue
		}

		// Task is genuinely incomplete and recent — re-spawn it
		fmt.Printf("[Recovery] Re-spawning sub-agent for task %s: %s\n", task.ID[:8], task.Description)

		req := &SpawnRequest{
			Task:         task.Prompt,
			Description:  task.Description,
			Lane:         task.Lane,
			SystemPrompt: task.SystemPrompt,
			Wait:         false,
		}

		o.mu.Lock()

		agentID := fmt.Sprintf("agent-recovered-%s", task.ID[:8])
		agentCtx, cancel := context.WithCancel(ctx)

		lane := task.Lane
		if lane == "" {
			lane = LaneSubagent
		}

		agent := &SubAgent{
			ID:          agentID,
			TaskID:      task.ID,
			Task:        task.Prompt,
			Description: task.Description,
			Lane:        lane,
			Status:      StatusPending,
			StartedAt:   time.Now(),
			cancel:      cancel,
		}

		o.agents[agentID] = agent
		o.mu.Unlock()

		go o.runAgent(agentCtx, agent, req, task.SessionKey)
		recovered++
	}

	fmt.Printf("[Recovery] Recovered %d sub-agents out of %d candidates\n", recovered, len(tasks))
	return recovered, nil
}

