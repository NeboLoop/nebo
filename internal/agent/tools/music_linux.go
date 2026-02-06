//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// MusicTool provides Linux music player control via playerctl (MPRIS) or mpc (MPD).
type MusicTool struct {
	backend string // "playerctl", "mpc", or ""
}

func NewMusicTool() *MusicTool {
	t := &MusicTool{}
	t.backend = t.detectBackend()
	return t
}

func (t *MusicTool) detectBackend() string {
	if _, err := exec.LookPath("playerctl"); err == nil {
		return "playerctl"
	}
	if _, err := exec.LookPath("mpc"); err == nil {
		return "mpc"
	}
	return ""
}

func (t *MusicTool) Name() string { return "music" }

func (t *MusicTool) Description() string {
	if t.backend == "" {
		return "Control Music - requires playerctl (for Spotify/VLC/etc.) or mpc (for MPD) to be installed."
	}
	return fmt.Sprintf("Control Music (using %s) - play, pause, skip tracks, adjust volume, and get now playing info.", t.backend)
}

func (t *MusicTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["play", "pause", "next", "previous", "status", "volume", "shuffle", "players"],
				"description": "Action: play, pause, next, previous, status (now playing), volume, shuffle, players (list available)"
			},
			"query": {"type": "string", "description": "Search query or player name to control"},
			"volume": {"type": "integer", "description": "Volume level 0-100"},
			"shuffle": {"type": "boolean", "description": "Enable or disable shuffle mode"}
		},
		"required": ["action"]
	}`)
}

func (t *MusicTool) RequiresApproval() bool { return false }

type musicInputLinux struct {
	Action  string `json:"action"`
	Query   string `json:"query"`
	Volume  int    `json:"volume"`
	Shuffle *bool  `json:"shuffle"`
}

func (t *MusicTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.backend == "" {
		return &ToolResult{
			Content: "No music control backend available. Please install one of:\n" +
				"  - playerctl: sudo apt install playerctl (controls Spotify, VLC, Firefox, etc.)\n" +
				"  - mpc: sudo apt install mpc (for MPD - Music Player Daemon)",
			IsError: true,
		}, nil
	}

	var p musicInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch t.backend {
	case "playerctl":
		return t.executePlayerctl(ctx, p)
	case "mpc":
		return t.executeMpc(ctx, p)
	default:
		return &ToolResult{Content: "Unknown backend", IsError: true}, nil
	}
}

// ============================================================================
// playerctl implementation (MPRIS)
// ============================================================================

func (t *MusicTool) executePlayerctl(ctx context.Context, p musicInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "play":
		return t.playerctlCommand(ctx, p.Query, "play")
	case "pause":
		return t.playerctlCommand(ctx, p.Query, "pause")
	case "next":
		return t.playerctlCommand(ctx, p.Query, "next")
	case "previous":
		return t.playerctlCommand(ctx, p.Query, "previous")
	case "status":
		return t.playerctlStatus(ctx, p.Query)
	case "volume":
		return t.playerctlVolume(ctx, p.Query, p.Volume)
	case "shuffle":
		return t.playerctlShuffle(ctx, p.Query, p.Shuffle)
	case "players":
		return t.playerctlPlayers(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *MusicTool) playerctlCommand(ctx context.Context, player, action string) (*ToolResult, error) {
	args := []string{}
	if player != "" {
		args = append(args, "--player="+player)
	}
	args = append(args, action)

	cmd := exec.CommandContext(ctx, "playerctl", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "No player") || strings.Contains(output, "No players found") {
			return &ToolResult{Content: "No media player is running"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	actionNames := map[string]string{
		"play":     "Playing",
		"pause":    "Paused",
		"next":     "Skipped to next track",
		"previous": "Skipped to previous track",
	}
	return &ToolResult{Content: actionNames[action]}, nil
}

func (t *MusicTool) playerctlStatus(ctx context.Context, player string) (*ToolResult, error) {
	args := []string{}
	if player != "" {
		args = append(args, "--player="+player)
	}
	args = append(args, "metadata", "--format", "{{status}}: {{artist}} - {{title}}\nAlbum: {{album}}\nVolume: {{volume}}")

	cmd := exec.CommandContext(ctx, "playerctl", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "No player") || strings.Contains(output, "No players found") {
			return &ToolResult{Content: "No media player is running"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *MusicTool) playerctlVolume(ctx context.Context, player string, level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Volume must be 0-100", IsError: true}, nil
	}

	args := []string{}
	if player != "" {
		args = append(args, "--player="+player)
	}
	// playerctl uses 0.0-1.0 scale
	volumeFloat := float64(level) / 100.0
	args = append(args, "volume", fmt.Sprintf("%.2f", volumeFloat))

	cmd := exec.CommandContext(ctx, "playerctl", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Volume set to %d%%", level)}, nil
}

func (t *MusicTool) playerctlShuffle(ctx context.Context, player string, enable *bool) (*ToolResult, error) {
	args := []string{}
	if player != "" {
		args = append(args, "--player="+player)
	}

	if enable != nil {
		if *enable {
			args = append(args, "shuffle", "On")
		} else {
			args = append(args, "shuffle", "Off")
		}
	} else {
		args = append(args, "shuffle", "Toggle")
	}

	cmd := exec.CommandContext(ctx, "playerctl", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	// Check current shuffle status
	statusCmd := exec.CommandContext(ctx, "playerctl", "shuffle")
	statusOut, _ := statusCmd.Output()
	status := strings.TrimSpace(string(statusOut))
	if status == "On" || status == "true" {
		return &ToolResult{Content: "Shuffle: ON"}, nil
	}
	return &ToolResult{Content: "Shuffle: OFF"}, nil
}

func (t *MusicTool) playerctlPlayers(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "playerctl", "--list-all")
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "No player") || output == "" {
			return &ToolResult{Content: "No media players are currently running"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No media players are currently running"}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Available players:\n%s", output)}, nil
}

// ============================================================================
// mpc implementation (MPD)
// ============================================================================

func (t *MusicTool) executeMpc(ctx context.Context, p musicInputLinux) (*ToolResult, error) {
	switch p.Action {
	case "play":
		return t.mpcCommand(ctx, "play")
	case "pause":
		return t.mpcCommand(ctx, "pause")
	case "next":
		return t.mpcCommand(ctx, "next")
	case "previous":
		return t.mpcCommand(ctx, "prev")
	case "status":
		return t.mpcStatus(ctx)
	case "volume":
		return t.mpcVolume(ctx, p.Volume)
	case "shuffle":
		return t.mpcShuffle(ctx, p.Shuffle)
	case "players":
		return &ToolResult{Content: "MPD is the only player when using mpc"}, nil
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *MusicTool) mpcCommand(ctx context.Context, action string) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "mpc", action)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "Connection refused") {
			return &ToolResult{Content: "MPD is not running. Start it with 'systemctl --user start mpd'"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	actionNames := map[string]string{
		"play":  "Playing",
		"pause": "Paused",
		"next":  "Skipped to next track",
		"prev":  "Skipped to previous track",
	}
	return &ToolResult{Content: actionNames[action]}, nil
}

func (t *MusicTool) mpcStatus(ctx context.Context) (*ToolResult, error) {
	cmd := exec.CommandContext(ctx, "mpc", "status")
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if strings.Contains(output, "Connection refused") {
			return &ToolResult{Content: "MPD is not running"}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *MusicTool) mpcVolume(ctx context.Context, level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Volume must be 0-100", IsError: true}, nil
	}

	cmd := exec.CommandContext(ctx, "mpc", "volume", strconv.Itoa(level))
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Volume set to %d%%", level)}, nil
}

func (t *MusicTool) mpcShuffle(ctx context.Context, enable *bool) (*ToolResult, error) {
	action := "random"
	if enable != nil {
		if *enable {
			action = "random on"
		} else {
			action = "random off"
		}
	}

	cmd := exec.CommandContext(ctx, "mpc", action)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if strings.Contains(output, "random: on") {
		return &ToolResult{Content: "Shuffle: ON"}, nil
	}
	return &ToolResult{Content: "Shuffle: OFF"}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewMusicTool(),
		Platforms: []string{PlatformLinux},
		Category:  "media",
	})
}
