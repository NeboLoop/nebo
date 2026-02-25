//go:build linux

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

// ShortcutsTool provides Linux automation via cron, systemd timers, or shell scripts.
type ShortcutsTool struct {
	backend string // "systemd", "cron", or "scripts"
}

func NewShortcutsTool() *ShortcutsTool {
	t := &ShortcutsTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *ShortcutsTool) detectBackend() string {
	// Check for systemd user mode
	if _, err := exec.LookPath("systemctl"); err == nil {
		cmd := exec.Command("systemctl", "--user", "status")
		if cmd.Run() == nil {
			return "systemd"
		}
	}
	// Fallback to cron
	if _, err := exec.LookPath("crontab"); err == nil {
		return "cron"
	}
	return "scripts"
}

func (t *ShortcutsTool) Name() string { return "shortcuts" }

func (t *ShortcutsTool) Description() string {
	switch t.backend {
	case "systemd":
		return "Run Automations (using systemd) - create, list, and run user services and timers for scheduled tasks."
	case "cron":
		return "Run Automations (using cron) - create, list, and manage cron jobs for scheduled tasks."
	default:
		return "Run Automations - execute shell scripts from the shortcuts/ directory."
	}
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
			"command": {"type": "string", "description": "Shell command to execute (for create)"},
			"schedule": {"type": "string", "description": "Cron schedule (e.g., '0 9 * * *' for 9am daily) or systemd calendar spec"},
			"input": {"type": "string", "description": "Input to pass to the automation"}
		},
		"required": ["action"]
	}`)
}

func (t *ShortcutsTool) RequiresApproval() bool { return true }

type shortcutsInputLinux struct {
	Action   string `json:"action"`
	Name     string `json:"name"`
	Command  string `json:"command"`
	Schedule string `json:"schedule"`
	Input    string `json:"input"`
}

func (t *ShortcutsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p shortcutsInputLinux
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

	switch t.backend {
	case "systemd":
		// List user timers
		cmd := exec.CommandContext(ctx, "systemctl", "--user", "list-timers", "--all")
		out, _ := cmd.CombinedOutput()
		if len(out) > 0 {
			results = append(results, "Systemd User Timers:", strings.TrimSpace(string(out)))
		}

		// List user services
		cmd = exec.CommandContext(ctx, "systemctl", "--user", "list-units", "--type=service", "--all")
		out, _ = cmd.CombinedOutput()
		if len(out) > 0 {
			results = append(results, "\nSystemd User Services:", strings.TrimSpace(string(out)))
		}

	case "cron":
		// List crontab entries
		cmd := exec.CommandContext(ctx, "crontab", "-l")
		out, err := cmd.CombinedOutput()
		if err != nil {
			results = append(results, "Cron Jobs: (none)")
		} else {
			results = append(results, "Cron Jobs:", strings.TrimSpace(string(out)))
		}
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

	if len(results) == 0 {
		return &ToolResult{Content: "No automations found."}, nil
	}
	return &ToolResult{Content: strings.Join(results, "\n")}, nil
}

func (t *ShortcutsTool) runAutomation(ctx context.Context, p shortcutsInputLinux) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	// First check for custom script
	shortcutsDir := shortcutsDirPath()
	scriptPath := filepath.Join(shortcutsDir, p.Name)

	if _, err := os.Stat(scriptPath); err == nil {
		cmd := exec.CommandContext(ctx, "bash", scriptPath)
		if p.Input != "" {
			cmd.Stdin = strings.NewReader(p.Input)
		}
		out, err := cmd.CombinedOutput()
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Script failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Script output:\n%s", strings.TrimSpace(string(out)))}, nil
	}

	// Check for .sh extension
	if !strings.HasSuffix(p.Name, ".sh") {
		scriptPath = filepath.Join(shortcutsDir, p.Name+".sh")
		if _, err := os.Stat(scriptPath); err == nil {
			cmd := exec.CommandContext(ctx, "bash", scriptPath)
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

	// Try systemd service
	if t.backend == "systemd" {
		serviceName := p.Name
		if !strings.HasSuffix(serviceName, ".service") {
			serviceName = p.Name + ".service"
		}
		cmd := exec.CommandContext(ctx, "systemctl", "--user", "start", serviceName)
		out, err := cmd.CombinedOutput()
		if err == nil {
			return &ToolResult{Content: fmt.Sprintf("Started systemd service: %s", serviceName)}, nil
		}
		_ = out // ignore error output, try next option
	}

	return &ToolResult{Content: fmt.Sprintf("Automation '%s' not found. Check 'list' to see available automations.", p.Name), IsError: true}, nil
}

func (t *ShortcutsTool) createAutomation(ctx context.Context, p shortcutsInputLinux) (*ToolResult, error) {
	if p.Name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}
	if p.Command == "" {
		return &ToolResult{Content: "Command is required", IsError: true}, nil
	}

	// If schedule provided, create cron/systemd timer
	if p.Schedule != "" {
		switch t.backend {
		case "systemd":
			return t.createSystemdTimer(ctx, p)
		case "cron":
			return t.createCronJob(ctx, p)
		}
	}

	// Create simple script
	shortcutsDir := shortcutsDirPath()
	if err := os.MkdirAll(shortcutsDir, 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create shortcuts directory: %v", err), IsError: true}, nil
	}

	scriptName := p.Name
	if !strings.HasSuffix(scriptName, ".sh") {
		scriptName += ".sh"
	}
	scriptPath := filepath.Join(shortcutsDir, scriptName)

	content := fmt.Sprintf("#!/bin/bash\n# Created by Nebo\n%s\n", p.Command)
	if err := os.WriteFile(scriptPath, []byte(content), 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create script: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Created automation script: %s\nRun with: shortcuts run %s", scriptPath, p.Name)}, nil
}

func (t *ShortcutsTool) createSystemdTimer(ctx context.Context, p shortcutsInputLinux) (*ToolResult, error) {
	userDir := filepath.Join(os.Getenv("HOME"), ".config", "systemd", "user")
	if err := os.MkdirAll(userDir, 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create systemd user directory: %v", err), IsError: true}, nil
	}

	// Create service file
	serviceName := p.Name + ".service"
	serviceContent := fmt.Sprintf(`[Unit]
