// Music Plugin - macOS Music.app (Apple Music) integration via AppleScript
// Build: go build -o ~/.nebo/plugins/tools/music
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/rpc"
	"os/exec"
	"strings"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type MusicTool struct{}

type musicInput struct {
	Action   string `json:"action"`   // play, pause, next, previous, status, search, volume, playlist
	Query    string `json:"query"`    // Search query or playlist name
	Volume   int    `json:"volume"`   // Volume level (0-100)
	Shuffle  *bool  `json:"shuffle"`  // Enable/disable shuffle
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *MusicTool) Name() string {
	return "music"
}

func (t *MusicTool) Description() string {
	return "Control Apple Music - play, pause, skip tracks, search music, manage playlists, and adjust volume."
}

func (t *MusicTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["play", "pause", "next", "previous", "status", "search", "volume", "playlists", "shuffle"],
				"description": "Action: play, pause, next (skip), previous, status (now playing), search, volume, playlists (list), shuffle"
			},
			"query": {
				"type": "string",
				"description": "Search query for songs/artists/albums, or playlist name to play"
			},
			"volume": {
				"type": "integer",
				"description": "Volume level 0-100 (for volume action)"
			},
			"shuffle": {
				"type": "boolean",
				"description": "Enable or disable shuffle mode"
			}
		},
		"required": ["action"]
	}`)
}

func (t *MusicTool) RequiresApproval() bool {
	return false
}

func (t *MusicTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params musicInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "play":
		if params.Query != "" {
			return t.playSearch(params.Query)
		}
		return t.play()
	case "pause":
		return t.pause()
	case "next":
		return t.next()
	case "previous":
		return t.previous()
	case "status":
		return t.status()
	case "search":
		return t.search(params.Query)
	case "volume":
		return t.setVolume(params.Volume)
	case "playlists":
		return t.listPlaylists()
	case "shuffle":
		if params.Shuffle != nil {
			return t.setShuffle(*params.Shuffle)
		}
		return t.toggleShuffle()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *MusicTool) play() (*ToolResult, error) {
	script := `
		tell application "Music"
			play
			return "Playing"
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to play: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) playSearch(query string) (*ToolResult, error) {
	// First try to find a playlist
	script := fmt.Sprintf(`
		tell application "Music"
			set foundPlaylists to (every playlist whose name contains "%s")
			if (count of foundPlaylists) > 0 then
				play first item of foundPlaylists
				return "Playing playlist: " & name of first item of foundPlaylists
			end if

			-- Search in library
			set foundTracks to (every track whose name contains "%s" or artist contains "%s")
			if (count of foundTracks) > 0 then
				play first item of foundTracks
				return "Playing: " & name of first item of foundTracks & " by " & artist of first item of foundTracks
			end if

			return "No matching music found"
		end tell
	`, escapeAppleScript(query), escapeAppleScript(query), escapeAppleScript(query))

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to play: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) pause() (*ToolResult, error) {
	script := `
		tell application "Music"
			pause
			return "Paused"
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to pause: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) next() (*ToolResult, error) {
	script := `
		tell application "Music"
			next track
			delay 0.5
			set trackName to name of current track
			set artistName to artist of current track
			return "Now playing: " & trackName & " by " & artistName
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to skip: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) previous() (*ToolResult, error) {
	script := `
		tell application "Music"
			previous track
			delay 0.5
			set trackName to name of current track
			set artistName to artist of current track
			return "Now playing: " & trackName & " by " & artistName
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to go back: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) status() (*ToolResult, error) {
	script := `
		tell application "Music"
			if player state is playing then
				set trackName to name of current track
				set artistName to artist of current track
				set albumName to album of current track
				set pos to player position
				set dur to duration of current track
				set vol to sound volume

				set statusInfo to "Now Playing: " & trackName & return
				set statusInfo to statusInfo & "Artist: " & artistName & return
				set statusInfo to statusInfo & "Album: " & albumName & return
				set statusInfo to statusInfo & "Position: " & (round pos) & "/" & (round dur) & " seconds" & return
				set statusInfo to statusInfo & "Volume: " & vol & "%"
				return statusInfo
			else if player state is paused then
				return "Paused: " & name of current track & " by " & artist of current track
			else
				return "Not playing"
			end if
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get status: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) search(query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
		tell application "Music"
			set foundTracks to (every track whose name contains "%s" or artist contains "%s" or album contains "%s")
			set resultList to {}
			set maxResults to 10
			set resultCount to 0
			repeat with t in foundTracks
				if resultCount < maxResults then
					set end of resultList to name of t & " - " & artist of t & " (" & album of t & ")"
					set resultCount to resultCount + 1
				end if
			end repeat
			return resultList
		end tell
	`, escapeAppleScript(query), escapeAppleScript(query), escapeAppleScript(query))

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v", err), IsError: true}, nil
	}
	if output == "" || output == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No results found for '%s'", query), IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Search results for '%s':\n%s", query, output), IsError: false}, nil
}

func (t *MusicTool) setVolume(level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Volume must be between 0 and 100", IsError: true}, nil
	}

	script := fmt.Sprintf(`
		tell application "Music"
			set sound volume to %d
			return "Volume set to %d%%"
		end tell
	`, level, level)

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) listPlaylists() (*ToolResult, error) {
	script := `
		tell application "Music"
			set playlistList to {}
			repeat with p in playlists
				if special kind of p is none then
					set playlistInfo to name of p & " (" & (count of tracks of p) & " tracks)"
					set end of playlistList to playlistInfo
				end if
			end repeat
			return playlistList
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list playlists: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Playlists:\n%s", output), IsError: false}, nil
}

func (t *MusicTool) toggleShuffle() (*ToolResult, error) {
	script := `
		tell application "Music"
			set shuffle enabled to not shuffle enabled
			if shuffle enabled then
				return "Shuffle: ON"
			else
				return "Shuffle: OFF"
			end if
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to toggle shuffle: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MusicTool) setShuffle(enabled bool) (*ToolResult, error) {
	script := fmt.Sprintf(`
		tell application "Music"
			set shuffle enabled to %t
			if shuffle enabled then
				return "Shuffle: ON"
			else
				return "Shuffle: OFF"
			end if
		end tell
	`, enabled)
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set shuffle: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func escapeAppleScript(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return s
}

func runAppleScript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

// RPC wrapper
type MusicToolRPC struct {
	tool *MusicTool
}

func (t *MusicToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *MusicToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *MusicToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *MusicToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *MusicToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type MusicPlugin struct {
	tool *MusicTool
}

func (p *MusicPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &MusicToolRPC{tool: p.tool}, nil
}

func (p *MusicPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &MusicPlugin{tool: &MusicTool{}},
		},
	})
}
