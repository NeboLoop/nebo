//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// RemindersTool provides Linux reminders/tasks integration via taskwarrior or todo.txt.
type RemindersTool struct {
	backend string // "task", "todo.sh", or ""
}

func NewRemindersTool() *RemindersTool {
	t := &RemindersTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *RemindersTool) detectBackend() string {
	if _, err := exec.LookPath("task"); err == nil {
		return "task"
	}
	if _, err := exec.LookPath("todo.sh"); err == nil {
		return "todo.sh"
	}
	return ""
}

func (t *RemindersTool) Name() string { return "reminders" }

func (t *RemindersTool) Description() string {
	if t.backend == "" {
		return "Manage Tasks/Reminders - requires taskwarrior or todo.txt-cli to be installed."
	}
	return fmt.Sprintf("Manage Tasks/Reminders (using %s) - create, list, complete, and delete tasks with due dates and priorities.", t.backend)
}

func (t *RemindersTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "create", "complete", "delete", "lists"],
				"description": "Action: list (show tasks), create, complete (mark done), delete, lists (show projects/contexts)"
			},
			"name": {"type": "string", "description": "Task name (required for create/complete/delete)"},
			"list": {"type": "string", "description": "Project name (taskwarrior) or context (todo.txt)"},
			"due_date": {"type": "string", "description": "Due date: 'YYYY-MM-DD', 'tomorrow', 'in 2 days'"},
			"notes": {"type": "string", "description": "Additional notes"},
			"priority": {"type": "integer", "description": "Priority: 1 (high/H), 5 (medium/M), 9 (low/L)"}
		},
		"required": ["action"]
	}`)
}

func (t *RemindersTool) RequiresApproval() bool { return false }

type remindersInputLinux struct {
	Action   string `json:"action"`
	Name     string `json:"name"`
	List     string `json:"list"`
	DueDate  string `json:"due_date"`
	Notes    string `json:"notes"`
	Priority int    `json:"priority"`
}

func (t *RemindersTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No task management backend available. Please install one of:\n" +
				"  - taskwarrior: sudo apt install taskwarrior (Debian/Ubuntu)\n" +
				"  - todo.txt-cli: https://github.com/todotxt/todo.txt-cli",
			IsError: true,
		}, nil
	}

	var p remindersInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch t.backend {
	case "task":
		return t.executeTaskwarrior(ctx, p)
	case "todo.sh":
		return t.executeTodoTxt(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// taskwarrior implementation
// ============================================================================

func (t *RemindersTool) executeTaskwarrior(ctx context.Context, p remindersInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "lists":
		return t.taskProjects(ctx)
	case "list":
		return t.taskList(ctx, p.List)
	case "create":
		return t.taskCreate(ctx, p)
	case "complete":
		return t.taskComplete(ctx, p.Name)
	case "delete":
		return t.taskDelete(ctx, p.Name)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *RemindersTool) taskProjects(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "task", "projects")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Projects:\n%s", strings.TrimSpace(string(out)))}, nil
}

func (t *RemindersTool) taskList(ctx context.Context, project string) (*ToolResult, error) {
	args := []string{"list"}
	if project != "" {
		args = append(args, fmt.Sprintf("project:%s", project))
	}

	cmd := exec.CommandContext(ctx, "task", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "No matches") || output == "" {
			return &ToolResult{Content: "No pending tasks"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || strings.Contains(output, "No matches") {
		return &ToolResult{Content: "No pending tasks"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Tasks:\n%s", output)}, nil
}

func (t *RemindersTool) taskCreate(ctx context.Context, p remindersInputLinux) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	args := []string{"add", p.Name}

	if p.List != "" {
		args = append(args, fmt.Sprintf("project:%s", p.List))
	}

	if p.DueDate != "" {
		dueDate, err := parseTaskDueDate(p.DueDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid due date: %v", err), IsError: true}, nil
		}
		args = append(args, fmt.Sprintf("due:%s", dueDate.Format("2006-01-02")))
	}

	if p.Priority > 0 {
		var pri string
		switch {
		case p.Priority <= 3:
			pri = "H"
		case p.Priority <= 6:
			pri = "M"
		default:
			pri = "L"
		}
		args = append(args, fmt.Sprintf("priority:%s", pri))
	}

	cmd := exec.CommandContext(ctx, "task", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create task: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Task created: %s", p.Name)}, nil
}

func (t *RemindersTool) taskComplete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	// Find task by description
	cmd := exec.CommandContext(ctx, "task", fmt.Sprintf("/%s/", name), "done")
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "No matches") {
			return &ToolResult{Content: fmt.Sprintf("No task found matching '%s'", name)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	return &ToolResult{Content: "Task marked as complete"}, nil
}

func (t *RemindersTool) taskDelete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	// Find task by description and delete
	cmd := exec.CommandContext(ctx, "task", fmt.Sprintf("/%s/", name), "delete")
	cmd.Stdin = strings.NewReader("yes\n")
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "No matches") {
			return &ToolResult{Content: fmt.Sprintf("No task found matching '%s'", name)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	return &ToolResult{Content: "Task deleted"}, nil
}

// ============================================================================
// todo.txt implementation
// ============================================================================

func (t *RemindersTool) executeTodoTxt(ctx context.Context, p remindersInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "lists":
		return t.todoContexts(ctx)
	case "list":
		return t.todoList(ctx)
	case "create":
		return t.todoCreate(ctx, p)
	case "complete":
		return t.todoComplete(ctx, p.Name)
	case "delete":
		return t.todoDelete(ctx, p.Name)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *RemindersTool) todoContexts(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "todo.sh", "listcon")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Contexts:\n%s", strings.TrimSpace(string(out)))}, nil
}

func (t *RemindersTool) todoList(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "todo.sh", "list")
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if output == "" {
			return &ToolResult{Content: "No tasks"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No tasks"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Tasks:\n%s", output)}, nil
}

func (t *RemindersTool) todoCreate(ctx context.Context, p remindersInputLinux) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	task := p.Name

	// Add priority
	if p.Priority > 0 {
		var pri string
		switch {
		case p.Priority <= 3:
			pri = "A"
		case p.Priority <= 6:
			pri = "B"
		default:
			pri = "C"
		}
		task = fmt.Sprintf("(%s) %s", pri, task)
	}

	// Add context
	if p.List != "" {
		task = fmt.Sprintf("%s @%s", task, p.List)
	}

	// Add due date
	if p.DueDate != "" {
		dueDate, err := parseTaskDueDate(p.DueDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid due date: %v", err), IsError: true}, nil
		}
		task = fmt.Sprintf("%s due:%s", task, dueDate.Format("2006-01-02"))
	}

	cmd := exec.CommandContext(ctx, "todo.sh", "add", task)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create task: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Task created: %s", p.Name)}, nil
}

func (t *RemindersTool) todoComplete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name or number is required", IsError: true}, nil
	}

	// Try to find task number by searching
	// For simplicity, assume name is a task number
	cmd := exec.CommandContext(ctx, "todo.sh", "do", name)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: "Task marked as complete"}, nil
}

func (t *RemindersTool) todoDelete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name or number is required", IsError: true}, nil
	}

	cmd := exec.CommandContext(ctx, "todo.sh", "del", name)
	cmd.Stdin = strings.NewReader("y\n")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: "Task deleted"}, nil
}

func parseTaskDueDate(s string) (time.Time, error) {
	s = strings.ToLower(s)
	now := time.Now()
	switch {
	case s == "tomorrow":
		return now.AddDate(0, 0, 1), nil
	case s == "today":
		return now, nil
	case strings.HasPrefix(s, "in "):
		parts := strings.Fields(s[3:])
		if len(parts) >= 2 {
			var num int
			fmt.Sscanf(parts[0], "%d", &num)
			switch {
			case strings.HasPrefix(parts[1], "day"):
				return now.AddDate(0, 0, num), nil
			case strings.HasPrefix(parts[1], "week"):
				return now.AddDate(0, 0, num*7), nil
			}
		}
	}
	for _, f := range []string{"2006-01-02", "01/02/2006"} {
		if t, err := time.Parse(f, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse: %s", s)
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewRemindersTool(),
		Platforms: []string{PlatformLinux},
		Category:  "productivity",
	})
}
