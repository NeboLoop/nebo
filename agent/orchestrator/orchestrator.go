package orchestrator

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	"nebo/agent/ai"
	"nebo/agent/config"
	"nebo/agent/session"
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
	ID          string
	Task        string
	Description string
	Lane        string // Lane this agent runs in (default: LaneSubagent)
	Status      AgentStatus
	Result      string
	Error       error
	StartedAt   time.Time
	CompletedAt time.Time
	Events      []ai.StreamEvent
	cancel      context.CancelFunc
}

// Lane constants for task queue
const (
	LaneMain     = "main"
	LaneCron     = "cron"
	LaneSubagent = "subagent"
	LaneNested   = "nested"
)

// Orchestrator manages multiple concurrent sub-agents
type Orchestrator struct {
	mu        sync.RWMutex
	agents    map[string]*SubAgent
	sessions  *session.Manager
	providers []ai.Provider
	tools     ToolExecutor
	config    *config.Config

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
		maxConcurrent: 0,  // 0 = unlimited sub-agents
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

	agent := &SubAgent{
		ID:          agentID,
		Task:        req.Task,
		Description: req.Description,
		Lane:        lane,
		Status:      StatusPending,
		StartedAt:   time.Now(),
		cancel:      cancel,
	}

	o.agents[agentID] = agent
	o.mu.Unlock()

	// Start the agent in a goroutine
	go o.runAgent(agentCtx, agent, req)

	// If wait requested, block until completion
	if req.Wait {
		return o.waitForAgent(ctx, agentID)
	}

	return agent, nil
}

// runAgent executes the sub-agent's task
func (o *Orchestrator) runAgent(ctx context.Context, agent *SubAgent, req *SpawnRequest) {
	// Update status
	o.mu.Lock()
	agent.Status = StatusRunning
	o.mu.Unlock()

	defer func() {
		o.mu.Lock()
		agent.CompletedAt = time.Now()
		if agent.Status == StatusRunning {
			if agent.Error != nil {
				agent.Status = StatusFailed
			} else {
				agent.Status = StatusCompleted
			}
		}
		o.mu.Unlock()

		// Send result
		o.results <- AgentResult{
			AgentID: agent.ID,
			Success: agent.Status == StatusCompleted,
			Result:  agent.Result,
			Error:   agent.Error,
		}
	}()

	// Create a unique session for this sub-agent
	sessionKey := fmt.Sprintf("subagent-%s", agent.ID)
	sess, err := o.sessions.GetOrCreate(sessionKey)
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
	result, err := o.executeLoop(ctx, sess.ID, systemPrompt, agent)
	if err != nil {
		agent.Error = err
		return
	}

	agent.Result = result
}

// executeLoop runs the agentic loop for a sub-agent
func (o *Orchestrator) executeLoop(ctx context.Context, sessionID, systemPrompt string, agent *SubAgent) (string, error) {
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
	defer o.mu.Unlock()

	agent, exists := o.agents[agentID]
	if !exists {
		return fmt.Errorf("agent not found: %s", agentID)
	}

	if agent.Status != StatusRunning && agent.Status != StatusPending {
		return fmt.Errorf("agent is not running: %s", agent.Status)
	}

	agent.Status = StatusCancelled
	if agent.cancel != nil {
		agent.cancel()
	}

	return nil
}

// Results returns the results channel for monitoring
func (o *Orchestrator) Results() <-chan AgentResult {
	return o.results
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
