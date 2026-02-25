package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// ShellTool provides shell operations: execute commands, manage processes and sessions
type ShellTool struct {
	policy   *Policy
	registry *ProcessRegistry
}

// ShellInput represents the consolidated input for all shell operations
type ShellInput struct {
	// STRAP fields
	Resource string `json:"resource,omitempty"` // bash, process, session
	Action   string `json:"action"`             // exec, list, kill, info, poll, log, write, send_keys

	// Bash exec fields
	Command    string `json:"command,omitempty"`    // Shell command to execute
	Timeout    int    `json:"timeout,omitempty"`    // Timeout in seconds
	Cwd        string `json:"cwd,omitempty"`        // Working directory
	Background bool   `json:"background,omitempty"` // Run in background
	YieldMs    int    `json:"yield_ms,omitempty"`   // Yield window before backgrounding

	// Process fields
	PID    int    `json:"pid,omitempty"`    // Process ID
	Signal string `json:"signal,omitempty"` // Signal to send (SIGTERM, SIGKILL, etc.)
	Filter string `json:"filter,omitempty"` // Filter processes by name

	// Session fields
	SessionID string `json:"session_id,omitempty"` // Background session ID
	Data      string `json:"data,omitempty"`       // Data to send to session
}

// NewShellTool creates a new shell domain tool
func NewShellTool(policy *Policy, registry *ProcessRegistry) *ShellTool {
	return &ShellTool{policy: policy, registry: registry}
}

// Name returns the tool name
func (t *ShellTool) Name() string {
	return "shell"
}

// Domain returns the domain name
func (t *ShellTool) Domain() string {
	return "shell"
}

// Resources returns available resources
func (t *ShellTool) Resources() []string {
	return []string{"bash", "process", "session"}
}

// ActionsFor returns available actions for a resource
func (t *ShellTool) ActionsFor(resource string) []string {
	switch resource {
	case "bash":
		return []string{"exec"}
	case "process":
		return []string{"list", "kill", "info"}
	case "session":
		return []string{"list", "poll", "log", "write", "kill"}
	default:
		return []string{}
	}
}

// Description returns the tool description
func (t *ShellTool) Description() string {
	if runtime.GOOS == "windows" {
		return `Shell and process operations. Commands run in PowerShell.

Resources:
- bash: Execute PowerShell commands (exec)
- process: Manage system processes (list, kill, info)
- session: Manage background shell sessions (list, poll, log, write, kill)

Examples:
  shell(resource: bash, action: exec, command: "Get-ChildItem")
  shell(resource: bash, action: exec, command: "npm run build", background: true)
  shell(resource: process, action: list, filter: "node")
  shell(resource: process, action: kill, pid: 12345)
  shell(resource: session, action: list)
  shell(resource: session, action: poll, session_id: "abc123")`
	}
	return `Shell and process operations.

Resources:
- bash: Execute shell commands (exec)
- process: Manage system processes (list, kill, info)
- session: Manage background shell sessions (list, poll, log, write, kill)

Examples:
  shell(resource: bash, action: exec, command: "ls -la")
  shell(resource: bash, action: exec, command: "npm run build", background: true)
  shell(resource: process, action: list, filter: "node")
  shell(resource: process, action: kill, pid: 12345, signal: "SIGTERM")
  shell(resource: session, action: list)
  shell(resource: session, action: poll, session_id: "abc123")`
}

