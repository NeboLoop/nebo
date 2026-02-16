package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"runtime"
)

// TTSTool provides text-to-speech using the system's native TTS.
// macOS: `say`, Linux: `espeak`, Windows: PowerShell SpeechSynthesizer.
type TTSTool struct{}

func NewTTSTool() *TTSTool {
	return &TTSTool{}
}

func (t *TTSTool) Name() string {
	return "tts"
}

func (t *TTSTool) Description() string {
	return "Convert text to speech using the system's native voice. Speaks text aloud through the speakers."
}

func (t *TTSTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"text": {
				"type": "string",
				"description": "Text to convert to speech"
			}
		},
		"required": ["text"]
	}`)
}

func (t *TTSTool) RequiresApproval() bool {
	return false
}

type ttsInput struct {
	Text string `json:"text"`
}

func (t *TTSTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params ttsInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if params.Text == "" {
		return &ToolResult{Content: "text is required", IsError: true}, nil
	}

	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.CommandContext(ctx, "say", params.Text)
	case "linux":
		cmd = exec.CommandContext(ctx, "espeak", params.Text)
	case "windows":
		ps := fmt.Sprintf(`Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('%s')`, params.Text)
		cmd = exec.CommandContext(ctx, "powershell", "-Command", ps)
	default:
		return &ToolResult{Content: fmt.Sprintf("TTS not supported on %s", runtime.GOOS), IsError: true}, nil
	}

	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("TTS failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: "Spoke: " + params.Text}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewTTSTool(),
		Platforms: []string{PlatformAll},
		Category:  "media",
	})
}
