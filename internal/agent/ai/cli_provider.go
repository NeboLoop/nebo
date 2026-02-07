package ai

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/nebolabs/nebo/internal/agent/session"
)

// CLIProvider wraps an official CLI tool (claude, gemini, codex) as a provider
type CLIProvider struct {
	name    string
	command string
	args    []string
}

// NewCLIProvider creates a new CLI-based provider
func NewCLIProvider(name, command string, args []string) *CLIProvider {
	return &CLIProvider{
		name:    name,
		command: command,
		args:    args,
	}
}

// DefaultServerPort is the default HTTP server port where agent MCP tools are served
const DefaultServerPort = 27895

// NewClaudeCodeProvider creates a provider that wraps the Claude Code CLI.
// Claude Code: brew install claude-code or npm i -g @anthropic-ai/claude-code
// Uses ~/.claude/ for auth, supports extended thinking.
// Claude CLI connects to Nebo's agent MCP server at /agent/mcp for tool access.
// All built-in Claude Code tools are disabled (--tools "") so it only uses
// Nebo's STRAP tools via MCP. Model is passed via ChatRequest.Model at runtime.
// serverPort is the HTTP server port (0 = DefaultServerPort).
// maxTurns caps multi-turn tool use (0 = unlimited).
func NewClaudeCodeProvider(maxTurns int, serverPort int) *CLIProvider {
	if serverPort == 0 {
		serverPort = DefaultServerPort
	}

	// MCP config pointing to Nebo's agent MCP server which exposes all STRAP tools
	mcpConfig := fmt.Sprintf(`{"mcpServers":{"nebo-agent":{"type":"http","url":"http://localhost:%d/agent/mcp"}}}`, serverPort)

	args := []string{
		"--print",                              // Non-interactive output
		"--verbose",                            // Required for stream-json with --print
		"--output-format", "stream-json",       // Streaming JSON events
		"--include-partial-messages",           // Token-by-token streaming (not just turn-level)
		"--dangerously-skip-permissions",       // Autonomous mode
		"--tools", "",                          // Disable ALL built-in Claude Code tools
		"--mcp-config", mcpConfig,              // Use Nebo's STRAP tools via MCP
		"--strict-mcp-config",                  // Ignore user's other MCP servers
		"--allowedTools", "mcp__nebo-agent__*", // Auto-approve all Nebo MCP tool calls
	}

	// Only add --max-turns if explicitly configured (0 = unlimited)
	if maxTurns > 0 {
		args = append(args, "--max-turns", fmt.Sprintf("%d", maxTurns))
	}

	return &CLIProvider{
		name:    "claude-code",
		command: "claude",
		args:    args,
	}
}

// NewGeminiCLIProvider creates a provider that wraps the Google Gemini CLI
// Gemini CLI: npm i -g @google/gemini-cli
// FREE: 1000 requests/day, 1M context window, Google Search grounding
func NewGeminiCLIProvider() *CLIProvider {
	return &CLIProvider{
		name:    "gemini-cli",
		command: "gemini",
		args:    []string{}, // Gemini CLI reads from stdin
	}
}

// NewCodexCLIProvider creates a provider that wraps the OpenAI Codex CLI
// Codex CLI: brew install --cask codex or npm i -g @openai/codex
// Uses ChatGPT account or API key, supports --full-auto mode
func NewCodexCLIProvider() *CLIProvider {
	return &CLIProvider{
		name:    "codex-cli",
		command: "codex",
		args:    []string{"--full-auto"}, // Autonomous mode
	}
}

// ID returns the provider identifier
func (p *CLIProvider) ID() string {
	return p.name
}

// ProfileID returns empty - CLI providers don't use auth profiles
func (p *CLIProvider) ProfileID() string {
	return ""
}

// HandlesTools returns true - CLI providers execute tools via MCP
func (p *CLIProvider) HandlesTools() bool {
	return true
}

