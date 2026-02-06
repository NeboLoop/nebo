//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// RemindersTool provides macOS Reminders integration via AppleScript.
type RemindersTool struct{}

func NewRemindersTool() *RemindersTool { return &RemindersTool{} }

func (t *RemindersTool) Name() string { return "reminders" }

func (t *RemindersTool) Description() string {
	return "Manage Reminders - create, list, complete, and delete reminders with lists and due dates."
}

func (t *RemindersTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "create", "complete", "delete", "lists"],
				"description": "Action: list (show reminders), create, complete (mark done), delete, lists (show lists)"
			},
			"name": {"type": "string", "description": "Reminder name (required for create/complete/delete)"},
			"list": {"type": "string", "description": "Reminder list name (default: Reminders)"},
			"due_date": {"type": "string", "description": "Due date: 'YYYY-MM-DD HH:MM', 'tomorrow', 'in 2 hours'"},
			"notes": {"type": "string", "description": "Additional notes"},
			"priority": {"type": "integer", "description": "Priority: 1 (high), 5 (medium), 9 (low)"}
		},
		"required": ["action"]
	}`)
}

func (t *RemindersTool) RequiresApproval() bool { return false }

type remindersInput struct {
	Action   string `json:"action"`
	Name     string `json:"name"`
	List     string `json:"list"`
	DueDate  string `json:"due_date"`
	Notes    string `json:"notes"`
	Priority int    `json:"priority"`
}

func (t *RemindersTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p remindersInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "lists":
		return t.getLists()
	case "list":
		return t.getReminders(p.List)
	case "create":
		return t.createReminder(p)
	case "complete":
		return t.completeReminder(p.Name, p.List)
	case "delete":
		return t.deleteReminder(p.Name, p.List)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *RemindersTool) getLists() (*ToolResult, error) {
	script := `tell application "Reminders"
		set listInfo to {}
		repeat with l in lists
			set incompleteCount to count of (reminders of l whose completed is false)
			set end of listInfo to name of l & " (" & incompleteCount & " incomplete)"
		end repeat
		return listInfo
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Reminder lists:\n%s", out)}, nil
}

func (t *RemindersTool) getReminders(listName string) (*ToolResult, error) {
	var script string
	if listName != "" {
		script = fmt.Sprintf(`tell application "Reminders"
			set theList to list "%s"
			set reminderList to {}
			repeat with r in (reminders of theList whose completed is false)
				set rInfo to name of r
				if due date of r is not missing value then
					set rInfo to rInfo & " | Due: " & (due date of r as string)
				end if
				if priority of r is not 0 then
					set rInfo to rInfo & " | Priority: " & priority of r
				end if
				set end of reminderList to rInfo
			end repeat
			return reminderList
		end tell`, escapeAS(listName))
	} else {
		script = `tell application "Reminders"
			set reminderList to {}
			repeat with l in lists
				repeat with r in (reminders of l whose completed is false)
					set rInfo to "[" & name of l & "] " & name of r
					if due date of r is not missing value then
						set rInfo to rInfo & " | Due: " & (due date of r as string)
					end if
					set end of reminderList to rInfo
				end repeat
			end repeat
			return reminderList
		end tell`
	}
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: "No incomplete reminders found"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Reminders:\n%s", out)}, nil
}

func (t *RemindersTool) createReminder(p remindersInput) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Reminder name is required", IsError: true}, nil
	}
	listName := p.List
	if listName == "" {
		listName = "Reminders"
	}

	script := fmt.Sprintf(`tell application "Reminders"
		set theList to list "%s"
		set newReminder to make new reminder at end of theList with properties {name:"%s"`,
		escapeAS(listName), escapeAS(p.Name))
	if p.Notes != "" {
		script += fmt.Sprintf(`, body:"%s"`, escapeAS(p.Notes))
	}
	if p.Priority > 0 {
		script += fmt.Sprintf(`, priority:%d`, p.Priority)
	}
	script += `}`

	if p.DueDate != "" {
		dueTime, err := parseReminderDueDate(p.DueDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid due date: %v", err), IsError: true}, nil
		}
		script += fmt.Sprintf(`
		set due date of newReminder to date "%s"`, dueTime.Format("January 2, 2006 3:04:05 PM"))
	}
	script += `
	end tell
	return "Reminder created successfully"`

	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *RemindersTool) completeReminder(name, listName string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Reminder name is required", IsError: true}, nil
	}
	var script string
	if listName != "" {
		script = fmt.Sprintf(`tell application "Reminders"
			set theList to list "%s"
			set matchingReminders to (reminders of theList whose name contains "%s" and completed is false)
			if (count of matchingReminders) = 0 then return "No matching reminder found"
			set completed of (first item of matchingReminders) to true
			return "Reminder marked as complete"
		end tell`, escapeAS(listName), escapeAS(name))
	} else {
		script = fmt.Sprintf(`tell application "Reminders"
			repeat with l in lists
				set matchingReminders to (reminders of l whose name contains "%s" and completed is false)
				if (count of matchingReminders) > 0 then
					set completed of (first item of matchingReminders) to true
					return "Reminder marked as complete"
				end if
			end repeat
			return "No matching reminder found"
		end tell`, escapeAS(name))
	}
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *RemindersTool) deleteReminder(name, listName string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Reminder name is required", IsError: true}, nil
	}
	var script string
	if listName != "" {
		script = fmt.Sprintf(`tell application "Reminders"
			set theList to list "%s"
			set matchingReminders to (reminders of theList whose name contains "%s")
			if (count of matchingReminders) = 0 then return "No matching reminder found"
			delete (first item of matchingReminders)
			return "Reminder deleted"
		end tell`, escapeAS(listName), escapeAS(name))
	} else {
		script = fmt.Sprintf(`tell application "Reminders"
			repeat with l in lists
				set matchingReminders to (reminders of l whose name contains "%s")
				if (count of matchingReminders) > 0 then
					delete (first item of matchingReminders)
					return "Reminder deleted"
				end if
			end repeat
			return "No matching reminder found"
		end tell`, escapeAS(name))
	}
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func parseReminderDueDate(s string) (time.Time, error) {
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
			case strings.HasPrefix(parts[1], "minute"):
				return now.Add(time.Duration(num) * time.Minute), nil
			}
		}
	}
	for _, f := range []string{"2006-01-02 15:04", "2006-01-02", "01/02/2006 15:04", "01/02/2006"} {
		if t, err := time.Parse(f, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse: %s", s)
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewRemindersTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "productivity",
	})
}
