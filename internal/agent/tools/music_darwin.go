//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// MusicTool provides Apple Music control via AppleScript.
type MusicTool struct{}

func NewMusicTool() *MusicTool { return &MusicTool{} }

func (t *MusicTool) Name() string { return "music" }

func (t *MusicTool) Description() string {
	return "Control Apple Music - play, pause, skip tracks, search, manage playlists, and adjust volume."
}

func (t *MusicTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["play", "pause", "next", "previous", "status", "search", "volume", "playlists", "shuffle"],
				"description": "Action: play, pause, next, previous, status (now playing), search, volume, playlists, shuffle"
			},
			"query": {"type": "string", "description": "Search query or playlist name to play"},
			"volume": {"type": "integer", "description": "Volume level 0-100"},
			"shuffle": {"type": "boolean", "description": "Enable or disable shuffle mode"}
		},
		"required": ["action"]
	}`)
}

func (t *MusicTool) RequiresApproval() bool { return false }

type musicInput struct {
	Action  string `json:"action"`
	Query   string `json:"query"`
	Volume  int    `json:"volume"`
	Shuffle *bool  `json:"shuffle"`
}

func (t *MusicTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p musicInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "play":
		if p.Query != "" {
			return t.playSearch(p.Query)
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
		return t.search(p.Query)
	case "volume":
		return t.setVolume(p.Volume)
	case "playlists":
		return t.listPlaylists()
	case "shuffle":
		if p.Shuffle != nil {
			return t.setShuffle(*p.Shuffle)
		}
		return t.toggleShuffle()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *MusicTool) play() (*ToolResult, error) {
	out, err := execAppleScript(`tell application "Music" to play`)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Playing" + out}, nil
}

func (t *MusicTool) playSearch(query string) (*ToolResult, error) {
	script := fmt.Sprintf(`tell application "Music"
		set foundPlaylists to (every playlist whose name contains "%s")
		if (count of foundPlaylists) > 0 then
			play first item of foundPlaylists
			return "Playing playlist: " & name of first item of foundPlaylists
		end if
		set foundTracks to (every track whose name contains "%s" or artist contains "%s")
		if (count of foundTracks) > 0 then
			play first item of foundTracks
			return "Playing: " & name of first item of foundTracks & " by " & artist of first item of foundTracks
		end if
		return "No matching music found"
	end tell`, escapeAS(query), escapeAS(query), escapeAS(query))
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MusicTool) pause() (*ToolResult, error) {
	_, err := execAppleScript(`tell application "Music" to pause`)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Paused"}, nil
}

func (t *MusicTool) next() (*ToolResult, error) {
	script := `tell application "Music"
		next track
		delay 0.5
		return "Now playing: " & name of current track & " by " & artist of current track
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MusicTool) previous() (*ToolResult, error) {
	script := `tell application "Music"
		previous track
		delay 0.5
		return "Now playing: " & name of current track & " by " & artist of current track
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MusicTool) status() (*ToolResult, error) {
	script := `tell application "Music"
		if player state is playing then
			set info to "Now Playing: " & name of current track & return
			set info to info & "Artist: " & artist of current track & return
			set info to info & "Album: " & album of current track & return
			set info to info & "Volume: " & sound volume & "%"
			return info
		else if player state is paused then
			return "Paused: " & name of current track & " by " & artist of current track
		else
			return "Not playing"
		end if
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MusicTool) search(query string) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "Music"
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
	end tell`, escapeAS(query), escapeAS(query), escapeAS(query))
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No results for '%s'", query)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Results for '%s':\n%s", query, out)}, nil
}

func (t *MusicTool) setVolume(level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Volume must be 0-100", IsError: true}, nil
	}
	script := fmt.Sprintf(`tell application "Music"
		set sound volume to %d
		return "Volume set to %d%%"
	end tell`, level, level)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MusicTool) listPlaylists() (*ToolResult, error) {
	script := `tell application "Music"
		set playlistList to {}
		repeat with p in playlists
			if special kind of p is none then
				set end of playlistList to name of p & " (" & (count of tracks of p) & " tracks)"
			end if
		end repeat
		return playlistList
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Playlists:\n%s", out)}, nil
}

func (t *MusicTool) toggleShuffle() (*ToolResult, error) {
	script := `tell application "Music"
		set shuffle enabled to not shuffle enabled
		if shuffle enabled then return "Shuffle: ON"
		return "Shuffle: OFF"
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MusicTool) setShuffle(enabled bool) (*ToolResult, error) {
	script := fmt.Sprintf(`tell application "Music"
		set shuffle enabled to %t
		if shuffle enabled then return "Shuffle: ON"
		return "Shuffle: OFF"
	end tell`, enabled)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewMusicTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "media",
	})
}
