package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
)

// BashSessionsTool allows querying and controlling backgrounded bash processes
type BashSessionsTool struct {
	registry *ProcessRegistry
}

// NewBashSessionsTool creates a new bash sessions tool
func NewBashSessionsTool(registry *ProcessRegistry) *BashSessionsTool {
	return &BashSessionsTool{registry: registry}
}

// Name returns the tool name
func (t *BashSessionsTool) Name() string {
	return "bash_sessions"
}

// Description returns the tool description
func (t *BashSessionsTool) Description() string {
	return `Query and control backgrounded bash processes.

Actions:
- list: Show all running and recently finished bash sessions
- poll: Get new output from a backgrounded session
- log: Get full or partial log from a session
- write: Send input to a session's stdin
- kill: Terminate a running session
- clear: Remove a finished session from the registry`
}

// Schema returns the JSON schema for the tool input
func (t *BashSessionsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "poll", "log", "write", "kill", "clear"],
				"description": "Action to perform"
			},
			"session_id": {
				"type": "string",
				"description": "Session ID for poll/log/write/kill/clear actions"
			},
			"data": {
				"type": "string",
				"description": "Data to write for 'write' action"
			},
			"tail": {
				"type": "integer",
				"description": "Number of lines from end for 'log' action"
			}
		},
		"required": ["action"]
	}`)
}

// BashSessionsInput represents the tool input
type BashSessionsInput struct {
	Action    string `json:"action"`
	SessionID string `json:"session_id"`
	Data      string `json:"data"`
	Tail      int    `json:"tail"`
}

// Execute runs the bash sessions tool action
func (t *BashSessionsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in BashSessionsInput
	if err := json.Unmarshal(input, &in); err != nil {
		return nil, fmt.Errorf("invalid input: %w", err)
	}

	switch in.Action {
	case "list":
		return t.list()
	case "poll":
		return t.poll(in.SessionID)
	case "log":
		return t.log(in.SessionID, in.Tail)
	case "write":
		return t.write(in.SessionID, in.Data)
	case "kill":
		return t.kill(in.SessionID)
	case "clear":
		return t.clear(in.SessionID)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s. Valid actions: list, poll, log, write, kill, clear", in.Action),
			IsError: true,
		}, nil
	}
}

// list returns all running and finished sessions
func (t *BashSessionsTool) list() (*ToolResult, error) {
	running := t.registry.ListRunningSessions()
	finished := t.registry.ListFinishedSessions()

	var result strings.Builder
	result.WriteString("## Running Bash Sessions\n\n")

	if len(running) == 0 {
		result.WriteString("No running backgrounded sessions.\n")
	} else {
		for _, sess := range running {
			status := sess.GetStatus()
			result.WriteString(fmt.Sprintf("- **%s** (PID %d)\n", sess.ID, sess.PID))
			result.WriteString(fmt.Sprintf("  Command: `%s`\n", truncateBashCommand(sess.Command)))
			result.WriteString(fmt.Sprintf("  Duration: %dms\n", status["duration_ms"]))
			if sess.Cwd != "" {
				result.WriteString(fmt.Sprintf("  CWD: %s\n", sess.Cwd))
			}
		}
	}

	result.WriteString("\n## Recently Finished\n\n")

	if len(finished) == 0 {
		result.WriteString("No recently finished sessions.\n")
	} else {
		for _, sess := range finished {
			status := sess.GetStatus()
			exitCode := "?"
			if code, ok := status["exit_code"]; ok {
				exitCode = fmt.Sprintf("%d", code)
			}
			result.WriteString(fmt.Sprintf("- **%s** (exit %s)\n", sess.ID, exitCode))
			result.WriteString(fmt.Sprintf("  Command: `%s`\n", truncateBashCommand(sess.Command)))
			result.WriteString(fmt.Sprintf("  Duration: %dms\n", status["duration_ms"]))
		}
	}

	return &ToolResult{Content: result.String()}, nil
}

// poll returns new output since last poll
func (t *BashSessionsTool) poll(sessionID string) (*ToolResult, error) {
	if sessionID == "" {
		return &ToolResult{
			Content: "Error: session_id is required for poll action",
			IsError: true,
		}, nil
	}

	sess := t.registry.GetAnySession(sessionID)
	if sess == nil {
		return &ToolResult{
			Content: fmt.Sprintf("Session not found: %s", sessionID),
			IsError: true,
		}, nil
	}

	stdout, stderr := t.registry.DrainPending(sessionID)

	var result strings.Builder
	status := sess.GetStatus()

	if sess.Exited {
		exitCode := "?"
		if code, ok := status["exit_code"]; ok {
			exitCode = fmt.Sprintf("%d", code)
		}
		result.WriteString(fmt.Sprintf("[Session exited with code %s]\n\n", exitCode))
	}

	if len(stdout) > 0 {
		result.Write(stdout)
	}
	if len(stderr) > 0 {
		if result.Len() > 0 {
			result.WriteString("\n")
		}
		result.WriteString("STDERR:\n")
		result.Write(stderr)
	}

	if result.Len() == 0 {
		if sess.Exited {
			result.WriteString("(no new output)")
		} else {
			result.WriteString("(waiting for output...)")
		}
	}

	return &ToolResult{Content: result.String()}, nil
}

// log returns the full or partial output log
func (t *BashSessionsTool) log(sessionID string, tailLines int) (*ToolResult, error) {
	if sessionID == "" {
		return &ToolResult{
			Content: "Error: session_id is required for log action",
			IsError: true,
		}, nil
	}

	sess := t.registry.GetAnySession(sessionID)
	if sess == nil {
		return &ToolResult{
			Content: fmt.Sprintf("Session not found: %s", sessionID),
			IsError: true,
		}, nil
	}

	output := sess.GetOutput()

	if tailLines > 0 {
		lines := strings.Split(output, "\n")
		if len(lines) > tailLines {
			lines = lines[len(lines)-tailLines:]
			output = strings.Join(lines, "\n")
		}
	}

	if output == "" {
		output = "(no output)"
	}

	var result strings.Builder
	status := sess.GetStatus()

	result.WriteString(fmt.Sprintf("## Log for %s\n\n", sessionID))
	result.WriteString(fmt.Sprintf("Command: `%s`\n", sess.Command))
	result.WriteString(fmt.Sprintf("Status: %s\n", formatSessionStatus(status)))
	if status["truncated"] == true {
		result.WriteString("(output truncated)\n")
	}
	result.WriteString("\n---\n\n")
	result.WriteString(output)

	return &ToolResult{Content: result.String()}, nil
}

// write sends data to stdin
func (t *BashSessionsTool) write(sessionID string, data string) (*ToolResult, error) {
	if sessionID == "" {
		return &ToolResult{
			Content: "Error: session_id is required for write action",
			IsError: true,
		}, nil
	}

	sess := t.registry.GetSession(sessionID)
	if sess == nil {
		// Check if it's finished
		if t.registry.GetFinishedSession(sessionID) != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Session %s has already exited", sessionID),
				IsError: true,
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Session not found: %s", sessionID),
			IsError: true,
		}, nil
	}

	if err := t.registry.WriteStdin(sessionID, []byte(data)); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to write to stdin: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Sent %d bytes to %s", len(data), sessionID),
	}, nil
}

// kill terminates a running session
func (t *BashSessionsTool) kill(sessionID string) (*ToolResult, error) {
	if sessionID == "" {
		return &ToolResult{
			Content: "Error: session_id is required for kill action",
			IsError: true,
		}, nil
	}

	sess := t.registry.GetSession(sessionID)
	if sess == nil {
		if t.registry.GetFinishedSession(sessionID) != nil {
			return &ToolResult{
				Content: fmt.Sprintf("Session %s has already exited", sessionID),
			}, nil
		}
		return &ToolResult{
			Content: fmt.Sprintf("Session not found: %s", sessionID),
			IsError: true,
		}, nil
	}

	if err := t.registry.KillSession(sessionID); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to kill session: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Killed session %s (PID %d)", sessionID, sess.PID),
	}, nil
}

// clear removes a finished session from the registry
func (t *BashSessionsTool) clear(sessionID string) (*ToolResult, error) {
	if sessionID == "" {
		return &ToolResult{
			Content: "Error: session_id is required for clear action",
			IsError: true,
		}, nil
	}

	sess := t.registry.GetAnySession(sessionID)
	if sess == nil {
		return &ToolResult{
			Content: fmt.Sprintf("Session not found: %s", sessionID),
			IsError: true,
		}, nil
	}

	if !sess.Exited {
		return &ToolResult{
			Content: fmt.Sprintf("Session %s is still running. Use 'kill' first.", sessionID),
			IsError: true,
		}, nil
	}

	t.registry.DeleteSession(sessionID)

	return &ToolResult{
		Content: fmt.Sprintf("Cleared session %s", sessionID),
	}, nil
}

// RequiresApproval returns false - bash sessions tool is mostly read-only
func (t *BashSessionsTool) RequiresApproval() bool {
	return false
}

// Helper functions

func truncateBashCommand(cmd string) string {
	if len(cmd) > 60 {
		return cmd[:57] + "..."
	}
	return cmd
}

func formatSessionStatus(status map[string]any) string {
	if status["exited"] == true {
		if code, ok := status["exit_code"]; ok {
			return fmt.Sprintf("exited (code %d)", code)
		}
		return "exited"
	}
	if status["backgrounded"] == true {
		return "running (backgrounded)"
	}
	return "running"
}
