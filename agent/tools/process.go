package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"syscall"
)

// ProcessTool manages system processes
type ProcessTool struct{}

// ProcessInput defines the input for the process tool
type ProcessInput struct {
	Action  string `json:"action"`            // "list", "kill", "info"
	PID     int    `json:"pid,omitempty"`     // Process ID (for kill/info)
	Signal  string `json:"signal,omitempty"`  // Signal to send (default: SIGTERM)
	Filter  string `json:"filter,omitempty"`  // Filter processes by name (for list)
}

// ProcessInfo represents information about a process
type ProcessInfo struct {
	PID     int    `json:"pid"`
	PPID    int    `json:"ppid"`
	User    string `json:"user"`
	CPU     string `json:"cpu"`
	Memory  string `json:"memory"`
	Command string `json:"command"`
}

// NewProcessTool creates a new process tool
func NewProcessTool() *ProcessTool {
	return &ProcessTool{}
}

// Name returns the tool name
func (t *ProcessTool) Name() string {
	return "process"
}

// Description returns the tool description
func (t *ProcessTool) Description() string {
	return "Manage system processes. List running processes, get process info, or send signals (kill/terminate)."
}

// Schema returns the JSON schema
func (t *ProcessTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action to perform: 'list' (show processes), 'kill' (send signal), 'info' (get details)",
				"enum": ["list", "kill", "info"]
			},
			"pid": {
				"type": "integer",
				"description": "Process ID (required for 'kill' and 'info' actions)"
			},
			"signal": {
				"type": "string",
				"description": "Signal to send for 'kill' action: 'SIGTERM' (default, graceful), 'SIGKILL' (force), 'SIGINT', 'SIGHUP'",
				"enum": ["SIGTERM", "SIGKILL", "SIGINT", "SIGHUP"]
			},
			"filter": {
				"type": "string",
				"description": "Filter process list by command name (case-insensitive substring match)"
			}
		},
		"required": ["action"]
	}`)
}

// RequiresApproval returns true - killing processes is dangerous
func (t *ProcessTool) RequiresApproval() bool {
	return true
}

// Execute performs the process operation
func (t *ProcessTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params ProcessInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid input: %v", err),
			IsError: true,
		}, nil
	}

	switch params.Action {
	case "list":
		return t.listProcesses(ctx, params.Filter)
	case "kill":
		if params.PID <= 0 {
			return &ToolResult{
				Content: "Error: 'pid' is required for kill action",
				IsError: true,
			}, nil
		}
		return t.killProcess(params.PID, params.Signal)
	case "info":
		if params.PID <= 0 {
			return &ToolResult{
				Content: "Error: 'pid' is required for info action",
				IsError: true,
			}, nil
		}
		return t.processInfo(ctx, params.PID)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s. Use 'list', 'kill', or 'info'", params.Action),
			IsError: true,
		}, nil
	}
}

// listProcesses lists running processes
func (t *ProcessTool) listProcesses(ctx context.Context, filter string) (*ToolResult, error) {
	var cmd *exec.Cmd

	if runtime.GOOS == "darwin" || runtime.GOOS == "linux" {
		// Use ps with common columns
		cmd = exec.CommandContext(ctx, "ps", "aux")
	} else if runtime.GOOS == "windows" {
		cmd = exec.CommandContext(ctx, "tasklist", "/V")
	} else {
		return &ToolResult{
			Content: fmt.Sprintf("Unsupported OS: %s", runtime.GOOS),
			IsError: true,
		}, nil
	}

	output, err := cmd.Output()
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error listing processes: %v", err),
			IsError: true,
		}, nil
	}

	lines := strings.Split(string(output), "\n")
	var result strings.Builder

	// Include header
	if len(lines) > 0 {
		result.WriteString(lines[0])
		result.WriteString("\n")
	}

	filter = strings.ToLower(filter)
	count := 0

	for _, line := range lines[1:] {
		if line == "" {
			continue
		}

		// Apply filter if specified
		if filter != "" && !strings.Contains(strings.ToLower(line), filter) {
			continue
		}

		result.WriteString(line)
		result.WriteString("\n")
		count++

		// Limit output
		if count >= 50 {
			result.WriteString(fmt.Sprintf("\n... (showing first 50 of matching processes, use filter to narrow down)"))
			break
		}
	}

	if count == 0 && filter != "" {
		return &ToolResult{
			Content: fmt.Sprintf("No processes found matching: %s", filter),
			IsError: false,
		}, nil
	}

	return &ToolResult{
		Content: result.String(),
		IsError: false,
	}, nil
}

// killProcess sends a signal to a process
func (t *ProcessTool) killProcess(pid int, signal string) (*ToolResult, error) {
	// Find the process
	process, err := os.FindProcess(pid)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Process not found: %d", pid),
			IsError: true,
		}, nil
	}

	// Determine signal
	var sig syscall.Signal
	switch strings.ToUpper(signal) {
	case "SIGKILL", "9":
		sig = syscall.SIGKILL
	case "SIGINT", "2":
		sig = syscall.SIGINT
	case "SIGHUP", "1":
		sig = syscall.SIGHUP
	default:
		sig = syscall.SIGTERM
	}

	// Send signal
	err = process.Signal(sig)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error sending signal to process %d: %v", pid, err),
			IsError: true,
		}, nil
	}

	signalName := signal
	if signalName == "" {
		signalName = "SIGTERM"
	}

	return &ToolResult{
		Content: fmt.Sprintf("Sent %s to process %d", signalName, pid),
		IsError: false,
	}, nil
}

// processInfo gets detailed info about a specific process
func (t *ProcessTool) processInfo(ctx context.Context, pid int) (*ToolResult, error) {
	var cmd *exec.Cmd
	var result strings.Builder

	if runtime.GOOS == "darwin" {
		// macOS: use ps with specific PID
		cmd = exec.CommandContext(ctx, "ps", "-p", strconv.Itoa(pid), "-o", "pid,ppid,user,%cpu,%mem,state,start,time,command")
	} else if runtime.GOOS == "linux" {
		// Linux: use ps with specific PID
		cmd = exec.CommandContext(ctx, "ps", "-p", strconv.Itoa(pid), "-o", "pid,ppid,user,%cpu,%mem,stat,start,time,cmd")
	} else if runtime.GOOS == "windows" {
		cmd = exec.CommandContext(ctx, "tasklist", "/FI", fmt.Sprintf("PID eq %d", pid), "/V")
	} else {
		return &ToolResult{
			Content: fmt.Sprintf("Unsupported OS: %s", runtime.GOOS),
			IsError: true,
		}, nil
	}

	output, err := cmd.Output()
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Process %d not found or error: %v", pid, err),
			IsError: true,
		}, nil
	}

	result.WriteString(fmt.Sprintf("Process Information (PID: %d)\n", pid))
	result.WriteString("=" + strings.Repeat("=", 40) + "\n")
	result.WriteString(string(output))

	// On Unix, try to get additional info
	if runtime.GOOS == "darwin" || runtime.GOOS == "linux" {
		// Get open files count
		lsofCmd := exec.CommandContext(ctx, "lsof", "-p", strconv.Itoa(pid))
		if lsofOutput, err := lsofCmd.Output(); err == nil {
			lines := strings.Split(string(lsofOutput), "\n")
			result.WriteString(fmt.Sprintf("\nOpen files: %d\n", len(lines)-1))
		}

		// Get environment (limited)
		if runtime.GOOS == "linux" {
			envPath := fmt.Sprintf("/proc/%d/environ", pid)
			if envData, err := os.ReadFile(envPath); err == nil {
				envVars := strings.Split(string(envData), "\x00")
				result.WriteString(fmt.Sprintf("\nEnvironment variables: %d\n", len(envVars)))
			}
		}
	}

	return &ToolResult{
		Content: result.String(),
		IsError: false,
	}, nil
}
