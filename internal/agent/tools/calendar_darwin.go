//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// CalendarTool provides macOS Calendar integration via AppleScript.
type CalendarTool struct{}

func NewCalendarTool() *CalendarTool { return &CalendarTool{} }

func (t *CalendarTool) Name() string { return "calendar" }

func (t *CalendarTool) Description() string {
	return "Manage Calendar - list events, create new events, view today's schedule, and see upcoming events."
}

func (t *CalendarTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "create", "today", "upcoming", "calendars"],
				"description": "Action: list (all events), create (new event), today, upcoming (next N days), calendars"
			},
			"title": {"type": "string", "description": "Event title (required for create)"},
			"start_date": {"type": "string", "description": "Start date/time: 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD'"},
			"end_date": {"type": "string", "description": "End date/time: 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD'"},
			"calendar": {"type": "string", "description": "Calendar name (default: first available)"},
			"location": {"type": "string", "description": "Event location"},
			"notes": {"type": "string", "description": "Event notes"},
			"days": {"type": "integer", "description": "Days to look ahead (for upcoming, default: 7)"}
		},
		"required": ["action"]
	}`)
}

func (t *CalendarTool) RequiresApproval() bool { return false }

type calendarInput struct {
	Action    string `json:"action"`
	Title     string `json:"title"`
	StartDate string `json:"start_date"`
	EndDate   string `json:"end_date"`
	Calendar  string `json:"calendar"`
	Location  string `json:"location"`
	Notes     string `json:"notes"`
	Days      int    `json:"days"`
}

func (t *CalendarTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p calendarInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "calendars":
		return t.listCalendars()
	case "today":
		return t.getTodayEvents()
	case "upcoming":
		days := p.Days
		if days <= 0 {
			days = 7
		}
		return t.getUpcomingEvents(days)
	case "create":
		return t.createEvent(p)
	case "list":
		return t.listEvents()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *CalendarTool) listCalendars() (*ToolResult, error) {
	script := `tell application "Calendar"
		set calList to {}
		repeat with cal in calendars
			set end of calList to name of cal
		end repeat
		return calList
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available calendars:\n%s", out)}, nil
}

func (t *CalendarTool) getTodayEvents() (*ToolResult, error) {
	today := time.Now().Format("January 2, 2006")
	tomorrow := time.Now().AddDate(0, 0, 1).Format("January 2, 2006")
	script := fmt.Sprintf(`tell application "Calendar"
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
	end tell`, today, tomorrow)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: "No events scheduled for today."}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Today's events:\n%s", out)}, nil
}

func (t *CalendarTool) getUpcomingEvents(days int) (*ToolResult, error) {
	today := time.Now().Format("January 2, 2006")
	future := time.Now().AddDate(0, 0, days).Format("January 2, 2006")
	script := fmt.Sprintf(`tell application "Calendar"
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
	end tell`, today, future)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No events in the next %d days.", days)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Events in the next %d days:\n%s", days, out)}, nil
}

func (t *CalendarTool) createEvent(p calendarInput) (*ToolResult, error) {
	if p.Title == "" {
		return &ToolResult{Content: "Title is required", IsError: true}, nil
	}
	if p.StartDate == "" {
		return &ToolResult{Content: "Start date is required", IsError: true}, nil
	}
	startTime, err := parseCalendarDate(p.StartDate)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid start date: %v", err), IsError: true}, nil
	}
	endTime := startTime.Add(time.Hour)
	if p.EndDate != "" {
		endTime, err = parseCalendarDate(p.EndDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid end date: %v", err), IsError: true}, nil
		}
	}
	startStr := startTime.Format("January 2, 2006 3:04:05 PM")
	endStr := endTime.Format("January 2, 2006 3:04:05 PM")
	calName := p.Calendar
	if calName == "" {
		calName = "Calendar"
	}

	script := fmt.Sprintf(`tell application "Calendar"
		tell calendar "%s"
			set newEvent to make new event with properties {summary:"%s", start date:date "%s", end date:date "%s"}`,
		escapeAS(calName), escapeAS(p.Title), startStr, endStr)
	if p.Location != "" {
		script += fmt.Sprintf(`
			set location of newEvent to "%s"`, escapeAS(p.Location))
	}
	if p.Notes != "" {
		script += fmt.Sprintf(`
			set description of newEvent to "%s"`, escapeAS(p.Notes))
	}
	script += `
		end tell
	end tell
	return "Event created successfully"`

	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *CalendarTool) listEvents() (*ToolResult, error) {
	script := `tell application "Calendar"
		set eventList to {}
		repeat with cal in calendars
			set calEvents to every event of cal
			repeat with evt in calEvents
				set evtInfo to (name of cal) & ": " & (summary of evt) & " | " & (start date of evt as string)
				set end of eventList to evtInfo
			end repeat
		end repeat
		return eventList
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("All events:\n%s", out)}, nil
}

func parseCalendarDate(s string) (time.Time, error) {
	for _, f := range []string{"2006-01-02 15:04", "2006-01-02", "01/02/2006 15:04", "01/02/2006"} {
		if t, err := time.Parse(f, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse: %s", s)
}

func escapeAS(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `"`, `\"`)
	return s
}

func execAppleScript(script string) (string, error) {
	out, err := exec.Command("osascript", "-e", script).CombinedOutput()
	return strings.TrimSpace(string(out)), err
}

