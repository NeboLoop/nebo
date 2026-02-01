package tools

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"time"
)

// TTSTool provides text-to-speech using ElevenLabs API.
// Cross-platform: API calls work everywhere, playback is platform-specific.
type TTSTool struct {
	apiKey string
}

// NewTTSTool creates a new TTS tool
func NewTTSTool() *TTSTool {
	return &TTSTool{
		apiKey: os.Getenv("ELEVENLABS_API_KEY"),
	}
}

// ElevenLabs voice IDs
var elevenLabsVoices = map[string]string{
	"rachel":  "21m00Tcm4TlvDq8ikWAM",
	"domi":    "AZnzlk1XvdvUeBnXmlld",
	"bella":   "EXAVITQu4vr4xnSDxMaL",
	"antoni":  "ErXwobaYiN019PkySvjV",
	"elli":    "MF3mGyEYCl7XYWbV9V6O",
	"josh":    "TxGEqnHWrfWFTfGW9XjX",
	"arnold":  "VR6AewLTigWG4xSOukaG",
	"adam":    "pNInz6obpgDQGcFmaJgB",
	"sam":     "yoZ06aMxZJJ28mfd3POQ",
}

func (t *TTSTool) Name() string {
	return "tts"
}

func (t *TTSTool) Description() string {
	return "Convert text to speech using ElevenLabs API. Creates high-quality audio from text. Requires ELEVENLABS_API_KEY."
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
				"description": "Voice: rachel, domi, bella, antoni, elli, josh, arnold, adam, sam. Default: rachel"
			},
			"output": {
				"type": "string",
				"description": "Output file path. Default: ~/.nebo/audio/tts_{timestamp}.mp3"
			},
			"play": {
				"type": "boolean",
				"description": "Auto-play the generated audio"
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

type ttsInput struct {
	Text   string  `json:"text"`
	Voice  string  `json:"voice"`
	Output string  `json:"output"`
	Play   bool    `json:"play"`
	Speed  float64 `json:"speed"`
}

func (t *TTSTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params ttsInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if params.Text == "" {
		return &ToolResult{Content: "text is required", IsError: true}, nil
	}

	apiKey := t.apiKey
	if apiKey == "" {
		apiKey = os.Getenv("ELEVENLABS_API_KEY")
	}
	if apiKey == "" {
		return &ToolResult{Content: "ELEVENLABS_API_KEY not set", IsError: true}, nil
	}

	// Resolve voice ID
	voiceID := elevenLabsVoices["rachel"]
	if params.Voice != "" {
		if id, ok := elevenLabsVoices[params.Voice]; ok {
			voiceID = id
		} else {
			voiceID = params.Voice // Assume direct voice ID
		}
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

	requestBody := map[string]any{
		"text":     params.Text,
		"model_id": "eleven_monolingual_v1",
		"voice_settings": map[string]any{
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
		if err := playAudioFile(outputPath); err != nil {
			result += fmt.Sprintf("\nFailed to play audio: %v", err)
		} else {
			result += "\nPlaying audio..."
		}
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func playAudioFile(path string) error {
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

func init() {
	// TTS is available everywhere (just needs API key)
	RegisterCapability(&Capability{
		Tool:          NewTTSTool(),
		Platforms:     []string{PlatformAll},
		Category:      "media",
		RequiresSetup: true, // Needs ELEVENLABS_API_KEY
	})
}
