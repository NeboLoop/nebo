// Calendar Plugin - macOS Calendar integration via AppleScript
// Build: go build -o ~/.nebo/plugins/tools/calendar
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

type CalendarTool struct{}

type calendarInput struct {
	Action      string `json:"action"`       // list, create, today, upcoming, delete
	Title       string `json:"title"`        // Event title
	StartDate   string `json:"start_date"`   // Start date/time (YYYY-MM-DD HH:MM)
	EndDate     string `json:"end_date"`     // End date/time (YYYY-MM-DD HH:MM)
	Calendar    string `json:"calendar"`     // Calendar name (default: first calendar)
	Location    string `json:"location"`     // Event location
	Notes       string `json:"notes"`        // Event notes
	AllDay      bool   `json:"all_day"`      // All-day event
	Days        int    `json:"days"`         // Days to look ahead for upcoming
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *CalendarTool) Name() string {
	return "calendar"
}

func (t *CalendarTool) Description() string {
	return "Manage macOS Calendar - list events, create new events, view today's schedule, and see upcoming events."
}

func (t *CalendarTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "create", "today", "upcoming", "delete", "calendars"],
				"description": "Action to perform: list (all events), create (new event), today (today's events), upcoming (next N days), calendars (list calendars)"
			},
			"title": {
				"type": "string",
				"description": "Event title (required for create)"
			},
			"start_date": {
				"type": "string",
				"description": "Start date/time in format 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' for all-day"
			},
			"end_date": {
				"type": "string",
				"description": "End date/time in format 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' for all-day"
			},
			"calendar": {
				"type": "string",
				"description": "Calendar name to use (default: first available)"
			},
			"location": {
				"type": "string",
				"description": "Event location"
			},
			"notes": {
				"type": "string",
				"description": "Event notes/description"
			},
			"all_day": {
				"type": "boolean",
				"description": "Create as all-day event"
			},
			"days": {
				"type": "integer",
				"description": "Number of days to look ahead (for upcoming action, default: 7)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *CalendarTool) RequiresApproval() bool {
	return false
}

func (t *CalendarTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params calendarInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "calendars":
		return t.listCalendars()
	case "today":
		return t.getTodayEvents()
	case "upcoming":
		days := params.Days
		if days <= 0 {
			days = 7
		}
		return t.getUpcomingEvents(days)
	case "create":
		return t.createEvent(params)
	case "list":
		return t.listEvents(params.Calendar)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *CalendarTool) listCalendars() (*ToolResult, error) {
	script := `
		tell application "Calendar"
			set calList to {}
			repeat with cal in calendars
				set end of calList to name of cal
			end repeat
			return calList
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list calendars: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available calendars:\n%s", output), IsError: false}, nil
}

func (t *CalendarTool) getTodayEvents() (*ToolResult, error) {
	today := time.Now().Format("January 2, 2006")
	tomorrow := time.Now().AddDate(0, 0, 1).Format("January 2, 2006")

	script := fmt.Sprintf(`
		tell application "Calendar"
			set todayStart to date "%s 12:00:00 AM"
			set todayEnd to date "%s 12:00:00 AM"
			set eventList to {}
			repeat with cal in calendars
				set theEvents to (every event of cal whose start date >= todayStart and start date < todayEnd)
				repeat with evt in theEvents
					set evtInfo to (summary of evt) & " | " & (start date of evt as string)
					if location of evt is not missing value and location of evt is not "" then
						set evtInfo to evtInfo & " @ " & (location of evt)
					end if
					set end of eventList to evtInfo
				end repeat
			end repeat
			return eventList
		end tell
	`, today, tomorrow)

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get today's events: %v", err), IsError: true}, nil
	}

	if output == "" || output == "{}" {
		return &ToolResult{Content: "No events scheduled for today.", IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Today's events:\n%s", output), IsError: false}, nil
}

func (t *CalendarTool) getUpcomingEvents(days int) (*ToolResult, error) {
	today := time.Now().Format("January 2, 2006")
	future := time.Now().AddDate(0, 0, days).Format("January 2, 2006")

	script := fmt.Sprintf(`
		tell application "Calendar"
			set startDate to date "%s 12:00:00 AM"
			set endDate to date "%s 11:59:59 PM"
			set eventList to {}
			repeat with cal in calendars
				set theEvents to (every event of cal whose start date >= startDate and start date <= endDate)
				repeat with evt in theEvents
					set evtInfo to (summary of evt) & " | " & (start date of evt as string)
					if location of evt is not missing value and location of evt is not "" then
						set evtInfo to evtInfo & " @ " & (location of evt)
					end if
					set end of eventList to evtInfo
				end repeat
			end repeat
			return eventList
		end tell
	`, today, future)

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get upcoming events: %v", err), IsError: true}, nil
	}

	if output == "" || output == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No events in the next %d days.", days), IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Events in the next %d days:\n%s", days, output), IsError: false}, nil
}

func (t *CalendarTool) createEvent(params calendarInput) (*ToolResult, error) {
	if params.Title == "" {
		return &ToolResult{Content: "Title is required for creating events", IsError: true}, nil
	}
	if params.StartDate == "" {
		return &ToolResult{Content: "Start date is required for creating events", IsError: true}, nil
	}

	// Parse dates
	startTime, err := parseDateTime(params.StartDate)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid start date: %v", err), IsError: true}, nil
	}

	endTime := startTime.Add(time.Hour) // Default 1 hour duration
	if params.EndDate != "" {
		endTime, err = parseDateTime(params.EndDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid end date: %v", err), IsError: true}, nil
		}
	}

	startStr := startTime.Format("January 2, 2006 3:04:05 PM")
	endStr := endTime.Format("January 2, 2006 3:04:05 PM")

	calendarName := params.Calendar
	if calendarName == "" {
		calendarName = "Calendar" // Default calendar
	}

	script := fmt.Sprintf(`
		tell application "Calendar"
			tell calendar "%s"
				set newEvent to make new event with properties {summary:"%s", start date:date "%s", end date:date "%s"}
	`, calendarName, escapeAppleScript(params.Title), startStr, endStr)

	if params.Location != "" {
		script += fmt.Sprintf(`
				set location of newEvent to "%s"
		`, escapeAppleScript(params.Location))
	}
	if params.Notes != "" {
		script += fmt.Sprintf(`
				set description of newEvent to "%s"
		`, escapeAppleScript(params.Notes))
	}

	script += `
			end tell
		end tell
		return "Event created successfully"
	`

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create event: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *CalendarTool) listEvents(calendarName string) (*ToolResult, error) {
	script := `
		tell application "Calendar"
			set eventList to {}
			repeat with cal in calendars
				set calEvents to every event of cal
				repeat with evt in calEvents
					set evtInfo to (name of cal) & ": " & (summary of evt) & " | " & (start date of evt as string)
					set end of eventList to evtInfo
				end repeat
			end repeat
			return eventList
		end tell
	`

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list events: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("All events:\n%s", output), IsError: false}, nil
}

func parseDateTime(s string) (time.Time, error) {
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
type CalendarToolRPC struct {
	tool *CalendarTool
}

func (t *CalendarToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *CalendarToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *CalendarToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *CalendarToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *CalendarToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type CalendarPlugin struct {
	tool *CalendarTool
}

func (p *CalendarPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &CalendarToolRPC{tool: p.tool}, nil
}

func (p *CalendarPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &CalendarPlugin{tool: &CalendarTool{}},
		},
	})
}
