//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// OSAppTool provides Linux application control via standard utilities.
type OSAppTool struct{}

func NewOSAppTool() *OSAppTool { return &OSAppTool{} }

func (t *OSAppTool) Name() string { return "app" }

func (t *OSAppTool) Description() string {
	return "Control applications: list running apps, launch, quit, activate, and get app info."
}

func (t *OSAppTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "launch", "quit", "activate", "info", "frontmost"],
				"description": "Action to perform"
			},
			"name": {"type": "string", "description": "Application name or process name"},
			"path": {"type": "string", "description": "Application path for launch"},
			"force": {"type": "boolean", "description": "Force quit (SIGKILL instead of SIGTERM)"}
		},
		"required": ["action"]
	}`)
}

func (t *OSAppTool) RequiresApproval() bool { return true }

type appInput struct {
	Action string `json:"action"`
	Name   string `json:"name"`
	Path   string `json:"path"`
	Force  bool   `json:"force"`
}

func (t *OSAppTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p appInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "list":
		return t.listApps()
	case "launch":
		return t.launchApp(p.Name, p.Path)
	case "quit":
		return t.quitApp(p.Name, p.Force)
	case "activate":
		return t.activateApp(p.Name)
	case "info":
		return t.getAppInfo(p.Name)
	case "frontmost":
		return t.getFrontmostApp()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *OSAppTool) listApps() (*ToolResult, error) {
	// Use wmctrl to list windows with their PIDs
	out, err := exec.Command("wmctrl", "-l", "-p").Output()
	if err != nil {
		// Fallback to ps if wmctrl is not available
		out, err = exec.Command("ps", "-eo", "pid,comm,args", "--no-headers").Output()
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to list apps: %v", err), IsError: true}, nil
		}
		lines := strings.Split(strings.TrimSpace(string(out)), "\n")
		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("Running Processes (%d):\n\n", len(lines)))
		for _, line := range lines {
			if line != "" {
				sb.WriteString(fmt.Sprintf("• %s\n", strings.TrimSpace(line)))
			}
		}
		return &ToolResult{Content: sb.String()}, nil
	}

	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Open Windows (%d):\n\n", len(lines)))
	for _, line := range lines {
		if line != "" {
			// wmctrl format: WID DESKTOP PID HOST TITLE
			parts := strings.Fields(line)
			if len(parts) >= 4 {
				wid := parts[0]
				pid := parts[2]
				title := strings.Join(parts[4:], " ")
				sb.WriteString(fmt.Sprintf("• [%s] PID %s: %s\n", wid, pid, title))
			}
		}
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *OSAppTool) launchApp(name, path string) (*ToolResult, error) {
	if name == "" && path == "" {
		return &ToolResult{Content: "Name or path is required", IsError: true}, nil
	}

	target := path
	if target == "" {
		target = name
	}

	// Launch in background
	cmd := exec.Command("setsid", target)
	if err := cmd.Start(); err != nil {
		// Try with nohup as fallback
		cmd = exec.Command("sh", "-c", fmt.Sprintf("nohup %s > /dev/null 2>&1 &", target))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to launch: %v", err), IsError: true}, nil
		}
	}

	appName := name
	if appName == "" {
		appName = path
	}
	return &ToolResult{Content: fmt.Sprintf("Launched %s", appName)}, nil
}

func (t *OSAppTool) quitApp(name string, force bool) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	signal := "TERM"
	if force {
		signal = "KILL"
	}

	if err := exec.Command("pkill", "-"+signal, name).Run(); err != nil {
		// Try with full process name match
		if err := exec.Command("pkill", "-"+signal, "-f", name).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("No matching process found for '%s'", name), IsError: true}, nil
		}
	}

	if force {
		return &ToolResult{Content: fmt.Sprintf("Force killed %s", name)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Terminated %s", name)}, nil
}

func (t *OSAppTool) activateApp(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	// Try wmctrl first
	if err := exec.Command("wmctrl", "-a", name).Run(); err != nil {
		// Try xdotool as fallback
		out, err := exec.Command("xdotool", "search", "--name", name).Output()
		if err != nil || len(out) == 0 {
			return &ToolResult{Content: fmt.Sprintf("Window for '%s' not found", name), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]
		if err := exec.Command("xdotool", "windowactivate", windowID).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to activate: %v", err), IsError: true}, nil
		}
	}
	return &ToolResult{Content: fmt.Sprintf("Activated %s", name)}, nil
}

func (t *OSAppTool) getAppInfo(name string) (*ToolResult, error) {
	if name == "" {
		return &ToolResult{Content: "Name is required", IsError: true}, nil
	}

	// Get process info via ps
	out, err := exec.Command("ps", "-eo", "pid,ppid,user,pcpu,pmem,vsz,rss,stat,start,comm,args",
		"--no-headers", "-C", name).Output()
	if err != nil {
		// Try with full command match
		out, err = exec.Command("sh", "-c",
			fmt.Sprintf("ps -eo pid,ppid,user,pcpu,pmem,vsz,rss,stat,start,comm,args --no-headers | grep -i '%s' | head -5", name)).Output()
		if err != nil || len(out) == 0 {
			return &ToolResult{Content: fmt.Sprintf("Process '%s' not found", name), IsError: true}, nil
		}
	}

	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Process Info for '%s':\n\n", name))
	for _, line := range lines {
		if line != "" {
			fields := strings.Fields(line)
			if len(fields) >= 10 {
				sb.WriteString(fmt.Sprintf("PID: %s\n", fields[0]))
				sb.WriteString(fmt.Sprintf("Parent PID: %s\n", fields[1]))
				sb.WriteString(fmt.Sprintf("User: %s\n", fields[2]))
				sb.WriteString(fmt.Sprintf("CPU: %s%%\n", fields[3]))
				sb.WriteString(fmt.Sprintf("Memory: %s%%\n", fields[4]))
				sb.WriteString(fmt.Sprintf("Virtual Memory: %s KB\n", fields[5]))
				sb.WriteString(fmt.Sprintf("Resident Memory: %s KB\n", fields[6]))
				sb.WriteString(fmt.Sprintf("State: %s\n", fields[7]))
				sb.WriteString(fmt.Sprintf("Started: %s\n", fields[8]))
				sb.WriteString(fmt.Sprintf("Command: %s\n", strings.Join(fields[9:], " ")))
				sb.WriteString("\n")
			}
		}
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *OSAppTool) getFrontmostApp() (*ToolResult, error) {
	// Try xdotool first
	out, err := exec.Command("xdotool", "getactivewindow", "getwindowname").Output()
	if err != nil {
		return &ToolResult{Content: "Failed to get active window (xdotool may not be installed)", IsError: true}, nil
	}
	title := strings.TrimSpace(string(out))

	// Get PID of active window
	pidOut, _ := exec.Command("xdotool", "getactivewindow", "getwindowpid").Output()
	pid := strings.TrimSpace(string(pidOut))

	result := fmt.Sprintf("Active Window: %s", title)
	if pid != "" {
		// Get process name from PID
		nameOut, _ := exec.Command("ps", "-p", pid, "-o", "comm=").Output()
		procName := strings.TrimSpace(string(nameOut))
		if procName != "" {
			result = fmt.Sprintf("Process: %s (PID: %s)\nWindow: %s", procName, pid, title)
		}
	}
	return &ToolResult{Content: result}, nil
}