Description=Nebo automation: %s

[Service]
Type=oneshot
ExecStart=/bin/bash -c '%s'
`, p.Name, strings.ReplaceAll(p.Command, "'", "'\\''"))

	servicePath := filepath.Join(userDir, serviceName)
	if err := os.WriteFile(servicePath, []byte(serviceContent), 0644); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create service: %v", err), IsError: true}, nil
	}

	// Create timer file
	timerName := p.Name + ".timer"
	timerContent := fmt.Sprintf(`[Unit]
Description=Timer for Nebo automation: %s

[Timer]
OnCalendar=%s
Persistent=true

[Install]
WantedBy=timers.target
`, p.Name, p.Schedule)

	timerPath := filepath.Join(userDir, timerName)
	if err := os.WriteFile(timerPath, []byte(timerContent), 0644); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create timer: %v", err), IsError: true}, nil
	}

	// Reload and enable
	exec.CommandContext(ctx, "systemctl", "--user", "daemon-reload").Run()
	cmd := exec.CommandContext(ctx, "systemctl", "--user", "enable", "--now", timerName)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Created files but failed to enable timer: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Created and enabled systemd timer: %s\nSchedule: %s", timerName, p.Schedule)}, nil
}

func (t *ShortcutsTool) createCronJob(ctx context.Context, p shortcutsInputLinux) (*ToolResult, error) {
	// Get existing crontab
	cmd := exec.CommandContext(ctx, "crontab", "-l")
	existing, _ := cmd.Output()

	// Add new entry
	newEntry := fmt.Sprintf("# Nebo: %s\n%s %s\n", p.Name, p.Schedule, p.Command)
	newCrontab := string(existing) + newEntry

	// Write new crontab
	cmd = exec.CommandContext(ctx, "crontab", "-")
	cmd.Stdin = strings.NewReader(newCrontab)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to update crontab: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Created cron job: %s\nSchedule: %s", p.Name, p.Schedule)}, nil
}

func (t *ShortcutsTool) deleteAutomation(ctx context.Context, name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	var deleted []string

	// Try to delete script
	shortcutsDir := shortcutsDirPath()
	for _, scriptName := range []string{name, name + ".sh"} {
		scriptPath := filepath.Join(shortcutsDir, scriptName)
		if err := os.Remove(scriptPath); err == nil {
			deleted = append(deleted, "script: "+scriptName)
		}
	}

	// Try to delete systemd timer/service
	if t.backend == "systemd" {
		userDir := filepath.Join(os.Getenv("HOME"), ".config", "systemd", "user")
		timerName := name + ".timer"
		serviceName := name + ".service"

		// Stop and disable timer
		exec.CommandContext(ctx, "systemctl", "--user", "stop", timerName).Run()
		exec.CommandContext(ctx, "systemctl", "--user", "disable", timerName).Run()

		if err := os.Remove(filepath.Join(userDir, timerName)); err == nil {
			deleted = append(deleted, "timer: "+timerName)
		}
		if err := os.Remove(filepath.Join(userDir, serviceName)); err == nil {
			deleted = append(deleted, "service: "+serviceName)
		}

		exec.CommandContext(ctx, "systemctl", "--user", "daemon-reload").Run()
	}

	// Try to remove from crontab
	if t.backend == "cron" {
		cmd := exec.CommandContext(ctx, "crontab", "-l")
		existing, err := cmd.Output()
		if err == nil {
			lines := strings.Split(string(existing), "\n")
			var newLines []string
			skipNext := false
			for _, line := range lines {
				if strings.Contains(line, "# Nebo: "+name) {
					skipNext = true
					continue
				}
				if skipNext {
					skipNext = false
					deleted = append(deleted, "cron job")
					continue
				}
				newLines = append(newLines, line)
			}
			if len(deleted) > 0 {
				cmd = exec.CommandContext(ctx, "crontab", "-")
				cmd.Stdin = strings.NewReader(strings.Join(newLines, "\n"))
				cmd.Run()
			}
		}
	}

	if len(deleted) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No automation found with name '%s'", name)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Deleted: %s", strings.Join(deleted, ", "))}, nil
}

