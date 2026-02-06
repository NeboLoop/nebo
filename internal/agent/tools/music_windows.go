//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// MusicTool provides Windows music player control via media keys and PowerShell.
type MusicTool struct{}

func NewMusicTool() *MusicTool {
	return &MusicTool{}
}

func (t *MusicTool) Name() string { return "music" }

func (t *MusicTool) Description() string {
	return "Control Music - play, pause, skip tracks using system media keys. Works with Spotify, Windows Media Player, and other media apps."
}

func (t *MusicTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["play", "pause", "toggle", "next", "previous", "stop", "mute", "volume_up", "volume_down"],
				"description": "Action: play, pause, toggle (play/pause), next, previous, stop, mute, volume_up, volume_down"
			},
			"volume": {"type": "integer", "description": "Volume level 0-100 (for volume action)"}
		},
		"required": ["action"]
	}`)
}

func (t *MusicTool) RequiresApproval() bool { return false }

type musicInputWin struct {
	Action string `json:"action"`
	Volume int    `json:"volume"`
}

func (t *MusicTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p musicInputWin
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "play", "pause", "toggle":
		return t.sendMediaKey(ctx, "MEDIA_PLAY_PAUSE", "Toggled play/pause")
	case "next":
		return t.sendMediaKey(ctx, "MEDIA_NEXT_TRACK", "Skipped to next track")
	case "previous":
		return t.sendMediaKey(ctx, "MEDIA_PREV_TRACK", "Skipped to previous track")
	case "stop":
		return t.sendMediaKey(ctx, "MEDIA_STOP", "Stopped playback")
	case "mute":
		return t.sendMediaKey(ctx, "VOLUME_MUTE", "Toggled mute")
	case "volume_up":
		return t.sendMediaKey(ctx, "VOLUME_UP", "Volume increased")
	case "volume_down":
		return t.sendMediaKey(ctx, "VOLUME_DOWN", "Volume decreased")
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *MusicTool) sendMediaKey(ctx context.Context, key, successMsg string) (*ToolResult, error) {
	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.SendKeys]::SendWait("{%s}")
Write-Output "%s"
`, key, successMsg)

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to send media key: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewMusicTool(),
		Platforms: []string{PlatformWindows},
		Category:  "media",
	})
}