// Stream sends a request to the CLI and streams the response
func (p *CLIProvider) Stream(ctx context.Context, req *ChatRequest) (<-chan StreamEvent, error) {
	resultCh := make(chan StreamEvent, 100)

	go func() {
		defer close(resultCh)

		// Build the prompt from messages
		prompt := buildPromptFromMessages(req.Messages)

		// Build command args
		args := append([]string{}, p.args...)

		// Add model flag if specified in request (for CLI providers that support it)
		if req.Model != "" && (p.name == "claude-cli" || p.name == "codex-cli") {
			args = append(args, "--model", req.Model)
		}

		// Pass system prompt if provided (claude CLI supports --system-prompt)
		if req.System != "" && p.command == "claude" {
			args = append(args, "--system-prompt", req.System)
		}

		// Use "--" to separate flags from the positional prompt argument.
		args = append(args, "--", prompt)

		// Log command start (not individual stream lines)
		fmt.Printf("[CLIProvider] Running: %s (prompt_len=%d)\n", p.command, len(prompt))

		// Create command
		cmd := exec.CommandContext(ctx, p.command, args...)

		// Get stdout pipe for streaming
		stdout, err := cmd.StdoutPipe()
		if err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("failed to create stdout pipe: %w", err),
			}
			return
		}

		stderr, err := cmd.StderrPipe()
		if err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("failed to create stderr pipe: %w", err),
			}
			return
		}

		// Start the command
		if err := cmd.Start(); err != nil {
			fmt.Printf("[CLIProvider] Failed to start: %v\n", err)
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("failed to start %s: %w", p.command, err),
			}
			return
		}
		fmt.Printf("[CLIProvider] Command started, PID=%d\n", cmd.Process.Pid)

		// Read stderr in background - wait for it before finishing
		var stderrWg sync.WaitGroup
		var stderrOutput string
		stderrWg.Add(1)
		go func() {
			defer stderrWg.Done()
			errBytes, _ := io.ReadAll(stderr)
			stderrOutput = strings.TrimSpace(string(errBytes))
			if stderrOutput != "" {
				fmt.Printf("[CLIProvider] STDERR: %s\n", stderrOutput)
			}
		}()

		// Stream stdout line by line
		scanner := bufio.NewScanner(stdout)
		scanner.Buffer(make([]byte, 1024*1024), 1024*1024) // 1MB buffer

		// Tool state tracking for this stream.
		// Claude's streaming API sends tool input incrementally:
		//   content_block_start (tool_use, input:{})
		//   content_block_delta (input_json_delta) × N
		//   content_block_stop
		// We accumulate the input and emit EventTypeToolCall only at stop.
		type pendingToolInfo struct {
			ID    string
			Name  string
			Input strings.Builder
		}
		var pendingTool *pendingToolInfo

		for scanner.Scan() {
			select {
			case <-ctx.Done():
				cmd.Process.Kill()
				return
			default:
				line := scanner.Text()
				if line == "" {
					continue
				}

				// Pre-parse raw JSON to intercept tool-related streaming events.
				// This lets us accumulate tool input deltas and emit EventTypeToolCall
				// with the full input (instead of empty {}).
				var rawEvent map[string]any
				if err := json.Unmarshal([]byte(line), &rawEvent); err == nil {
					eventType, _ := rawEvent["type"].(string)
					// Handle nested event envelope
					if innerEvent, ok := rawEvent["event"].(map[string]any); ok {
						eventType, _ = innerEvent["type"].(string)
						rawEvent = innerEvent
					}

					switch eventType {
					case "content_block_start":
						if block, ok := rawEvent["content_block"].(map[string]any); ok {
							if blockType, _ := block["type"].(string); blockType == "tool_use" {
								name, _ := block["name"].(string)
								id, _ := block["id"].(string)
								pendingTool = &pendingToolInfo{ID: id, Name: name}
								continue // Don't emit yet — wait for full input
							}
						}

					case "content_block_delta":
						if delta, ok := rawEvent["delta"].(map[string]any); ok {
							if deltaType, _ := delta["type"].(string); deltaType == "input_json_delta" {
								if pendingTool != nil {
									if partial, ok := delta["partial_json"].(string); ok {
										pendingTool.Input.WriteString(partial)
									}
								}
								continue // Accumulated, don't emit
							}
						}

					case "content_block_stop":
						if pendingTool != nil {
							// Emit the full tool call with accumulated input
							inputJSON := pendingTool.Input.String()
							if inputJSON == "" {
								inputJSON = "{}"
							}
							resultCh <- StreamEvent{
								Type: EventTypeToolCall,
								ToolCall: &ToolCall{
									ID:    pendingTool.ID,
									Name:  pendingTool.Name,
									Input: json.RawMessage(inputJSON),
								},
							}
							pendingTool = nil
							continue
						}
					}
				}

				// For all other events, use existing parseLine
				event := p.parseLine(line)

				// Skip empty text events (message_start, etc.)
				// These generate unnecessary WebSocket frames and UI re-renders
				if event.Type == EventTypeText && event.Text == "" {
					continue
				}
				resultCh <- event
			}
		}

		if err := scanner.Err(); err != nil {
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("error reading output: %w", err),
			}
		}

		// Wait for stderr to be fully read
		stderrWg.Wait()

		// Wait for command to finish
		waitErr := cmd.Wait()
		fmt.Printf("[CLIProvider] Command finished, waitErr=%v\n", waitErr)

		// Report stderr as error if command failed
		if waitErr != nil {
			// Don't emit error if context was cancelled
			if ctx.Err() == nil {
				errMsg := fmt.Sprintf("%s exited with error: %v", p.command, waitErr)
				if stderrOutput != "" {
					errMsg = fmt.Sprintf("%s: %s", errMsg, stderrOutput)
				}
				fmt.Printf("[CLIProvider] ERROR: %s\n", errMsg)
				resultCh <- StreamEvent{
					Type:  EventTypeError,
					Error: fmt.Errorf("%s", errMsg),
				}
			}
		} else if stderrOutput != "" && !strings.Contains(stderrOutput, "ANTHROPIC_API_KEY") {
			// Emit non-fatal stderr as error event
			resultCh <- StreamEvent{
				Type:  EventTypeError,
				Error: fmt.Errorf("%s stderr: %s", p.command, stderrOutput),
			}
		}

		resultCh <- StreamEvent{Type: EventTypeDone}
	}()

	return resultCh, nil
}

