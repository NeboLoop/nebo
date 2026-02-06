//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// RemindersTool provides Windows task/reminder integration via Outlook Tasks or Task Scheduler.
type RemindersTool struct {
	hasOutlook bool
}

func NewRemindersTool() *RemindersTool {
	t := &RemindersTool{}
	t.hasOutlook = t.checkOutlook()
	return t
}

func (t *RemindersTool) checkOutlook() bool {
	script := `try { $null = New-Object -ComObject Outlook.Application; Write-Output "true" } catch { Write-Output "false" }`
	cmd := exec.Command("powershell", "-NoProfile", "-Command", script)
	out, err := cmd.Output()
	if err != nil {
		return false
	}
	return strings.TrimSpace(string(out)) == "true"
}

func (t *RemindersTool) Name() string { return "reminders" }

func (t *RemindersTool) Description() string {
	if t.hasOutlook {
		return "Manage Tasks (using Outlook) - create, list, complete, and delete tasks with due dates and priorities."
	}
	return "Manage Tasks (using Task Scheduler) - create reminders via scheduled tasks."
}

func (t *RemindersTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "create", "complete", "delete", "lists"],
				"description": "Action: list (show tasks), create, complete (mark done), delete, lists (show categories)"
			},
			"name": {"type": "string", "description": "Task name (required for create/complete/delete)"},
			"list": {"type": "string", "description": "Category name"},
			"due_date": {"type": "string", "description": "Due date: 'YYYY-MM-DD', 'tomorrow', 'in 2 days'"},
			"notes": {"type": "string", "description": "Additional notes"},
			"priority": {"type": "integer", "description": "Priority: 1 (high), 5 (medium), 9 (low)"}
		},
		"required": ["action"]
	}`)
}

func (t *RemindersTool) RequiresApproval() bool { return false }

type remindersInputWin struct {
	Action   string `json:"action"`
	Name     string `json:"name"`
	List     string `json:"list"`
	DueDate  string `json:"due_date"`
	Notes    string `json:"notes"`
	Priority int    `json:"priority"`
}

func (t *RemindersTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p remindersInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if t.hasOutlook {
		return t.executeOutlook(ctx, p)
	}
	return t.executeTaskScheduler(ctx, p)
}

// ============================================================================
// Outlook Tasks implementation
// ============================================================================

func (t *RemindersTool) executeOutlook(ctx context.Context, p remindersInputWin) (*ToolResult, error) {
	switch p.Action {
	case "lists":
		return t.outlookCategories(ctx)
	case "list":
		return t.outlookList(ctx)
	case "create":
		return t.outlookCreate(ctx, p)
	case "complete":
		return t.outlookComplete(ctx, p.Name)
	case "delete":
		return t.outlookDelete(ctx, p.Name)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *RemindersTool) outlookCategories(ctx context.Context) (*ToolResult, error) {
	script := `
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$tasks = $namespace.GetDefaultFolder(13).Items
$categories = @{}
foreach ($task in $tasks) {
    if ($task.Categories) {
        $cats = $task.Categories -split ","
        foreach ($cat in $cats) {
            $cat = $cat.Trim()
            if (-not $categories.ContainsKey($cat)) {
                $categories[$cat] = 0
            }
            $categories[$cat]++
        }
    }
}
if ($categories.Count -eq 0) {
    "No categories found"
} else {
    $categories.GetEnumerator() | ForEach-Object { "$($_.Key) ($($_.Value) tasks)" } | Sort-Object
}
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || output == "No categories found" {
		return &ToolResult{Content: "No categories found"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Categories:\n%s", output)}, nil
}

func (t *RemindersTool) outlookList(ctx context.Context) (*ToolResult, error) {
	script := `
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$tasks = $namespace.GetDefaultFolder(13).Items
$results = @()
foreach ($task in $tasks) {
    if ($task.Complete -eq $false) {
        $due = if ($task.DueDate -and $task.DueDate.Year -ne 4501) { " | Due: " + $task.DueDate.ToString('yyyy-MM-dd') } else { "" }
        $pri = switch ($task.Importance) { 2 { " [HIGH]" } 1 { " [NORMAL]" } 0 { " [LOW]" } default { "" } }
        $results += "$($task.Subject)$pri$due"
    }
}
if ($results.Count -eq 0) {
    "No pending tasks"
} else {
    $results -join [char]10
}
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || output == "No pending tasks" {
		return &ToolResult{Content: "No pending tasks"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Tasks:\n%s", output)}, nil
}

func (t *RemindersTool) outlookCreate(ctx context.Context, p remindersInputWin) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$task = $outlook.CreateItem(3)
$task.Subject = "%s"
`, escapePSReminder(p.Name))

	if p.Notes != "" {
		script += fmt.Sprintf(`$task.Body = "%s"
`, escapePSReminder(p.Notes))
	}

	if p.List != "" {
		script += fmt.Sprintf(`$task.Categories = "%s"
`, escapePSReminder(p.List))
	}

	if p.DueDate != "" {
		dueDate, err := parseReminderDueDateWin(p.DueDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid due date: %v", err), IsError: true}, nil
		}
		script += fmt.Sprintf(`$task.DueDate = [DateTime]"%s"
$task.ReminderSet = $true
$task.ReminderTime = [DateTime]"%s"
`, dueDate.Format("2006-01-02"), dueDate.Format("2006-01-02 09:00:00"))
	}

	// Set priority (2=High, 1=Normal, 0=Low)
	if p.Priority > 0 {
		var importance int
		switch {
		case p.Priority <= 3:
			importance = 2
		case p.Priority <= 6:
			importance = 1
		default:
			importance = 0
		}
		script += fmt.Sprintf(`$task.Importance = %d
`, importance)
	}

	script += `$task.Save()
Write-Output "Task created successfully"
`

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create task: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Task created: %s", p.Name)}, nil
}