// Schema returns the JSON schema
func (t *ShellTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"resource": {
				"type": "string",
				"description": "Resource type: bash, process, session",
				"enum": ["bash", "process", "session"]
			},
			"action": {
				"type": "string",
				"description": "Action: exec (bash), list/kill/info (process), list/poll/log/write/kill (session)",
				"enum": ["exec", "list", "kill", "info", "poll", "log", "write"]
			},
			"command": {
				"type": "string",
				"description": "Shell command to execute (for bash/exec)"
			},
			"timeout": {
				"type": "integer",
				"description": "Timeout in seconds (default: 120)"
			},
			"cwd": {
				"type": "string",
				"description": "Working directory for command"
			},
			"background": {
				"type": "boolean",
				"description": "Run in background and return session ID"
			},
			"yield_ms": {
				"type": "integer",
				"description": "Yield window in ms before backgrounding (default: 10000)"
			},
			"pid": {
				"type": "integer",
				"description": "Process ID (for process/kill and process/info)"
			},
			"signal": {
				"type": "string",
				"description": "Signal to send: SIGTERM (default), SIGKILL, SIGINT, SIGHUP",
				"enum": ["SIGTERM", "SIGKILL", "SIGINT", "SIGHUP"]
			},
			"filter": {
				"type": "string",
				"description": "Filter process list by command name"
			},
			"session_id": {
				"type": "string",
				"description": "Background session ID (for session actions)"
			},
			"data": {
				"type": "string",
				"description": "Data to write to session stdin"
			}
		},
		"required": ["action"]
	}`)
}

// RequiresApproval returns true for shell operations
func (t *ShellTool) RequiresApproval() bool {
	return true
}

// Execute routes to the appropriate handler
func (t *ShellTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in ShellInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	// Default resource based on action
	if in.Resource == "" {
		switch in.Action {
		case "exec":
			in.Resource = "bash"
		case "poll", "log", "write":
			in.Resource = "session"
		default:
			// Infer from other fields
			if in.PID > 0 {
				in.Resource = "process"
			} else if in.SessionID != "" {
				in.Resource = "session"
			} else if in.Command != "" {
				in.Resource = "bash"
			}
		}
	}

	switch in.Resource {
	case "bash":
		return t.handleBash(ctx, in)
	case "process":
		return t.handleProcess(ctx, in)
	case "session":
		return t.handleSession(ctx, in)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown resource: %s (valid: bash, process, session)", in.Resource),
			IsError: true,
		}, nil
	}
}

// handleBash executes shell commands
func (t *ShellTool) handleBash(ctx context.Context, in ShellInput) (*ToolResult, error) {
	if in.Command == "" {
		return &ToolResult{Content: "Error: command is required", IsError: true}, nil
	}

	// Handle background execution
	if in.Background && t.registry != nil {
		return t.executeBackground(ctx, in)
	}

	// Set default timeout
	timeout := 120 * time.Second
	if in.Timeout > 0 {
		timeout = time.Duration(in.Timeout) * time.Second
	}

	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// Create command
	shell, shellArgs := ShellCommand()
	args := append(shellArgs, in.Command)
	cmd := exec.CommandContext(ctx, shell, args...)
	if in.Cwd != "" {
		cmd.Dir = in.Cwd
	}

	cmd.Env = sanitizedEnv()

	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err := cmd.Run()

	// Build result
	var result strings.Builder
	if stdout.Len() > 0 {
		result.WriteString(stdout.String())
	}
	if stderr.Len() > 0 {
		if result.Len() > 0 {
			result.WriteString("\n")
		}
		result.WriteString("STDERR:\n")
		result.WriteString(stderr.String())
	}

	// Handle errors
	if err != nil {
		if ctx.Err() == context.DeadlineExceeded {
			return &ToolResult{
				Content: fmt.Sprintf("Command timed out after %v\n%s", timeout, result.String()),
				IsError: true,
			}, nil
		}
		if exitErr, ok := err.(*exec.ExitError); ok {
			return &ToolResult{
				Content: fmt.Sprintf("Command exited with code %d\n%s", exitErr.ExitCode(), result.String()),
				IsError: true,
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Command failed: %v\n%s", err, result.String()),
			IsError: true,
		}, nil
	}

	output := result.String()
	if output == "" {
		output = "(no output)"
	}

	// Truncate very long output
	const maxOutput = 50000
	if len(output) > maxOutput {
		output = output[:maxOutput] + "\n... (output truncated)"
	}

	return &ToolResult{Content: output}, nil
}

// executeBackground runs a command in background
func (t *ShellTool) executeBackground(ctx context.Context, in ShellInput) (*ToolResult, error) {
	yieldMs := 10000
	if in.YieldMs > 0 {
		yieldMs = in.YieldMs
	}

	sess, err := t.registry.SpawnBackgroundProcess(ctx, in.Command, in.Cwd, yieldMs)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to start background process: %v", err),
			IsError: true,
		}, nil
	}

	time.Sleep(100 * time.Millisecond)

	sess = t.registry.GetAnySession(sess.ID)
	if sess == nil {
		return &ToolResult{Content: "Process completed before returning", IsError: true}, nil
	}

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Background session started: **%s** (PID %d)\n\n", sess.ID, sess.PID))
	result.WriteString(fmt.Sprintf("Command: `%s`\n", in.Command))

	if sess.Exited {
		exitCode := "?"
		if sess.ExitCode != nil {
			exitCode = fmt.Sprintf("%d", *sess.ExitCode)
		}
		result.WriteString(fmt.Sprintf("\nProcess completed with exit code %s\n\n", exitCode))
		output := sess.GetOutput()
		if output != "" {
			result.WriteString("Output:\n")
			result.WriteString(output)
		}
	} else {
		result.WriteString("\nProcess running in background. Use shell tool with session resource to:\n")
		result.WriteString(fmt.Sprintf("- Poll: `{\"resource\": \"session\", \"action\": \"poll\", \"session_id\": \"%s\"}`\n", sess.ID))
		result.WriteString(fmt.Sprintf("- Log: `{\"resource\": \"session\", \"action\": \"log\", \"session_id\": \"%s\"}`\n", sess.ID))
		result.WriteString(fmt.Sprintf("- Write: `{\"resource\": \"session\", \"action\": \"write\", \"session_id\": \"%s\", \"data\": \"...\"}`\n", sess.ID))
		result.WriteString(fmt.Sprintf("- Kill: `{\"resource\": \"session\", \"action\": \"kill\", \"session_id\": \"%s\"}`\n", sess.ID))

		if initialOutput := sess.GetOutput(); initialOutput != "" {
			result.WriteString("\nInitial output:\n")
			result.WriteString(initialOutput)
		}
	}

	return &ToolResult{Content: result.String()}, nil
}

