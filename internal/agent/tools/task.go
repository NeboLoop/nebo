package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/agent/ai"
	"github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/orchestrator"
	"github.com/nebolabs/nebo/internal/agent/recovery"
	"github.com/nebolabs/nebo/internal/agent/session"
)

// registryAdapter wraps Registry to implement orchestrator.ToolExecutor
type registryAdapter struct {
	registry *Registry
}

func (a *registryAdapter) Execute(ctx context.Context, call *ai.ToolCall) *orchestrator.ToolExecResult {
	result := a.registry.Execute(ctx, call)
	return &orchestrator.ToolExecResult{
		Content: result.Content,
		IsError: result.IsError,
	}
}

func (a *registryAdapter) List() []ai.ToolDefinition {
	return a.registry.List()
}

// TaskTool spawns sub-agents to handle complex, multi-step tasks
type TaskTool struct {
	orchestrator *orchestrator.Orchestrator
}

// TaskInput defines the input for the task tool
type TaskInput struct {
	// Description is a short (3-5 word) description of what the agent will do
	Description string `json:"description"`

	// Prompt is the detailed task for the agent to perform
	Prompt string `json:"prompt"`

	// Wait determines if we should wait for the agent to complete (default: true)
	Wait *bool `json:"wait,omitempty"`

	// Timeout in seconds (default: 300 = 5 minutes)
	Timeout int `json:"timeout,omitempty"`

	// AgentType hints at what kind of task this is (optional)
	// Values: "explore" (codebase exploration), "plan" (planning), "general" (default)
	AgentType string `json:"agent_type,omitempty"`
}

// NewTaskTool creates a new task tool
// Note: The orchestrator must be set later via SetOrchestrator
func NewTaskTool() *TaskTool {
	return &TaskTool{}
}

// SetOrchestrator sets the orchestrator for spawning sub-agents
func (t *TaskTool) SetOrchestrator(orch *orchestrator.Orchestrator) {
	t.orchestrator = orch
}

// CreateOrchestrator creates and sets a new orchestrator
func (t *TaskTool) CreateOrchestrator(cfg *config.Config, sessions *session.Manager, providers []ai.Provider, registry *Registry) {
	adapter := &registryAdapter{registry: registry}
	t.orchestrator = orchestrator.NewOrchestrator(cfg, sessions, providers, adapter)
}

// GetOrchestrator returns the orchestrator for sharing with other tools
func (t *TaskTool) GetOrchestrator() *orchestrator.Orchestrator {
	return t.orchestrator
}

// SetRecoveryManager sets the recovery manager for subagent persistence
// This enables subagent runs to survive agent restarts
func (t *TaskTool) SetRecoveryManager(mgr *recovery.Manager) {
	if t.orchestrator != nil {
		t.orchestrator.SetRecoveryManager(mgr)
	}
}

// RecoverSubagents restores pending subagent tasks from the database
// Call this after SetRecoveryManager during agent startup
func (t *TaskTool) RecoverSubagents(ctx context.Context) (int, error) {
	if t.orchestrator == nil {
		return 0, nil
	}
	return t.orchestrator.RecoverAgents(ctx)
}

// Name returns the tool name
func (t *TaskTool) Name() string {
	return "task"
}

// Description returns the tool description
func (t *TaskTool) Description() string {
	return "Spawn a sub-agent to handle complex, multi-step tasks autonomously. Use this for tasks that require multiple tool calls, exploration, or independent work streams."
}