func (t *RemindersTool) outlookComplete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$tasks = $namespace.GetDefaultFolder(13).Items
$found = $false
foreach ($task in $tasks) {
    if ($task.Subject -like "*%s*" -and $task.Complete -eq $false) {
        $task.Complete = $true
        $task.Save()
        $found = $true
        break
    }
}
if ($found) {
    "Task marked as complete"
} else {
    "No matching task found"
}
`, escapePSReminder(name))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *RemindersTool) outlookDelete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$tasks = $namespace.GetDefaultFolder(13).Items
$found = $false
foreach ($task in $tasks) {
    if ($task.Subject -like "*%s*") {
        $task.Delete()
        $found = $true
        break
    }
}
if ($found) {
    "Task deleted"
} else {
    "No matching task found"
}
`, escapePSReminder(name))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

// ============================================================================
// Task Scheduler implementation (fallback)
// ============================================================================

func (t *RemindersTool) executeTaskScheduler(ctx context.Context, p remindersInputWin) (*ToolResult, error) {
	switch p.Action {
	case "lists":
		return &ToolResult{Content: "Task Scheduler doesn't support categories. Install Outlook for full task management."}, nil
	case "list":
		return t.schedulerList(ctx)
	case "create":
		return t.schedulerCreate(ctx, p)
	case "complete", "delete":
		return t.schedulerDelete(ctx, p.Name)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *RemindersTool) schedulerList(ctx context.Context) (*ToolResult, error) {
	script := `Get-ScheduledTask | Where-Object { $_.TaskPath -eq "\Nebo\" } | Select-Object TaskName, State | Format-Table -AutoSize`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No scheduled reminders"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Scheduled reminders:\n%s", output)}, nil
}

func (t *RemindersTool) schedulerCreate(ctx context.Context, p remindersInputWin) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	// Create a reminder popup using msg.exe
	triggerTime := time.Now().Add(time.Hour)
	if p.DueDate != "" {
		var err error
		triggerTime, err = parseReminderDueDateWin(p.DueDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid due date: %v", err), IsError: true}, nil
		}
	}

	script := fmt.Sprintf(`
$action = New-ScheduledTaskAction -Execute "msg.exe" -Argument "* Reminder: %s"
$trigger = New-ScheduledTaskTrigger -Once -At "%s"
Register-ScheduledTask -TaskPath "\Nebo\" -TaskName "%s" -Action $action -Trigger $trigger -Force
Write-Output "Reminder scheduled"
`, escapePSReminder(p.Name), triggerTime.Format("2006-01-02 15:04"), escapePSReminder(p.Name))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create reminder: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Reminder scheduled: %s for %s", p.Name, triggerTime.Format("2006-01-02 15:04"))}, nil
}

func (t *RemindersTool) schedulerDelete(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Task name is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`Unregister-ScheduledTask -TaskPath "\Nebo\" -TaskName "%s" -Confirm:$false`, escapePSReminder(name))
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: "Reminder removed"}, nil
}

func parseReminderDueDateWin(s string) (time.Time, error) {
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
			case strings.HasPrefix(parts[1], "hour"):
				return now.Add(time.Duration(num) * time.Hour), nil
			case strings.HasPrefix(parts[1], "day"):
				return now.AddDate(0, 0, num), nil
			case strings.HasPrefix(parts[1], "week"):
				return now.AddDate(0, 0, num*7), nil
			}
		}
	}
	for _, f := range []string{"2006-01-02 15:04", "2006-01-02", "01/02/2006"} {
		if t, err := time.Parse(f, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse: %s", s)
}

func escapePSReminder(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewRemindersTool(),
		Platforms: []string{PlatformWindows},
		Category:  "productivity",
	})
}