// parseLine parses a line of output from Claude Code's stream-json format
func (p *CLIProvider) parseLine(line string) StreamEvent {
	if line == "" {
		return StreamEvent{Type: EventTypeText, Text: ""}
	}

	var data map[string]any
	if err := json.Unmarshal([]byte(line), &data); err != nil {
		// Plain text fallback
		return StreamEvent{Type: EventTypeText, Text: line + "\n"}
	}

	eventType, _ := data["type"].(string)

	// Unwrap stream_event container - Claude Code wraps events in this
	if eventType == "stream_event" {
		if innerEvent, ok := data["event"].(map[string]any); ok {
			data = innerEvent
			eventType, _ = data["type"].(string)
		}
	}

	switch eventType {
	// Text streaming delta
	case "content_block_delta":
		if delta, ok := data["delta"].(map[string]any); ok {
			deltaType, _ := delta["type"].(string)
			switch deltaType {
			case "text_delta":
				if text, ok := delta["text"].(string); ok {
					return StreamEvent{Type: EventTypeText, Text: text}
				}
			case "thinking_delta":
				if text, ok := delta["thinking"].(string); ok {
					return StreamEvent{Type: EventTypeThinking, Text: text}
				}
			case "input_json_delta":
				// Tool input streaming is handled in Stream() — accumulates deltas there
				return StreamEvent{Type: EventTypeText, Text: ""}
			}
		}

	// Tool use start — actual handling (with full input) is in Stream().
	// This is a fallback for non-tool-use content_block_start events.
	case "content_block_start":
		// Tool use blocks are intercepted in Stream() before parseLine is called.
		// Non-tool blocks fall through here and return empty.

	// Result message - signals completion of this CLI turn
	case "result":
		subtype, _ := data["subtype"].(string)
		if subtype == "success" || subtype == "error_max_turns" {
			// CLI provider completed its full agentic loop.
			// Runner uses provider.HandlesTools() to skip tool execution.
			return StreamEvent{Type: EventTypeDone}
		}
		// For other cases (errors, etc.), emit the result text
		if result, ok := data["result"].(string); ok {
			return StreamEvent{Type: EventTypeText, Text: result}
		}

	// System messages
	case "system":
		if msg, ok := data["message"].(string); ok {
			return StreamEvent{Type: EventTypeText, Text: "[system] " + msg + "\n"}
		}

	// Assistant message container - extract full message for saving
	case "assistant":
		if message, ok := data["message"].(map[string]any); ok {
			msg := &session.Message{Role: "assistant"}

			if content, ok := message["content"].([]any); ok {
				var textParts []string
				var toolCalls []session.ToolCall

				for _, block := range content {
					if blockMap, ok := block.(map[string]any); ok {
						blockType, _ := blockMap["type"].(string)
						switch blockType {
						case "text":
							if text, ok := blockMap["text"].(string); ok {
								textParts = append(textParts, text)
							}
						case "tool_use":
							id, _ := blockMap["id"].(string)
							name, _ := blockMap["name"].(string)
							input, _ := json.Marshal(blockMap["input"])
							toolCalls = append(toolCalls, session.ToolCall{
								ID:    id,
								Name:  name,
								Input: input,
							})
						}
					}
				}

				msg.Content = strings.Join(textParts, "\n")
				if len(toolCalls) > 0 {
					msg.ToolCalls, _ = json.Marshal(toolCalls)
				}
			}

			// Emit as full message event for saving
			return StreamEvent{Type: EventTypeMessage, Message: msg, Text: msg.Content}
		}
		return StreamEvent{Type: EventTypeText, Text: ""}

	// Message delta (contains stop_reason)
	case "message_delta":
		// Just ignore - we handle completion via the "result" event
		return StreamEvent{Type: EventTypeText, Text: ""}

	// Message stop
	case "message_stop", "content_block_stop":
		return StreamEvent{Type: EventTypeText, Text: ""}

	// Error
	case "error":
		msg := fmt.Sprintf("%v", data["error"])
		return StreamEvent{Type: EventTypeError, Error: &ProviderError{Message: msg}}

	// Task/subagent events - log for visibility
	case "task":
		taskID, _ := data["task_id"].(string)
		status, _ := data["status"].(string)
		description, _ := data["description"].(string)
		if status != "" {
			return StreamEvent{Type: EventTypeText, Text: fmt.Sprintf("[Task %s] %s: %s\n", taskID, status, description)}
		}

	// Agent output events
	case "agent_output", "subagent":
		agentID, _ := data["agent_id"].(string)
		output, _ := data["output"].(string)
		if output != "" {
			return StreamEvent{Type: EventTypeText, Text: fmt.Sprintf("[Agent %s] %s\n", agentID, output)}
		}

	// User message - extract tool results
	case "user":
		if message, ok := data["message"].(map[string]any); ok {
			msg := &session.Message{Role: "user"}

			if content, ok := message["content"].([]any); ok {
				var textParts []string
				var toolResults []session.ToolResult

				for _, block := range content {
					if blockMap, ok := block.(map[string]any); ok {
						blockType, _ := blockMap["type"].(string)
						switch blockType {
						case "text":
							if text, ok := blockMap["text"].(string); ok {
								textParts = append(textParts, text)
							}
						case "tool_result":
							toolUseID, _ := blockMap["tool_use_id"].(string)
							resultContent := extractToolResultContent(blockMap["content"])
							isError, _ := blockMap["is_error"].(bool)
							toolResults = append(toolResults, session.ToolResult{
								ToolCallID: toolUseID,
								Content:    resultContent,
								IsError:    isError,
							})
						}
					}
				}

				msg.Content = strings.Join(textParts, "\n")
				if len(toolResults) > 0 {
					msg.ToolResults, _ = json.Marshal(toolResults)
				}
			}

			// Only emit if there's actual content
			if msg.Content != "" || len(msg.ToolResults) > 0 {
				return StreamEvent{Type: EventTypeMessage, Message: msg}
			}
		}
		return StreamEvent{Type: EventTypeText, Text: ""}
	}

	// Silently ignore known envelope types (system, message_start, content_block_start/stop, etc.)

	// Unknown event type - return empty
	return StreamEvent{Type: EventTypeText, Text: ""}
}