// handleProcess manages system processes
func (t *ShellTool) handleProcess(ctx context.Context, in ShellInput) (*ToolResult, error) {
	switch in.Action {
	case "list":
		return t.listProcesses(ctx, in.Filter)
	case "kill":
		if in.PID <= 0 {
			return &ToolResult{Content: "Error: pid is required for kill action", IsError: true}, nil
		}
		return t.killProcess(in.PID, in.Signal)
	case "info":
		if in.PID <= 0 {
			return &ToolResult{Content: "Error: pid is required for info action", IsError: true}, nil
		}
		return t.processInfo(ctx, in.PID)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action for process: %s (valid: list, kill, info)", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *ShellTool) listProcesses(ctx context.Context, filter string) (*ToolResult, error) {
	var cmd *exec.Cmd

	if runtime.GOOS == "darwin" || runtime.GOOS == "linux" {
		cmd = exec.CommandContext(ctx, "ps", "aux")
	} else if runtime.GOOS == "windows" {
		cmd = exec.CommandContext(ctx, "tasklist", "/V")
	} else {
		return &ToolResult{Content: fmt.Sprintf("Unsupported OS: %s", runtime.GOOS), IsError: true}, nil
	}

	output, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error listing processes: %v", err), IsError: true}, nil
	}

	lines := strings.Split(string(output), "\n")
	var result strings.Builder

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

		if filter != "" && !strings.Contains(strings.ToLower(line), filter) {
			continue
		}

		result.WriteString(line)
		result.WriteString("\n")
		count++

		if count >= 50 {
			result.WriteString("\n... (showing first 50 matching processes)")
			break
		}
	}

	if count == 0 && filter != "" {
		return &ToolResult{Content: fmt.Sprintf("No processes found matching: %s", filter)}, nil
	}

	return &ToolResult{Content: result.String()}, nil
}

func (t *ShellTool) killProcess(pid int, signal string) (*ToolResult, error) {
	process, err := os.FindProcess(pid)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Process not found: %d", pid), IsError: true}, nil
	}

	err = KillProcessWithSignal(process, signal)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error sending signal to process %d: %v", pid, err),
			IsError: true,
		}, nil
	}

	signalName := signal
	if signalName == "" {
		signalName = DefaultSignalName()
	}

	return &ToolResult{Content: fmt.Sprintf("Sent %s to process %d", signalName, pid)}, nil
}

