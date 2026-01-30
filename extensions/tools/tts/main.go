// TTS Plugin - Text-to-Speech using ElevenLabs API
// Build: go build -o ~/.nebo/plugins/tools/tts
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/rpc"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"time"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type TTSTool struct {
	apiKey string
}

type ttsInput struct {
	Text     string  `json:"text"`      // Text to convert to speech
	Voice    string  `json:"voice"`     // Voice ID (default: "Rachel")
	Output   string  `json:"output"`    // Output file path
	Model    string  `json:"model"`     // Model ID (default: "eleven_monolingual_v1")
	Play     bool    `json:"play"`      // Auto-play the audio
	Speed    float64 `json:"speed"`     // Speech speed (0.5-2.0, default: 1.0)
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

// ElevenLabs voices
var defaultVoices = map[string]string{
	"rachel":   "21m00Tcm4TlvDq8ikWAM",
	"domi":     "AZnzlk1XvdvUeBnXmlld",
	"bella":    "EXAVITQu4vr4xnSDxMaL",
	"antoni":   "ErXwobaYiN019PkySvjV",
	"elli":     "MF3mGyEYCl7XYWbV9V6O",
	"josh":     "TxGEqnHWrfWFTfGW9XjX",
	"arnold":   "VR6AewLTigWG4xSOukaG",
	"adam":     "pNInz6obpgDQGcFmaJgB",
	"sam":      "yoZ06aMxZJJ28mfd3POQ",
}

func (t *TTSTool) Name() string {
	return "tts"
}

func (t *TTSTool) Description() string {
	return "Convert text to speech using ElevenLabs API. Creates high-quality audio files from text."
}

func (t *TTSTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"text": {
				"type": "string",
				"description": "Text to convert to speech"
			},
			"voice": {
				"type": "string",
				"description": "Voice name: rachel, domi, bella, antoni, elli, josh, arnold, adam, sam. Default: rachel"
			},
			"output": {
				"type": "string",
				"description": "Output file path for the audio. Default: ~/.nebo/audio/tts_{timestamp}.mp3"
			},
			"play": {
				"type": "boolean",
				"description": "Auto-play the generated audio. Default: false"
			},
			"speed": {
				"type": "number",
				"description": "Speech speed (0.5-2.0). Default: 1.0"
			}
		},
		"required": ["text"]
	}`)
}

func (t *TTSTool) RequiresApproval() bool {
	return false
}

func (t *TTSTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params ttsInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if params.Text == "" {
		return &ToolResult{Content: "text is required", IsError: true}, nil
	}

	// Get API key
	apiKey := t.apiKey
	if apiKey == "" {
		apiKey = os.Getenv("ELEVENLABS_API_KEY")
	}
	if apiKey == "" {
		return &ToolResult{Content: "ELEVENLABS_API_KEY not set", IsError: true}, nil
	}

	// Set defaults
	voiceID := defaultVoices["rachel"]
	if params.Voice != "" {
		if id, ok := defaultVoices[params.Voice]; ok {
			voiceID = id
		} else {
			// Assume it's a direct voice ID
			voiceID = params.Voice
		}
	}

	model := params.Model
	if model == "" {
		model = "eleven_monolingual_v1"
	}

	speed := params.Speed
	if speed == 0 {
		speed = 1.0
	}

	// Generate output path
	outputPath := params.Output
	if outputPath == "" {
		homeDir, _ := os.UserHomeDir()
		audioDir := filepath.Join(homeDir, ".nebo", "audio")
		os.MkdirAll(audioDir, 0755)
		outputPath = filepath.Join(audioDir, fmt.Sprintf("tts_%s.mp3", time.Now().Format("20060102_150405")))
	}

	// Make API request
	url := fmt.Sprintf("https://api.elevenlabs.io/v1/text-to-speech/%s", voiceID)

	requestBody := map[string]interface{}{
		"text":     params.Text,
		"model_id": model,
		"voice_settings": map[string]interface{}{
			"stability":        0.5,
			"similarity_boost": 0.75,
			"speed":            speed,
		},
	}

	jsonBody, _ := json.Marshal(requestBody)
	req, err := http.NewRequestWithContext(ctx, "POST", url, bytes.NewReader(jsonBody))
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create request: %v", err), IsError: true}, nil
	}

	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("xi-api-key", apiKey)
	req.Header.Set("Accept", "audio/mpeg")

	client := &http.Client{Timeout: 60 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("API request failed: %v", err), IsError: true}, nil
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return &ToolResult{Content: fmt.Sprintf("API error (%d): %s", resp.StatusCode, string(body)), IsError: true}, nil
	}

	// Ensure output directory exists
	if err := os.MkdirAll(filepath.Dir(outputPath), 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create directory: %v", err), IsError: true}, nil
	}

	// Save audio
	file, err := os.Create(outputPath)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create file: %v", err), IsError: true}, nil
	}

	written, err := io.Copy(file, resp.Body)
	file.Close()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to write audio: %v", err), IsError: true}, nil
	}

	result := fmt.Sprintf("Generated audio: %s (%d bytes)", outputPath, written)

	// Auto-play if requested
	if params.Play {
		if err := playAudio(outputPath); err != nil {
			result += fmt.Sprintf("\nFailed to play audio: %v", err)
		} else {
			result += "\nPlaying audio..."
		}
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func playAudio(path string) error {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("afplay", path)
	case "linux":
		cmd = exec.Command("aplay", path)
	case "windows":
		cmd = exec.Command("cmd", "/c", "start", path)
	default:
		return fmt.Errorf("unsupported platform: %s", runtime.GOOS)
	}
	return cmd.Start()
}

// RPC wrapper
type TTSToolRPC struct {
	tool *TTSTool
}

func (t *TTSToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *TTSToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *TTSToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *TTSToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *TTSToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type TTSPlugin struct {
	tool *TTSTool
}

func (p *TTSPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &TTSToolRPC{tool: p.tool}, nil
}

func (p *TTSPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return &TTSToolRPCClient{client: c}, nil
}

type TTSToolRPCClient struct {
	client *rpc.Client
}

func (c *TTSToolRPCClient) Name() string {
	var reply string
	c.client.Call("Plugin.Name", new(interface{}), &reply)
	return reply
}

func (c *TTSToolRPCClient) Description() string {
	var reply string
	c.client.Call("Plugin.Description", new(interface{}), &reply)
	return reply
}

func (c *TTSToolRPCClient) Schema() json.RawMessage {
	var reply json.RawMessage
	c.client.Call("Plugin.Schema", new(interface{}), &reply)
	return reply
}

func (c *TTSToolRPCClient) RequiresApproval() bool {
	var reply bool
	c.client.Call("Plugin.RequiresApproval", new(interface{}), &reply)
	return reply
}

func (c *TTSToolRPCClient) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var reply ToolResult
	err := c.client.Call("Plugin.Execute", &ExecuteArgs{Input: input}, &reply)
	return &reply, err
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &TTSPlugin{tool: &TTSTool{}},
		},
	})
}