// Schema returns the JSON schema for the tool
func (t *TaskTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"description": {
				"type": "string",
				"description": "A short (3-5 word) description of the task"
			},
			"prompt": {
				"type": "string",
				"description": "The detailed task for the sub-agent to perform"
			},
			"wait": {
				"type": "boolean",
				"description": "Wait for the agent to complete before returning (default: true)",
				"default": true
			},
			"timeout": {
				"type": "integer",
				"description": "Timeout in seconds (default: 300 = 5 minutes)",
				"default": 300
			},
			"agent_type": {
				"type": "string",
				"description": "Type of agent: 'explore' (codebase exploration), 'plan' (planning), 'general' (default)",
				"enum": ["explore", "plan", "general"]
			}
		},
		"required": ["description", "prompt"]
	}`)
}

// RequiresApproval returns false - sub-agents inherit parent's policy
func (t *TaskTool) RequiresApproval() bool {
	return false
}

// Execute spawns a sub-agent to perform the task
func (t *TaskTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.orchestrator == nil {
		return &ToolResult{
			Content: "Error: Task tool not initialized. Orchestrator not configured.",
			IsError: true,
		}, nil
	}

	var params TaskInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid input: %v", err),
			IsError: true,
		}, nil
	}

	if params.Prompt == "" {
		return &ToolResult{
			Content: "Error: 'prompt' is required",
			IsError: true,
		}, nil
	}

	if params.Description == "" {
		params.Description = truncateForDescription(params.Prompt)
	}

	// Default wait to true
	wait := true
	if params.Wait != nil {
		wait = *params.Wait
	}

	// Default timeout to 5 minutes
	timeout := 300
	if params.Timeout > 0 {
		timeout = params.Timeout
	}

	// Build system prompt based on agent type
	systemPrompt := buildAgentSystemPrompt(params.AgentType, params.Prompt)

	// Spawn the sub-agent
	agent, err := t.orchestrator.Spawn(ctx, &orchestrator.SpawnRequest{
		Task:         params.Prompt,
		Description:  params.Description,
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
		// Return the result
		var result strings.Builder
		result.WriteString(fmt.Sprintf("Sub-agent completed: %s\n", params.Description))
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

	// Return immediately with agent ID for tracking
	return &ToolResult{
		Content: fmt.Sprintf("Sub-agent spawned: %s\nAgent ID: %s\nDescription: %s\n\nThe agent is running in the background. Use 'agent_status' tool with this ID to check progress.",
			params.Description, agent.ID, params.Prompt),
		IsError: false,
	}, nil
}

// buildAgentSystemPrompt creates a system prompt based on agent type
func buildAgentSystemPrompt(agentType, task string) string {
	base := `You are a focused sub-agent working on a specific task. Complete the task efficiently and report your results.

Guidelines:
1. Focus ONLY on the assigned task
2. Use tools as needed to accomplish the task
3. Work independently - do not ask for clarification
4. When complete, provide a clear summary of what was done
5. If you encounter errors, try to resolve them

`

	switch agentType {
	case "explore":
		return base + `You are an EXPLORATION agent. Your job is to:
- Search through codebases to find relevant files and code
- Understand patterns and architecture
- Report findings clearly
- Do NOT modify any files - only read and analyze

Task: ` + task

	case "plan":
		return base + `You are a PLANNING agent. Your job is to:
- Analyze the task and break it into steps
- Identify files that need to be modified
- Consider edge cases and potential issues
- Create a clear, actionable plan
- Do NOT implement the plan - only create it

Task: ` + task

	default:
		return base + `Task: ` + task
	}
}

// truncateForDescription creates a short description from a prompt
func truncateForDescription(prompt string) string {
	// Take first line or first 50 chars
	lines := strings.SplitN(prompt, "\n", 2)
	desc := lines[0]
	if len(desc) > 50 {
		desc = desc[:47] + "..."
	}
	return desc
}

// AgentStatusTool checks the status of a running sub-agent
type AgentStatusTool struct {
	orchestrator *orchestrator.Orchestrator
}

// NewAgentStatusTool creates a new agent status tool
func NewAgentStatusTool() *AgentStatusTool {
	return &AgentStatusTool{}
}

// SetOrchestrator sets the orchestrator
func (t *AgentStatusTool) SetOrchestrator(orch *orchestrator.Orchestrator) {
	t.orchestrator = orch
}

// Name returns the tool name
func (t *AgentStatusTool) Name() string {
	return "agent_status"
}

// Description returns the tool description
func (t *AgentStatusTool) Description() string {
	return "Check the status of a running sub-agent or list all sub-agents"
}

// Schema returns the JSON schema
func (t *AgentStatusTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"agent_id": {
				"type": "string",
				"description": "The ID of the agent to check. Omit to list all agents."
			},
			"action": {
				"type": "string",
				"description": "Action to perform: 'status' (default), 'list', 'cancel'",
				"enum": ["status", "list", "cancel"]
			}
		}
	}`)
}

// RequiresApproval returns false
func (t *AgentStatusTool) RequiresApproval() bool {
	return false
}

// Execute checks agent status
func (t *AgentStatusTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.orchestrator == nil {
		return &ToolResult{
			Content: "Error: Orchestrator not configured",
			IsError: true,
		}, nil
	}

	var params struct {
		AgentID string `json:"agent_id"`
		Action  string `json:"action"`
	}
	json.Unmarshal(input, &params)

	if params.Action == "" {
		params.Action = "status"
	}

	switch params.Action {
	case "list":
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

	case "cancel":
		if params.AgentID == "" {
			return &ToolResult{Content: "Error: agent_id required for cancel", IsError: true}, nil
		}
		if err := t.orchestrator.CancelAgent(params.AgentID); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Error: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Agent %s cancelled", params.AgentID)}, nil

	default: // status
		if params.AgentID == "" {
			return &ToolResult{Content: "Error: agent_id required for status check", IsError: true}, nil
		}

		agent, exists := t.orchestrator.GetAgent(params.AgentID)
		if !exists {
			return &ToolResult{Content: fmt.Sprintf("Agent not found: %s", params.AgentID), IsError: true}, nil
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
}