func (t *ShellTool) processInfo(ctx context.Context, pid int) (*ToolResult, error) {
	var cmd *exec.Cmd
	var result strings.Builder

	if runtime.GOOS == "darwin" {
		cmd = exec.CommandContext(ctx, "ps", "-p", strconv.Itoa(pid), "-o", "pid,ppid,user,%cpu,%mem,state,start,time,command")
	} else if runtime.GOOS == "linux" {
		cmd = exec.CommandContext(ctx, "ps", "-p", strconv.Itoa(pid), "-o", "pid,ppid,user,%cpu,%mem,stat,start,time,cmd")
	} else if runtime.GOOS == "windows" {
		cmd = exec.CommandContext(ctx, "tasklist", "/FI", fmt.Sprintf("PID eq %d", pid), "/V")
	} else {
		return &ToolResult{Content: fmt.Sprintf("Unsupported OS: %s", runtime.GOOS), IsError: true}, nil
	}

	output, err := cmd.Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Process %d not found or error: %v", pid, err), IsError: true}, nil
	}

	result.WriteString(fmt.Sprintf("Process Information (PID: %d)\n", pid))
	result.WriteString("=" + strings.Repeat("=", 40) + "\n")
	result.WriteString(string(output))

	if runtime.GOOS == "darwin" || runtime.GOOS == "linux" {
		lsofCmd := exec.CommandContext(ctx, "lsof", "-p", strconv.Itoa(pid))
		if lsofOutput, err := lsofCmd.Output(); err == nil {
			lines := strings.Split(string(lsofOutput), "\n")
			result.WriteString(fmt.Sprintf("\nOpen files: %d\n", len(lines)-1))
		}
	}

	return &ToolResult{Content: result.String()}, nil
}

// handleSession manages background shell sessions
func (t *ShellTool) handleSession(ctx context.Context, in ShellInput) (*ToolResult, error) {
	if t.registry == nil {
		return &ToolResult{Content: "Error: session management not available", IsError: true}, nil
	}

	switch in.Action {
	case "list":
		return t.listSessions()
	case "poll":
		if in.SessionID == "" {
			return &ToolResult{Content: "Error: session_id is required", IsError: true}, nil
		}
		return t.pollSession(in.SessionID)
	case "log":
		if in.SessionID == "" {
			return &ToolResult{Content: "Error: session_id is required", IsError: true}, nil
		}
		return t.getSessionLog(in.SessionID)
	case "write":
		if in.SessionID == "" {
			return &ToolResult{Content: "Error: session_id is required", IsError: true}, nil
		}
		return t.writeToSession(in.SessionID, in.Data)
	case "kill":
		if in.SessionID == "" {
			return &ToolResult{Content: "Error: session_id is required", IsError: true}, nil
		}
		return t.killSession(in.SessionID)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action for session: %s (valid: list, poll, log, write, kill)", in.Action),
			IsError: true,
		}, nil
	}
}

func (t *ShellTool) listSessions() (*ToolResult, error) {
	running := t.registry.ListRunningSessions()
	finished := t.registry.ListFinishedSessions()

	var result strings.Builder

	if len(running) == 0 && len(finished) == 0 {
		return &ToolResult{Content: "No active or recent sessions"}, nil
	}

	if len(running) > 0 {
		result.WriteString("**Running Sessions:**\n")
		for _, s := range running {
			result.WriteString(fmt.Sprintf("- %s (PID %d): `%s`\n", s.ID, s.PID, truncateString(s.Command, 50)))
		}
	}

	if len(finished) > 0 {
		if result.Len() > 0 {
			result.WriteString("\n")
		}
		result.WriteString("**Recent Finished Sessions:**\n")
		for _, s := range finished {
			exitCode := "?"
			if s.ExitCode != nil {
				exitCode = fmt.Sprintf("%d", *s.ExitCode)
			}
			result.WriteString(fmt.Sprintf("- %s (exit %s): `%s`\n", s.ID, exitCode, truncateString(s.Command, 50)))
		}
	}

	return &ToolResult{Content: result.String()}, nil
}

func (t *ShellTool) pollSession(sessionID string) (*ToolResult, error) {
	sess := t.registry.GetAnySession(sessionID)
	if sess == nil {
		return &ToolResult{Content: fmt.Sprintf("Session not found: %s", sessionID), IsError: true}, nil
	}

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Session: %s (PID %d)\n", sess.ID, sess.PID))

	if sess.Exited {
		exitCode := "?"
		if sess.ExitCode != nil {
			exitCode = fmt.Sprintf("%d", *sess.ExitCode)
		}
		result.WriteString(fmt.Sprintf("Status: Exited (code %s)\n", exitCode))
	} else {
		result.WriteString("Status: Running\n")
	}

	// Get pending output using DrainPending
	stdout, stderr := t.registry.DrainPending(sessionID)
	if len(stdout) > 0 || len(stderr) > 0 {
		result.WriteString("\nNew output:\n")
		if len(stdout) > 0 {
			result.Write(stdout)
		}
		if len(stderr) > 0 {
			if len(stdout) > 0 {
				result.WriteString("\nSTDERR:\n")
			}
			result.Write(stderr)
		}
	} else {
		result.WriteString("\n(no new output)")
	}

	return &ToolResult{Content: result.String()}, nil
}