// extractToolResultContent extracts text content from a tool_result's content field.
// The Anthropic API allows content to be a string, an array of content blocks, or nil.
func extractToolResultContent(v any) string {
	if v == nil {
		return ""
	}
	// String content
	if s, ok := v.(string); ok {
		return s
	}
	// Array of content blocks: [{"type": "text", "text": "..."}]
	if blocks, ok := v.([]any); ok {
		var parts []string
		for _, block := range blocks {
			if m, ok := block.(map[string]any); ok {
				if text, ok := m["text"].(string); ok {
					parts = append(parts, text)
				}
			}
		}
		return strings.Join(parts, "\n")
	}
	// Fallback: marshal to JSON string
	data, err := json.Marshal(v)
	if err != nil {
		return fmt.Sprintf("%v", v)
	}
	return string(data)
}

// buildPromptFromMessages converts session messages to a single prompt string.
// Merges consecutive same-role messages to avoid fragmented/duplicated text.
func buildPromptFromMessages(messages []session.Message) string {
	var parts []string

	// Merge consecutive same-role messages and deduplicate content
	var lastRole string
	var pendingContent string

	flushPending := func() {
		if pendingContent == "" {
			return
		}
		switch lastRole {
		case "system":
			parts = append(parts, fmt.Sprintf("[System]\n%s", pendingContent))
		case "user":
			parts = append(parts, fmt.Sprintf("[User]\n%s", pendingContent))
		case "assistant":
			parts = append(parts, fmt.Sprintf("[Assistant]\n%s", pendingContent))
		}
		pendingContent = ""
	}

	for _, msg := range messages {
		role := msg.Role
		// Treat "tool" results as user messages (tool results come from user turn)
		if role == "tool" {
			role = "user"
		}

		content := strings.TrimSpace(msg.Content)

		// Handle tool results inline
		if len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			json.Unmarshal(msg.ToolResults, &results)
			for _, r := range results {
				if r.Content != "" {
					if content != "" {
						content += "\n"
					}
					content += fmt.Sprintf("[Tool Result: %s]\n%s", r.ToolCallID, r.Content)
				}
			}
		}

		// Skip completely empty messages
		if content == "" {
			continue
		}

		// If same role as previous, merge content
		if role == lastRole {
			// Deduplicate: skip if the new content is already contained in pending
			if !strings.Contains(pendingContent, content) {
				pendingContent += "\n\n" + content
			}
		} else {
			flushPending()
			lastRole = role
			pendingContent = content
		}
	}
	flushPending()

	return strings.Join(parts, "\n\n")
}

