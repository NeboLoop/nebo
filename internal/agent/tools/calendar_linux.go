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

// CalendarTool provides Linux calendar integration via khal, calcurse, or gcalcli.
type CalendarTool struct {
	backend string // "khal", "calcurse", "gcalcli", or ""
}

func NewCalendarTool() *CalendarTool {
	t := &CalendarTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *CalendarTool) detectBackend() string {
	// Check for khal first (most feature-complete)
	if _, err := exec.LookPath("khal"); err == nil {
		return "khal"
	}
	// Check for gcalcli (Google Calendar)
	if _, err := exec.LookPath("gcalcli"); err == nil {
		return "gcalcli"
	}
	// Check for calcurse (local calendar)
	if _, err := exec.LookPath("calcurse"); err == nil {
		return "calcurse"
	}
	return ""
}

func (t *CalendarTool) Name() string { return "calendar" }

func (t *CalendarTool) Description() string {
	if t.backend == "" {
		return "Manage Calendar - requires khal, gcalcli, or calcurse to be installed."
	}
	return fmt.Sprintf("Manage Calendar (using %s) - list events, create new events, view today's schedule, and see upcoming events.", t.backend)
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
	if t.backend == "" {
		return &ToolResult{
			Content: "No calendar backend available. Please install one of: khal (recommended), gcalcli, or calcurse.\n\n" +
				"Install options:\n" +
				"  - khal: sudo apt install khal (Debian/Ubuntu) or pip install khal\n" +
				"  - gcalcli: pip install gcalcli (requires Google Calendar)\n" +
				"  - calcurse: sudo apt install calcurse (Debian/Ubuntu)",
			IsError: true,
		}, nil
	}

	var p calendarInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch t.backend {
	case "khal":
		return t.executeKhal(ctx, p)
	case "gcalcli":
		return t.executeGcalcli(ctx, p)
	case "calcurse":
		return t.executeCalcurse(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// khal implementation
// ============================================================================

func (t *CalendarTool) executeKhal(ctx context.Context, p calendarInput) (*ToolResult, error) {
	switch p.Action {
	case "calendars":
		return t.khalListCalendars(ctx)
	case "today":
		return t.khalListEvents(ctx, "today", "today")
	case "upcoming":
		days := p.Days
		if days <= 0 {
			days = 7
		}
		return t.khalListEvents(ctx, "today", fmt.Sprintf("%dd", days))
	case "create":
		return t.khalCreateEvent(ctx, p)
	case "list":
		return t.khalListEvents(ctx, "", "365d")
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *CalendarTool) khalListCalendars(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "khal", "printcalendars")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list calendars: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	if len(strings.TrimSpace(string(out))) == 0 {
		return &ToolResult{Content: "No calendars configured. Please configure khal first (~/.config/khal/config)."}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available calendars:\n%s", string(out))}, nil
}

func (t *CalendarTool) khalListEvents(ctx context.Context, start, duration string) (*ToolResult, error) {
	args := []string{"list"}
	if start != "" {
		args = append(args, start)
	}
	if duration != "" {
		args = append(args, duration)
	}
	args = append(args, "--format", "{calendar}: {title} | {start-time} - {end-time} | {location}")

	cmd := exec.CommandContext(ctx, "khal", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list events: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	output := strings.TrimSpace(string(out))
	if output == "" || output == "No events" {
		if start == "today" && duration == "today" {
			return &ToolResult{Content: "No events scheduled for today."}, nil
		}
		return &ToolResult{Content: "No events found."}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Events:\n%s", output)}, nil
}

func (t *CalendarTool) khalCreateEvent(ctx context.Context, p calendarInput) (*ToolResult, error) {
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

	// khal new [OPTIONS] [CALENDAR] START [END] [TIMEZONE] SUMMARY [:: DESCRIPTION]
	// Format: YYYY-MM-DD HH:MM
	args := []string{"new"}

	if p.Calendar != "" {
		args = append(args, "-a", p.Calendar)
	}

	// Add start and end times
	startStr := startTime.Format("2006-01-02 15:04")
	endStr := endTime.Format("2006-01-02 15:04")
	args = append(args, startStr, endStr)

	// Add title
	title := p.Title
	if p.Location != "" {
		title = fmt.Sprintf("%s @ %s", p.Title, p.Location)
	}
	args = append(args, title)

	// Add description if provided
	if p.Notes != "" {
		args = append(args, "::", p.Notes)
	}

	cmd := exec.CommandContext(ctx, "khal", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create event: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Event created successfully: %s on %s", p.Title, startStr)}, nil
}

// ============================================================================
// gcalcli implementation (Google Calendar)
// ============================================================================

func (t *CalendarTool) executeGcalcli(ctx context.Context, p calendarInput) (*ToolResult, error) {
	switch p.Action {
	case "calendars":
		return t.gcalcliListCalendars(ctx)
	case "today":
		return t.gcalcliAgenda(ctx, 0)
	case "upcoming":
		days := p.Days
		if days <= 0 {
			days = 7
		}
		return t.gcalcliAgenda(ctx, days)
	case "create":
		return t.gcalcliCreateEvent(ctx, p)
	case "list":
		return t.gcalcliAgenda(ctx, 30)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *CalendarTool) gcalcliListCalendars(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "gcalcli", "list")
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list calendars: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available calendars:\n%s", string(out))}, nil
}

func (t *CalendarTool) gcalcliAgenda(ctx context.Context, days int) (*ToolResult, error) {
	args := []string{"agenda", "--nocolor"}
	if days > 0 {
		endDate := time.Now().AddDate(0, 0, days).Format("2006-01-02")
		args = append(args, time.Now().Format("2006-01-02"), endDate)
	}

	cmd := exec.CommandContext(ctx, "gcalcli", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get agenda: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || strings.Contains(output, "No Events Found") {
		if days == 0 {
			return &ToolResult{Content: "No events scheduled for today."}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("No events in the next %d days.", days)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Events:\n%s", output)}, nil
}

func (t *CalendarTool) gcalcliCreateEvent(ctx context.Context, p calendarInput) (*ToolResult, error) {
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

	// Calculate duration in minutes
	duration := 60
	if p.EndDate != "" {
		endTime, err := parseCalendarDate(p.EndDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid end date: %v", err), IsError: true}, nil
		}
		duration = int(endTime.Sub(startTime).Minutes())
		if duration <= 0 {
			duration = 60
		}
	}

	args := []string{
		"add",
		"--title", p.Title,
		"--when", startTime.Format("2006-01-02 15:04"),
		"--duration", fmt.Sprintf("%d", duration),
		"--noprompt",
	}

	if p.Calendar != "" {
		args = append(args, "--calendar", p.Calendar)
	}
	if p.Location != "" {
		args = append(args, "--where", p.Location)
	}
	if p.Notes != "" {
		args = append(args, "--description", p.Notes)
	}

	cmd := exec.CommandContext(ctx, "gcalcli", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create event: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Event created successfully: %s on %s", p.Title, startTime.Format("2006-01-02 15:04"))}, nil
}

// ============================================================================
// calcurse implementation (local calendar)
// ============================================================================

func (t *CalendarTool) executeCalcurse(ctx context.Context, p calendarInput) (*ToolResult, error) {
	switch p.Action {
	case "calendars":
		return &ToolResult{Content: "calcurse uses a single local calendar stored in ~/.local/share/calcurse/"}, nil
	case "today":
		return t.calcurseList(ctx, true)
	case "upcoming", "list":
		return t.calcurseList(ctx, false)
	case "create":
		return t.calcurseCreate(ctx, p)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *CalendarTool) calcurseList(ctx context.Context, todayOnly bool) (*ToolResult, error) {
	args := []string{"-Q", "--filter-type", "apt"}
	if todayOnly {
		args = append(args, "-d", "1")
	}

	cmd := exec.CommandContext(ctx, "calcurse", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		// calcurse returns error if no appointments
		output := strings.TrimSpace(string(out))
		if output == "" {
			if todayOnly {
				return &ToolResult{Content: "No events scheduled for today."}, nil
			}
			return &ToolResult{Content: "No events found."}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed to list events: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		if todayOnly {
			return &ToolResult{Content: "No events scheduled for today."}, nil
		}
		return &ToolResult{Content: "No events found."}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Events:\n%s", output)}, nil
}

func (t *CalendarTool) calcurseCreate(ctx context.Context, p calendarInput) (*ToolResult, error) {
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

	// Format for calcurse: MM/DD/YYYY @ HH:MM -> MM/DD/YYYY @ HH:MM |message
	appointmentStr := fmt.Sprintf("%s @ %s -> %s @ %s |%s",
		startTime.Format("01/02/2006"),
		startTime.Format("15:04"),
		endTime.Format("01/02/2006"),
		endTime.Format("15:04"),
		p.Title,
	)

	// Use echo to pipe to calcurse
	cmd := exec.CommandContext(ctx, "bash", "-c", fmt.Sprintf("echo '%s' | calcurse -i -", appointmentStr))
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create event: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Event created successfully: %s on %s", p.Title, startTime.Format("2006-01-02 15:04"))}, nil
}

// Shared utilities

func parseCalendarDate(s string) (time.Time, error) {
	for _, f := range []string{"2006-01-02 15:04", "2006-01-02", "01/02/2006 15:04", "01/02/2006"} {
		if t, err := time.Parse(f, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse: %s", s)
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewCalendarTool(),
		Platforms: []string{PlatformLinux},
		Category:  "productivity",
	})
}