func (t *ShellTool) getSessionLog(sessionID string) (*ToolResult, error) {
	sess := t.registry.GetAnySession(sessionID)
	if sess == nil {
		return &ToolResult{Content: fmt.Sprintf("Session not found: %s", sessionID), IsError: true}, nil
	}

	output := sess.GetOutput()
	if output == "" {
		output = "(no output)"
	}

	return &ToolResult{Content: output}, nil
}

func (t *ShellTool) writeToSession(sessionID, data string) (*ToolResult, error) {
	sess := t.registry.GetSession(sessionID)
	if sess == nil {
		// Check if it's finished
		if t.registry.GetFinishedSession(sessionID) != nil {
			return &ToolResult{Content: fmt.Sprintf("Session %s has already exited", sessionID), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Session not found: %s", sessionID), IsError: true}, nil
	}

	if err := t.registry.WriteStdin(sessionID, []byte(data)); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error writing to session: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Wrote %d bytes to session %s", len(data), sessionID)}, nil
}

func (t *ShellTool) killSession(sessionID string) (*ToolResult, error) {
	sess := t.registry.GetSession(sessionID)
	if sess == nil {
		if t.registry.GetFinishedSession(sessionID) != nil {
			return &ToolResult{Content: fmt.Sprintf("Session %s has already exited", sessionID)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Session not found: %s", sessionID), IsError: true}, nil
	}

	if err := t.registry.KillSession(sessionID); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Error killing session: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Killed session %s (PID %d)", sessionID, sess.PID)}, nil
}

// truncateString truncates a string to max length
func truncateString(s string, max int) string {
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}

// dangerousEnvVars contains environment variables that can be exploited
// for code injection (LD_PRELOAD, DYLD_INSERT_LIBRARIES) or behavior
// manipulation (IFS, CDPATH, BASH_ENV, PROMPT_COMMAND).
var dangerousEnvVars = map[string]bool{
	// Dynamic linker injection (Linux)
	"LD_PRELOAD":      true,
	"LD_LIBRARY_PATH": true,
	"LD_AUDIT":        true,
	// Dynamic linker injection (macOS)
	"DYLD_INSERT_LIBRARIES":  true,
	"DYLD_LIBRARY_PATH":      true,
	"DYLD_FRAMEWORK_PATH":    true,
	"DYLD_FALLBACK_LIBRARY_PATH": true,
	// Shell behavior manipulation
	"IFS":              true,
	"CDPATH":           true,
	"BASH_ENV":         true,
	"ENV":              true,
	"PROMPT_COMMAND":   true,
	"BASH_FUNC_":      true, // ShellShock-style function exports (prefix match below)
	"SHELLOPTS":        true,
	"BASHOPTS":         true,
	"GLOBIGNORE":       true,
	"BASH_XTRACEFD":   true,
	// Dangerous locale/format manipulation
	"LOCALDOMAIN":      true,
	"HOSTALIASES":      true,
	"RESOLV_HOST_CONF": true,
	// Python/Ruby/Perl code injection
	"PYTHONSTARTUP":    true,
	"PYTHONPATH":       true,
	"RUBYOPT":          true,
	"RUBYLIB":          true,
	"PERL5OPT":         true,
	"PERL5LIB":         true,
	"PERL5DB":          true,
	"NODE_OPTIONS":     true,
}

// sanitizedEnv returns a copy of the current environment with dangerous
// variables removed. This prevents LD_PRELOAD injection, IFS manipulation,
// BASH_ENV execution, and similar environment-based attacks.
func sanitizedEnv() []string {
	env := os.Environ()
	clean := make([]string, 0, len(env))

	for _, e := range env {
		key := e
		if idx := strings.IndexByte(e, '='); idx >= 0 {
			key = e[:idx]
		}

		upperKey := strings.ToUpper(key)

		// Block exact matches
		if dangerousEnvVars[upperKey] {
			continue
		}

		// Block prefix matches (e.g., BASH_FUNC_xxx%%)
		if strings.HasPrefix(upperKey, "BASH_FUNC_") {
			continue
		}

		// Block all LD_ prefixed vars (catches future linker vars)
		if strings.HasPrefix(upperKey, "LD_") {
			continue
		}

		// Block all DYLD_ prefixed vars (catches future macOS linker vars)
		if strings.HasPrefix(upperKey, "DYLD_") {
			continue
		}

		clean = append(clean, e)
	}

	return clean
}