// CheckCLIAvailable checks if a CLI command is available in PATH
func CheckCLIAvailable(command string) bool {
	_, err := exec.LookPath(command)
	return err == nil
}

// GetAvailableCLIProviders returns a list of available CLI providers
func GetAvailableCLIProviders() []string {
	var available []string

	clis := []string{"claude", "gemini", "codex"}
	for _, cli := range clis {
		if CheckCLIAvailable(cli) {
			available = append(available, cli)
		}
	}

	return available
}

// CLIStatus represents the installation and authentication status of a CLI
type CLIStatus struct {
	Installed     bool   `json:"installed"`
	Authenticated bool   `json:"authenticated"`
	Version       string `json:"version,omitempty"`
}

// CheckCLIStatus checks if a CLI is installed and authenticated
func CheckCLIStatus(command string) CLIStatus {
	status := CLIStatus{}

	// Check if installed
	if !CheckCLIAvailable(command) {
		return status
	}
	status.Installed = true

	// Check authentication based on CLI type
	switch command {
	case "claude":
		// Claude CLI: run `claude --version` - returns version if authenticated
		// If not authenticated, it will prompt for login (which we catch via timeout)
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, "claude", "--version")
		output, err := cmd.Output()
		if err == nil {
			status.Authenticated = true
			status.Version = strings.TrimSpace(string(output))
		}

	case "gemini":
		// Gemini CLI: check for auth by running --version or checking config
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, "gemini", "--version")
		output, err := cmd.Output()
		if err == nil {
			status.Authenticated = true
			status.Version = strings.TrimSpace(string(output))
		}

	case "codex":
		// Codex CLI: check for auth by running --version
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		cmd := exec.CommandContext(ctx, "codex", "--version")
		output, err := cmd.Output()
		if err == nil {
			status.Authenticated = true
			status.Version = strings.TrimSpace(string(output))
		}
	}

	return status
}

// GetAllCLIStatuses returns the status of all known CLIs
func GetAllCLIStatuses() map[string]CLIStatus {
	return map[string]CLIStatus{
		"claude": CheckCLIStatus("claude"),
		"codex":  CheckCLIStatus("codex"),
		"gemini": CheckCLIStatus("gemini"),
	}
}
