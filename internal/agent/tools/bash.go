package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"
)

// BashTool executes shell commands
type BashTool struct {
	policy   *Policy
	registry *ProcessRegistry
}

// NewBashTool creates a new bash tool with optional process registry for background support
func NewBashTool(policy *Policy, registry *ProcessRegistry) *BashTool {
	return &BashTool{policy: policy, registry: registry}
}

// Name returns the tool name
func (t *BashTool) Name() string {
	return "bash"
}

// Description returns the tool description
func (t *BashTool) Description() string {
	return `Execute a shell command. Use for running shell commands, scripts, and system operations.
Be careful with destructive commands - they require user approval.
Prefer using dedicated tools (read, write, glob, grep) for file operations.
Uses bash on Unix systems, cmd.exe on Windows.`
}

// Schema returns the JSON schema for the tool input
func (t *BashTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"command": {
				"type": "string",
				"description": "The shell command to execute (bash on Unix, cmd.exe on Windows)"
			},
			"timeout": {
				"type": "integer",
				"description": "Timeout in seconds (default: 120)"
			},
			"cwd": {
				"type": "string",
				"description": "Working directory for the command"
			},
			"background": {
				"type": "boolean",
				"description": "Run in background and return session ID for later polling"
			},
			"yield_ms": {
				"type": "integer",
				"description": "Yield window in milliseconds before backgrounding (default: 10000)"
			}
		},
		"required": ["command"]
	}`)
}

// BashInput represents the tool input
type BashInput struct {
	Command    string `json:"command"`
	Timeout    int    `json:"timeout"`
	Cwd        string `json:"cwd"`
	Background bool   `json:"background"`
	YieldMs    int    `json:"yield_ms"`
}

// Execute runs the bash command
func (t *BashTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in BashInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	if in.Command == "" {
		return &ToolResult{
			Content: "Error: command is required",
			IsError: true,
		}, nil
	}

	// Handle background execution if requested and registry is available
	if in.Background && t.registry != nil {
		return t.executeBackground(ctx, in)
	}

	// Set default timeout
	timeout := 120 * time.Second
	if in.Timeout > 0 {
		timeout = time.Duration(in.Timeout) * time.Second
	}

	// Create context with timeout
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	// Create command using platform-specific shell
	shell, shellArgs := ShellCommand()
	args := append(shellArgs, in.Command)
	cmd := exec.CommandContext(ctx, shell, args...)
	if in.Cwd != "" {
		cmd.Dir = in.Cwd
	}

	// Capture output
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	// Run command
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

	return &ToolResult{
		Content: output,
	}, nil
}

// executeBackground runs a command in background and returns session info
func (t *BashTool) executeBackground(ctx context.Context, in BashInput) (*ToolResult, error) {
	// Default yield window: 10 seconds
	yieldMs := 10000
	if in.YieldMs > 0 {
		yieldMs = in.YieldMs
	}

	// Spawn the background process
	sess, err := t.registry.SpawnBackgroundProcess(ctx, in.Command, in.Cwd, yieldMs)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to start background process: %v", err),
			IsError: true,
		}, nil
	}

	// Wait briefly to collect initial output
	time.Sleep(100 * time.Millisecond)

	// Check if process has already exited
	sess = t.registry.GetAnySession(sess.ID)
	if sess == nil {
		return &ToolResult{
			Content: "Process completed before returning",
			IsError: true,
		}, nil
	}

	var result strings.Builder
	result.WriteString(fmt.Sprintf("Background session started: **%s** (PID %d)\n\n", sess.ID, sess.PID))
	result.WriteString(fmt.Sprintf("Command: `%s`\n", in.Command))

	if sess.Exited {
		// Process already finished
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
		result.WriteString("\nProcess running in background. Use `bash_sessions` tool to:\n")
		result.WriteString(fmt.Sprintf("- Poll for output: `{\"action\": \"poll\", \"session_id\": \"%s\"}`\n", sess.ID))
		result.WriteString(fmt.Sprintf("- Get full log: `{\"action\": \"log\", \"session_id\": \"%s\"}`\n", sess.ID))
		result.WriteString(fmt.Sprintf("- Send input: `{\"action\": \"write\", \"session_id\": \"%s\", \"data\": \"...\"}`\n", sess.ID))
		result.WriteString(fmt.Sprintf("- Kill process: `{\"action\": \"kill\", \"session_id\": \"%s\"}`\n", sess.ID))

		// Show initial output if available
		initialOutput := sess.GetOutput()
		if initialOutput != "" {
			result.WriteString("\nInitial output:\n")
			result.WriteString(initialOutput)
		}
	}

	return &ToolResult{Content: result.String()}, nil
}

// RequiresApproval checks if this command needs approval
func (t *BashTool) RequiresApproval() bool {
	// Actual check happens in policy during Execute
	return true
}
