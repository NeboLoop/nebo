// Reminders Plugin - macOS Reminders integration via AppleScript
// Build: go build -o ~/.nebo/plugins/tools/reminders
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/rpc"
	"os/exec"
	"strings"
	"time"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type RemindersTool struct{}

type remindersInput struct {
	Action   string `json:"action"`    // list, create, complete, delete, lists
	Name     string `json:"name"`      // Reminder name
	List     string `json:"list"`      // List name
	DueDate  string `json:"due_date"`  // Due date (YYYY-MM-DD HH:MM)
	Notes    string `json:"notes"`     // Notes
	Priority int    `json:"priority"`  // Priority (1=high, 5=medium, 9=low)
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *RemindersTool) Name() string {
	return "reminders"
}

func (t *RemindersTool) Description() string {
	return "Manage Apple Reminders - create, list, complete, and delete reminders. Organize with lists and due dates."
}

func (t *RemindersTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "create", "complete", "delete", "lists"],
				"description": "Action: list (show reminders), create (new reminder), complete (mark done), delete, lists (show lists)"
			},
			"name": {
				"type": "string",
				"description": "Reminder name (required for create, complete, delete)"
			},
			"list": {
				"type": "string",
				"description": "Reminder list name (default: Reminders)"
			},
			"due_date": {
				"type": "string",
				"description": "Due date in format 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' or relative like 'tomorrow', 'in 2 hours'"
			},
			"notes": {
				"type": "string",
				"description": "Additional notes for the reminder"
			},
			"priority": {
				"type": "integer",
				"description": "Priority: 1 (high), 5 (medium), 9 (low)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *RemindersTool) RequiresApproval() bool {
	return false
}

func (t *RemindersTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params remindersInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "lists":
		return t.getLists()
	case "list":
		return t.getReminders(params.List)
	case "create":
		return t.createReminder(params)
	case "complete":
		return t.completeReminder(params.Name, params.List)
	case "delete":
		return t.deleteReminder(params.Name, params.List)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *RemindersTool) getLists() (*ToolResult, error) {
	script := `
		tell application "Reminders"
			set listInfo to {}
			repeat with l in lists
				set incompleteCount to count of (reminders of l whose completed is false)
				set end of listInfo to name of l & " (" & incompleteCount & " incomplete)"
			end repeat
			return listInfo
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get lists: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Reminder lists:\n%s", output), IsError: false}, nil
}

func (t *RemindersTool) getReminders(listName string) (*ToolResult, error) {
	var script string
	if listName != "" {
		script = fmt.Sprintf(`
			tell application "Reminders"
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
			end tell
		`, escapeAppleScript(listName))
	} else {
		script = `
			tell application "Reminders"
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
			end tell
		`
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get reminders: %v", err), IsError: true}, nil
	}
	if output == "" || output == "{}" {
		return &ToolResult{Content: "No incomplete reminders found", IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Reminders:\n%s", output), IsError: false}, nil
}

func (t *RemindersTool) createReminder(params remindersInput) (*ToolResult, error) {
	if params.Name == "" {
		return &ToolResult{Content: "Reminder name is required", IsError: true}, nil
	}

	listName := params.List
	if listName == "" {
		listName = "Reminders"
	}

	script := fmt.Sprintf(`
		tell application "Reminders"
			set theList to list "%s"
			set newReminder to make new reminder at end of theList with properties {name:"%s"`,
		escapeAppleScript(listName), escapeAppleScript(params.Name))

	if params.Notes != "" {
		script += fmt.Sprintf(`, body:"%s"`, escapeAppleScript(params.Notes))
	}

	if params.Priority > 0 {
		script += fmt.Sprintf(`, priority:%d`, params.Priority)
	}

	script += `}`

	if params.DueDate != "" {
		dueTime, err := parseDueDate(params.DueDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid due date: %v", err), IsError: true}, nil
		}
		dueDateStr := dueTime.Format("January 2, 2006 3:04:05 PM")
		script += fmt.Sprintf(`
			set due date of newReminder to date "%s"`, dueDateStr)
	}

	script += `
		end tell
		return "Reminder created successfully"
	`

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create reminder: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *RemindersTool) completeReminder(name string, listName string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Reminder name is required", IsError: true}, nil
	}

	var script string
	if listName != "" {
		script = fmt.Sprintf(`
			tell application "Reminders"
				set theList to list "%s"
				set matchingReminders to (reminders of theList whose name contains "%s" and completed is false)
				if (count of matchingReminders) = 0 then
					return "No matching reminder found"
				end if
				set completed of (first item of matchingReminders) to true
				return "Reminder marked as complete"
			end tell
		`, escapeAppleScript(listName), escapeAppleScript(name))
	} else {
		script = fmt.Sprintf(`
			tell application "Reminders"
				repeat with l in lists
					set matchingReminders to (reminders of l whose name contains "%s" and completed is false)
					if (count of matchingReminders) > 0 then
						set completed of (first item of matchingReminders) to true
						return "Reminder marked as complete"
					end if
				end repeat
				return "No matching reminder found"
			end tell
		`, escapeAppleScript(name))
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to complete reminder: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *RemindersTool) deleteReminder(name string, listName string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Reminder name is required", IsError: true}, nil
	}

	var script string
	if listName != "" {
		script = fmt.Sprintf(`
			tell application "Reminders"
				set theList to list "%s"
				set matchingReminders to (reminders of theList whose name contains "%s")
				if (count of matchingReminders) = 0 then
					return "No matching reminder found"
				end if
				delete (first item of matchingReminders)
				return "Reminder deleted"
			end tell
		`, escapeAppleScript(listName), escapeAppleScript(name))
	} else {
		script = fmt.Sprintf(`
			tell application "Reminders"
				repeat with l in lists
					set matchingReminders to (reminders of l whose name contains "%s")
					if (count of matchingReminders) > 0 then
						delete (first item of matchingReminders)
						return "Reminder deleted"
					end if
				end repeat
				return "No matching reminder found"
			end tell
		`, escapeAppleScript(name))
	}

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to delete reminder: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func parseDueDate(s string) (time.Time, error) {
	s = strings.ToLower(s)
	now := time.Now()

	switch {
	case s == "tomorrow":
		return now.AddDate(0, 0, 1), nil
	case s == "today":
		return now, nil
	case strings.HasPrefix(s, "in "):
		// Parse "in X hours/days/minutes"
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

	formats := []string{
		"2006-01-02 15:04",
		"2006-01-02",
		"01/02/2006 15:04",
		"01/02/2006",
	}
	for _, format := range formats {
		if t, err := time.Parse(format, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse date: %s", s)
}

func escapeAppleScript(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return s
}

func runAppleScript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

// RPC wrapper
type RemindersToolRPC struct {
	tool *RemindersTool
}

func (t *RemindersToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *RemindersToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *RemindersToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *RemindersToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *RemindersToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type RemindersPlugin struct {
	tool *RemindersTool
}

func (p *RemindersPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &RemindersToolRPC{tool: p.tool}, nil
}

func (p *RemindersPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &RemindersPlugin{tool: &RemindersTool{}},
		},
	})
}
