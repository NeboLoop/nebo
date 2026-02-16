//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/neboloop/nebo/internal/defaults"
)

func shortcutsDirPath() string {
	dataDir, _ := defaults.DataDir()
	return filepath.Join(dataDir, "shortcuts")
}

// ShortcutsTool provides Windows automation via Task Scheduler and PowerShell scripts.
type ShortcutsTool struct{}

func NewShortcutsTool() *ShortcutsTool {
	return &ShortcutsTool{}
}

func (t *ShortcutsTool) Name() string { return "shortcuts" }

func (t *ShortcutsTool) Description() string {
	return "Run Automations (using Task Scheduler) - create, list, and run scheduled tasks and PowerShell scripts."
}

func (t *ShortcutsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["run", "list", "create", "delete"],
				"description": "Action: run (execute), list (show all), create (new automation), delete"
			},
			"name": {"type": "string", "description": "Name of the automation to run/create/delete"},
			"command": {"type": "string", "description": "PowerShell command to execute (for create)"},
			"schedule": {"type": "string", "description": "Schedule: 'daily 09:00', 'hourly', 'weekly monday 10:00', or cron-like '0 9 * * *'"},
			"input": {"type": "string", "description": "Input to pass to the automation"}
		},
		"required": ["action"]
	}`)
}

func (t *ShortcutsTool) RequiresApproval() bool { return true }

type shortcutsInputWin struct {
	Action   string `json:"action"`
	Name     string `json:"name"`
	Command  string `json:"command"`
	Schedule string `json:"schedule"`
	Input    string `json:"input"`
}

func (t *ShortcutsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p shortcutsInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "list":
		return t.listAutomations(ctx)
	case "run":
		return t.runAutomation(ctx, p)
	case "create":
		return t.createAutomation(ctx, p)
	case "delete":
		return t.deleteAutomation(ctx, p.Name)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *ShortcutsTool) listAutomations(ctx context.Context) (*ToolResult, error) {
	var results []string

	// List Task Scheduler tasks in \Nebo folder
	script := `Get-ScheduledTask -TaskPath "\Nebo\" -ErrorAction SilentlyContinue | Select-Object TaskName, State, @{N='NextRun';E={(Get-ScheduledTaskInfo -TaskName $_.TaskName -TaskPath $_.TaskPath -ErrorAction SilentlyContinue).NextRunTime}} | Format-Table -AutoSize | Out-String -Width 200`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err == nil && len(strings.TrimSpace(string(out))) > 0 {
		results = append(results, "Scheduled Tasks (\\Nebo\\):", strings.TrimSpace(string(out)))
	} else {
		results = append(results, "Scheduled Tasks (\\Nebo\\): (none)")
	}

	// List scripts in shortcuts directory
	shortcutsDir := shortcutsDirPath()
	if entries, err := os.ReadDir(shortcutsDir); err == nil && len(entries) > 0 {
		results = append(results, "\nCustom Scripts in shortcuts/ directory:")
		for _, e := range entries {
			if !e.IsDir() {
				results = append(results, fmt.Sprintf("  - %s", e.Name()))
			}
		}
	}

	return &ToolResult{Content: strings.Join(results, "\n")}, nil
}

