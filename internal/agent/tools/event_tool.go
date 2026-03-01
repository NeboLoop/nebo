package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// EventTool provides scheduling capabilities: cron jobs, reminders, and time-based automation.
// Flat domain (no resources) — actions map directly.
type EventTool struct {
	scheduler Scheduler
}

// NewEventTool creates a new event domain tool.
func NewEventTool(scheduler Scheduler) *EventTool {
	return &EventTool{scheduler: scheduler}
}

// SetAgentCallback sets the callback for agent task execution in cron.
func (t *EventTool) SetAgentCallback(cb AgentTaskCallback) {
	if t.scheduler == nil {
		return
	}
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

// GetScheduler returns the underlying scheduler for direct access.
func (t *EventTool) GetScheduler() Scheduler {
	return t.scheduler
}

// Close cleans up the scheduler.
func (t *EventTool) Close() error {
	if t.scheduler != nil {
		t.scheduler.Close()
	}
	return nil
}

func (t *EventTool) Name() string   { return "event" }
func (t *EventTool) Domain() string { return "event" }

func (t *EventTool) Resources() []string {
	return nil // flat domain
}

func (t *EventTool) ActionsFor(_ string) []string {
	return []string{"create", "list", "delete", "pause", "resume", "run", "history"}
}

func (t *EventTool) RequiresApproval() bool { return false }

// eventResources uses empty-string key for flat domain.
var eventResources = map[string]ResourceConfig{
	"": {Name: "", Actions: []string{"create", "list", "delete", "pause", "resume", "run", "history"}, Description: "Schedule reminders, cron jobs, and time-based automation"},
}

// eventResourceAliases maps common synonyms to the canonical (empty) resource.
var eventResourceAliases = map[string]string{
	"reminder":  "",
	"reminders": "",
	"routine":   "",
	"routines":  "",
	"remind":    "",
	"schedule":  "",
	"schedules": "",
	"job":       "",
	"jobs":      "",
	"cron":      "",
	"event":     "",
	"events":    "",
	"calendar":  "",
}

func (t *EventTool) Description() string {
	return `Schedule reminders, cron jobs, and time-based automation.

Actions: create, list, delete, pause, resume, run, history

Examples:
  event(action: create, name: "morning-brief", schedule: "0 0 8 * * 1-5", task_type: "agent", message: "Check today's calendar and send me a summary")
  event(action: create, name: "call-kristi", at: "in 10 minutes", task_type: "agent", message: "Remind user to call Kristi")
  event(action: list)
  event(action: delete, name: "morning-brief")
  event(action: pause, name: "morning-brief")
  event(action: resume, name: "morning-brief")
  event(action: run, name: "morning-brief")
  event(action: history, name: "morning-brief")`
}

func (t *EventTool) Schema() json.RawMessage {
	return BuildDomainSchema(DomainSchemaConfig{
		Domain:      "event",
		Description: t.Description(),
		Resources:   eventResources,
		Fields: []FieldConfig{
			{Name: "name", Type: "string", Description: "Unique event/reminder name"},
			{Name: "at", Type: "string", Description: "PREFERRED for one-time reminders: 'in 5 minutes', 'in 1 hour', '7:30pm', '19:30'"},
			{Name: "schedule", Type: "string", Description: "For recurring schedules only: 'second minute hour day-of-month month day-of-week'"},
			{Name: "instructions", Type: "string", Description: "How to accomplish the task: which tools, steps, constraints"},
			{Name: "command", Type: "string", Description: "Shell command (for bash tasks)"},
			{Name: "task_type", Type: "string", Description: "Task type: bash or agent", Enum: []string{"bash", "agent"}},
			{Name: "message", Type: "string", Description: "Agent prompt (for agent tasks)"},
			{Name: "deliver", Type: "object", Description: "Where to send result: {channel, to}"},
		},
	})
}

// EventInput defines the input for the event domain tool.
type EventInput struct {
	Resource string `json:"resource,omitempty"` // ignored (flat), but accepted for alias normalization
	Action   string `json:"action"`
	Name     string `json:"name,omitempty"`
	At       string `json:"at,omitempty"`
	Schedule string `json:"schedule,omitempty"`
	Command  string `json:"command,omitempty"`
	TaskType string `json:"task_type,omitempty"`
	Message  string `json:"message,omitempty"`
	Deliver  *struct {
		Channel string `json:"channel"`
		To      string `json:"to"`
	} `json:"deliver,omitempty"`
}

func (t *EventTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in EventInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	// Normalize resource aliases to empty (flat domain)
	if in.Resource != "" {
		if _, ok := eventResourceAliases[strings.ToLower(in.Resource)]; !ok {
			return &ToolResult{Content: fmt.Sprintf("Unknown resource: %s", in.Resource), IsError: true}, nil
		}
	}

	if t.scheduler == nil {
		return &ToolResult{Content: "Error: Scheduler not configured", IsError: true}, nil
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
		return &ToolResult{Content: fmt.Sprintf("Unknown event action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Schedule action failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: result}, nil
}
