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

// CalendarTool provides Windows calendar integration via PowerShell and Outlook COM.
type CalendarTool struct {
	hasOutlook bool
}

func NewCalendarTool() *CalendarTool {
	t := &CalendarTool{}
	t.hasOutlook = t.checkOutlook()
	return t
}

func (t *CalendarTool) checkOutlook() bool {
	// Check if Outlook COM is available (with timeout to prevent init() hang)
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	script := `try { $null = New-Object -ComObject Outlook.Application; Write-Output "true" } catch { Write-Output "false" }`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.Output()
	if err != nil {
		return false
	}
	return strings.TrimSpace(string(out)) == "true"
}

func (t *CalendarTool) Name() string { return "calendar" }

func (t *CalendarTool) Description() string {
	if !t.hasOutlook {
		return "Manage Calendar - requires Microsoft Outlook to be installed."
	}
	return "Manage Calendar (using Outlook) - list events, create new events, view today's schedule, and see upcoming events."
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

type calendarInputWin struct {
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
	if !t.hasOutlook {
		return &ToolResult{
			Content: "Microsoft Outlook is not installed or not accessible.\n\n" +
				"Calendar integration on Windows requires Outlook. Please install Microsoft Outlook to use this feature.",
			IsError: true,
		}, nil
	}

	var p calendarInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "calendars":
		return t.listCalendars(ctx)
	case "today":
		return t.getEvents(ctx, 0)
	case "upcoming":
		days := p.Days
		if days <= 0 {
			days = 7
		}
		return t.getEvents(ctx, days)
	case "create":
		return t.createEvent(ctx, p)
	case "list":
		return t.getEvents(ctx, 30)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *CalendarTool) listCalendars(ctx context.Context) (*ToolResult, error) {
	script := `
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$calendars = @()
foreach ($folder in $namespace.Folders) {
    foreach ($subfolder in $folder.Folders) {
        if ($subfolder.DefaultItemType -eq 9) {
            $calendars += "$($folder.Name) / $($subfolder.Name)"
        }
    }
}
$calendars -join [char]10
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list calendars: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No calendars found."}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available calendars:\n%s", output)}, nil
}

func (t *CalendarTool) getEvents(ctx context.Context, days int) (*ToolResult, error) {
	// Build date range filter
	today := time.Now()
	startDate := today.Format("01/02/2006")
	var endDate string
	if days == 0 {
		// Today only
		endDate = today.AddDate(0, 0, 1).Format("01/02/2006")
	} else {
		endDate = today.AddDate(0, 0, days).Format("01/02/2006")
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$calendar = $namespace.GetDefaultFolder(9)
$items = $calendar.Items
$items.Sort("[Start]")
$items.IncludeRecurrences = $true
$filter = "[Start] >= '%s' AND [Start] < '%s'"
$events = $items.Restrict($filter)
$results = @()
foreach ($event in $events) {
    $loc = if ($event.Location) { " @ " + $event.Location } else { "" }
    $results += "$($event.Subject) | $($event.Start.ToString('yyyy-MM-dd HH:mm')) - $($event.End.ToString('HH:mm'))$loc"
}
if ($results.Count -eq 0) {
    "No events found."
} else {
    $results -join [char]10
}
`, startDate, endDate)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get events: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || output == "No events found." {
		if days == 0 {
			return &ToolResult{Content: "No events scheduled for today."}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("No events in the next %d days.", days)}, nil
	}

	label := "Today's events"
	if days > 0 {
		label = fmt.Sprintf("Events in the next %d days", days)
	}
	return &ToolResult{Content: fmt.Sprintf("%s:\n%s", label, output)}, nil
}

func (t *CalendarTool) createEvent(ctx context.Context, p calendarInputWin) (*ToolResult, error) {
	if p.Title == "" {
		return &ToolResult{Content: "Title is required", IsError: true}, nil
	}
	if p.StartDate == "" {
		return &ToolResult{Content: "Start date is required", IsError: true}, nil
	}

	startTime, err := parseCalendarDateWin(p.StartDate)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid start date: %v", err), IsError: true}, nil
	}

	endTime := startTime.Add(time.Hour)
	if p.EndDate != "" {
		endTime, err = parseCalendarDateWin(p.EndDate)
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Invalid end date: %v", err), IsError: true}, nil
		}
	}

	// Build PowerShell script
	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$appt = $outlook.CreateItem(1)
$appt.Subject = "%s"
$appt.Start = [DateTime]"%s"
$appt.End = [DateTime]"%s"
`, escapePowerShell(p.Title), startTime.Format("2006-01-02 15:04:05"), endTime.Format("2006-01-02 15:04:05"))

	if p.Location != "" {
		script += fmt.Sprintf(`$appt.Location = "%s"
`, escapePowerShell(p.Location))
	}
	if p.Notes != "" {
		script += fmt.Sprintf(`$appt.Body = "%s"
`, escapePowerShell(p.Notes))
	}

	script += `$appt.Save()
Write-Output "Event created successfully"
`

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create event: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Event created successfully: %s on %s", p.Title, startTime.Format("2006-01-02 15:04"))}, nil
}

func parseCalendarDateWin(s string) (time.Time, error) {
	for _, f := range []string{"2006-01-02 15:04", "2006-01-02", "01/02/2006 15:04", "01/02/2006"} {
		if t, err := time.Parse(f, s); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("could not parse: %s", s)
}

func escapePowerShell(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}