func (t *ShortcutsTool) runAutomation(ctx context.Context, p shortcutsInputWin) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	// First check for custom script
	shortcutsDir := shortcutsDirPath()

	// Try various script extensions
	for _, ext := range []string{"", ".ps1", ".bat", ".cmd"} {
		scriptPath := filepath.Join(shortcutsDir, p.Name+ext)
		if _, err := os.Stat(scriptPath); err == nil {
			var cmd *exec.Cmd
			if strings.HasSuffix(scriptPath, ".ps1") {
				cmd = exec.CommandContext(ctx, "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", scriptPath)
			} else {
				cmd = exec.CommandContext(ctx, "cmd", "/c", scriptPath)
			}
			if p.Input != "" {
				cmd.Stdin = strings.NewReader(p.Input)
			}
			out, err := cmd.CombinedOutput()
			if err != nil {
				return &ToolResult{Content: fmt.Sprintf("Script failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
			}
			return &ToolResult{Content: fmt.Sprintf("Script output:\n%s", strings.TrimSpace(string(out)))}, nil
		}
	}

	// Try running scheduled task
	script := fmt.Sprintf(`Start-ScheduledTask -TaskPath "\Nebo\" -TaskName "%s"`, escapeShortcutPS(p.Name))
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err == nil {
		return &ToolResult{Content: fmt.Sprintf("Started scheduled task: %s", p.Name)}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Automation '%s' not found. Check 'list' to see available automations.\nError: %s", p.Name, string(out)), IsError: true}, nil
}

func (t *ShortcutsTool) createAutomation(ctx context.Context, p shortcutsInputWin) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	if p.Command == "" {
		return &ToolResult{Content: "Command is required", IsError: true}, nil
	}

	// If schedule provided, create scheduled task
	if p.Schedule != "" {
		return t.createScheduledTask(ctx, p)
	}

	// Create PowerShell script
	shortcutsDir := shortcutsDirPath()
	if err := os.MkdirAll(shortcutsDir, 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create shortcuts directory: %v", err), IsError: true}, nil
	}

	scriptName := p.Name
	if !strings.HasSuffix(scriptName, ".ps1") {
		scriptName += ".ps1"
	}
	scriptPath := filepath.Join(shortcutsDir, scriptName)

	content := fmt.Sprintf("# Created by Nebo\n%s\n", p.Command)
	if err := os.WriteFile(scriptPath, []byte(content), 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create script: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Created automation script: %s\nRun with: shortcuts run %s", scriptPath, p.Name)}, nil
}

func (t *ShortcutsTool) createScheduledTask(ctx context.Context, p shortcutsInputWin) (*ToolResult, error) {
	// Parse schedule
	trigger := t.parseSchedule(p.Schedule)
	if trigger == "" {
		return &ToolResult{Content: fmt.Sprintf("Invalid schedule format: %s\nUse: 'daily 09:00', 'hourly', 'weekly monday 10:00', or 'startup'", p.Schedule), IsError: true}, nil
	}

	script := fmt.Sprintf(`
$action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -Command %s"
$trigger = %s
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
Register-ScheduledTask -TaskPath "\Nebo\" -TaskName "%s" -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force
Write-Output "Scheduled task created successfully"
`, escapeShortcutPS(p.Command), trigger, escapeShortcutPS(p.Name))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create scheduled task: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Created scheduled task: %s\nSchedule: %s", p.Name, p.Schedule)}, nil
}

func (t *ShortcutsTool) parseSchedule(schedule string) string {
	schedule = strings.ToLower(strings.TrimSpace(schedule))

	// Handle common formats
	switch {
	case schedule == "startup":
		return `New-ScheduledTaskTrigger -AtStartup`
	case schedule == "logon":
		return `New-ScheduledTaskTrigger -AtLogon`
	case schedule == "hourly":
		return `New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Hours 1) -RepetitionDuration ([TimeSpan]::MaxValue)`
	case strings.HasPrefix(schedule, "daily "):
		time := strings.TrimPrefix(schedule, "daily ")
		return fmt.Sprintf(`New-ScheduledTaskTrigger -Daily -At "%s"`, time)
	case strings.HasPrefix(schedule, "weekly "):
		parts := strings.Fields(strings.TrimPrefix(schedule, "weekly "))
		if len(parts) >= 2 {
			day := strings.Title(parts[0])
			time := parts[1]
			return fmt.Sprintf(`New-ScheduledTaskTrigger -Weekly -DaysOfWeek %s -At "%s"`, day, time)
		}
	case strings.HasPrefix(schedule, "monthly "):
		parts := strings.Fields(strings.TrimPrefix(schedule, "monthly "))
		if len(parts) >= 2 {
			day := parts[0]
			time := parts[1]
			return fmt.Sprintf(`New-ScheduledTaskTrigger -Monthly -DaysOfMonth %s -At "%s"`, day, time)
		}
	}

	// Try to parse as time only (assumes daily)
	if len(schedule) == 5 && schedule[2] == ':' {
		return fmt.Sprintf(`New-ScheduledTaskTrigger -Daily -At "%s"`, schedule)
	}

	return ""
}

func (t *ShortcutsTool) deleteAutomation(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	var deleted []string

	// Try to delete script
	shortcutsDir := shortcutsDirPath()
	for _, ext := range []string{"", ".ps1", ".bat", ".cmd"} {
		scriptPath := filepath.Join(shortcutsDir, name+ext)
		if err := os.Remove(scriptPath); err == nil {
			deleted = append(deleted, "script: "+name+ext)
		}
	}

	// Try to delete scheduled task
	script := fmt.Sprintf(`Unregister-ScheduledTask -TaskPath "\Nebo\" -TaskName "%s" -Confirm:$false -ErrorAction SilentlyContinue`, escapeShortcutPS(name))
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err == nil {
		deleted = append(deleted, "scheduled task: "+name)
	}
	_ = out

	if len(deleted) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No automation found with name '%s'", name)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Deleted: %s", strings.Join(deleted, ", "))}, nil
}

func escapeShortcutPS(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	return s
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewShortcutsTool(),
		Platforms: []string{PlatformWindows},
		Category:  "automation",
	})
}
