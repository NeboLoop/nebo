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

	"gobot/agent/session"
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

// NewClaudeCodeProvider creates a provider that wraps the Claude Code CLI
// Claude Code: brew install claude-code or npm i -g @anthropic-ai/claude-code
// Uses ~/.claude/ for auth, supports extended thinking, MCP, agentic tools
// Runs in dangerously mode for fully autonomous operation (no permission prompts)
// Model is passed via ChatRequest.Model at runtime (defaults to "sonnet" if not specified)
func NewClaudeCodeProvider() *CLIProvider {
	return &CLIProvider{
		name:    "claude-code",
		command: "claude",
		args: []string{
			"--print",                        // Non-interactive output
			"--verbose",                      // Required for stream-json with --print
			"--dangerously-skip-permissions", // Autonomous mode
			"--output-format", "stream-json", // Faster streaming
			"--no-session-persistence",       // Skip disk writes (we manage sessions)
		},
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
		if req.Model != "" && (p.name == "claude-code" || p.name == "codex-cli") {
			args = append(args, "--model", req.Model)
		}

		args = append(args, prompt)

		// Log the full command for debugging (can be run manually)
		fmt.Printf("[CLIProvider] Running: %s %v\n", p.command, args)
		fmt.Printf("[CLIProvider] Prompt length: %d chars\n", len(prompt))
		// Print a runnable command for manual testing (escape prompt)
		escapedPrompt := strings.ReplaceAll(prompt, "\n", "\\n")
		if len(escapedPrompt) > 200 {
			escapedPrompt = escapedPrompt[:200] + "..."
		}
		fmt.Printf("[CLIProvider] Manual test: %s %s \"%s\"\n", p.command, strings.Join(p.args, " "), escapedPrompt)

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

		lineCount := 0
		for scanner.Scan() {
			lineCount++
			select {
			case <-ctx.Done():
				cmd.Process.Kill()
				return
			default:
				line := scanner.Text()
				preview := line
				if len(preview) > 100 {
					preview = preview[:100] + "..."
				}
				fmt.Printf("[CLIProvider] Line %d (len=%d): %s\n", lineCount, len(line), preview)

				// Try to parse as JSON (Claude Code may output structured data)
				event := p.parseLine(line)
				fmt.Printf("[CLIProvider] Parsed event: type=%s text_len=%d\n", event.Type, len(event.Text))
				resultCh <- event
			}
		}
		fmt.Printf("[CLIProvider] Scanning complete, total lines=%d\n", lineCount)

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
				// Tool input being streamed - skip, we get full input at end
				return StreamEvent{Type: EventTypeText, Text: ""}
			}
		}

	// Tool use start
	case "content_block_start":
		if block, ok := data["content_block"].(map[string]any); ok {
			blockType, _ := block["type"].(string)
			if blockType == "tool_use" {
				name, _ := block["name"].(string)
				id, _ := block["id"].(string)
				input, _ := json.Marshal(block["input"])
				return StreamEvent{
					Type: EventTypeToolCall,
					ToolCall: &ToolCall{
						ID:    id,
						Name:  name,
						Input: input,
					},
				}
			}
		}

	// Result message with full content
	case "result":
		if result, ok := data["result"].(string); ok {
			return StreamEvent{Type: EventTypeText, Text: result}
		}
		// Check for subtype in result
		if subtype, ok := data["subtype"].(string); ok && subtype == "error_max_turns" {
			return StreamEvent{Type: EventTypeError, Error: &ProviderError{Message: "max turns reached"}}
		}

	// System messages
	case "system":
		if msg, ok := data["message"].(string); ok {
			return StreamEvent{Type: EventTypeText, Text: "[system] " + msg + "\n"}
		}

	// Assistant message container
	case "assistant":
		// Contains full message, but we stream deltas instead
		return StreamEvent{Type: EventTypeText, Text: ""}

	// Message stop
	case "message_stop", "content_block_stop":
		return StreamEvent{Type: EventTypeText, Text: ""}

	// Error
	case "error":
		msg := fmt.Sprintf("%v", data["error"])
		return StreamEvent{Type: EventTypeError, Error: &ProviderError{Message: msg}}
	}

	// Unknown event type - return empty
	return StreamEvent{Type: EventTypeText, Text: ""}
}

// buildPromptFromMessages converts session messages to a single prompt string
func buildPromptFromMessages(messages []session.Message) string {
	var parts []string

	for _, msg := range messages {
		switch msg.Role {
		case "system":
			parts = append(parts, fmt.Sprintf("[System]\n%s", msg.Content))
		case "user":
			parts = append(parts, fmt.Sprintf("[User]\n%s", msg.Content))
		case "assistant":
			parts = append(parts, fmt.Sprintf("[Assistant]\n%s", msg.Content))
		case "tool":
			// Include tool results in context
			if len(msg.ToolResults) > 0 {
				var results []session.ToolResult
				json.Unmarshal(msg.ToolResults, &results)
				for _, r := range results {
					parts = append(parts, fmt.Sprintf("[Tool Result: %s]\n%s", r.ToolCallID, r.Content))
				}
			}
		}
	}

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
