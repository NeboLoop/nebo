// System Plugin - macOS system controls (volume, brightness, sleep, etc.)
// Build: go build -o ~/.gobot/plugins/tools/system
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

type SystemTool struct{}

type systemInput struct {
	Action     string `json:"action"`     // volume, brightness, sleep, lock, screenshot, wifi, bluetooth, darkmode, info
	Value      int    `json:"value"`      // Value for volume/brightness (0-100)
	Enable     *bool  `json:"enable"`     // Enable/disable for wifi/bluetooth/darkmode
	Path       string `json:"path"`       // Path for screenshot
	Delay      int    `json:"delay"`      // Delay in seconds for screenshot
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *SystemTool) Name() string {
	return "system"
}

func (t *SystemTool) Description() string {
	return "Control macOS system settings - volume, brightness, Wi-Fi, Bluetooth, dark mode, sleep, lock screen, and screenshots."
}

func (t *SystemTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["volume", "brightness", "sleep", "lock", "screenshot", "wifi", "bluetooth", "darkmode", "info", "mute", "unmute"],
				"description": "System action to perform"
			},
			"value": {
				"type": "integer",
				"description": "Value for volume/brightness (0-100)"
			},
			"enable": {
				"type": "boolean",
				"description": "Enable or disable for wifi/bluetooth/darkmode"
			},
			"path": {
				"type": "string",
				"description": "File path for screenshot (default: ~/Desktop)"
			},
			"delay": {
				"type": "integer",
				"description": "Delay in seconds before screenshot"
			}
		},
		"required": ["action"]
	}`)
}

func (t *SystemTool) RequiresApproval() bool {
	return false
}

func (t *SystemTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params systemInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "volume":
		return t.setVolume(params.Value)
	case "mute":
		return t.setMute(true)
	case "unmute":
		return t.setMute(false)
	case "brightness":
		return t.setBrightness(params.Value)
	case "sleep":
		return t.sleep()
	case "lock":
		return t.lock()
	case "screenshot":
		return t.screenshot(params.Path, params.Delay)
	case "wifi":
		if params.Enable != nil {
			return t.setWifi(*params.Enable)
		}
		return t.getWifiStatus()
	case "bluetooth":
		if params.Enable != nil {
			return t.setBluetooth(*params.Enable)
		}
		return t.getBluetoothStatus()
	case "darkmode":
		if params.Enable != nil {
			return t.setDarkMode(*params.Enable)
		}
		return t.getDarkModeStatus()
	case "info":
		return t.getSystemInfo()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *SystemTool) setVolume(level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Volume must be between 0 and 100", IsError: true}, nil
	}

	script := fmt.Sprintf(`set volume output volume %d`, level)
	_, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Volume set to %d%%", level), IsError: false}, nil
}

func (t *SystemTool) setMute(mute bool) (*ToolResult, error) {
	var script string
	if mute {
		script = `set volume with output muted`
	} else {
		script = `set volume without output muted`
	}
	_, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set mute: %v", err), IsError: true}, nil
	}
	if mute {
		return &ToolResult{Content: "Audio muted", IsError: false}, nil
	}
	return &ToolResult{Content: "Audio unmuted", IsError: false}, nil
}

func (t *SystemTool) setBrightness(level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Brightness must be between 0 and 100", IsError: true}, nil
	}

	// Use brightness CLI tool if available, otherwise AppleScript
	script := fmt.Sprintf(`
		tell application "System Events"
			tell appearance preferences
				set brightness to %f
			end tell
		end tell
	`, float64(level)/100.0)
	_, err := runAppleScript(script)
	if err != nil {
		// Try alternative method with brightness command
		brightnessFloat := float64(level) / 100.0
		cmd := exec.Command("brightness", fmt.Sprintf("%f", brightnessFloat))
		if err2 := cmd.Run(); err2 != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set brightness (try: brew install brightness): %v", err), IsError: true}, nil
		}
	}
	return &ToolResult{Content: fmt.Sprintf("Brightness set to %d%%", level), IsError: false}, nil
}

func (t *SystemTool) sleep() (*ToolResult, error) {
	cmd := exec.Command("pmset", "sleepnow")
	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to sleep: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Putting system to sleep...", IsError: false}, nil
}

func (t *SystemTool) lock() (*ToolResult, error) {
	script := `tell application "System Events" to keystroke "q" using {command down, control down}`
	_, err := runAppleScript(script)
	if err != nil {
		// Alternative method
		cmd := exec.Command("pmset", "displaysleepnow")
		if err2 := cmd.Run(); err2 != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to lock screen: %v", err), IsError: true}, nil
		}
	}
	return &ToolResult{Content: "Screen locked", IsError: false}, nil
}

func (t *SystemTool) screenshot(path string, delay int) (*ToolResult, error) {
	args := []string{}
	if delay > 0 {
		args = append(args, "-T", fmt.Sprintf("%d", delay))
	}
	if path != "" {
		args = append(args, path)
	}

	cmd := exec.Command("screencapture", args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Screenshot failed: %v\n%s", err, string(output)), IsError: true}, nil
	}

	if path != "" {
		return &ToolResult{Content: fmt.Sprintf("Screenshot saved to %s", path), IsError: false}, nil
	}
	return &ToolResult{Content: "Screenshot captured", IsError: false}, nil
}

func (t *SystemTool) getWifiStatus() (*ToolResult, error) {
	cmd := exec.Command("networksetup", "-getairportpower", "en0")
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get Wi-Fi status: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(output)), IsError: false}, nil
}

func (t *SystemTool) setWifi(enable bool) (*ToolResult, error) {
	state := "off"
	if enable {
		state = "on"
	}
	cmd := exec.Command("networksetup", "-setairportpower", "en0", state)
	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set Wi-Fi: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Wi-Fi turned %s", state), IsError: false}, nil
}

func (t *SystemTool) getBluetoothStatus() (*ToolResult, error) {
	script := `
		tell application "System Events"
			tell application process "SystemUIServer"
				if exists menu bar item "Bluetooth" of menu bar 1 then
					return "Bluetooth available"
				else
					return "Bluetooth status unavailable"
				end if
			end tell
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get Bluetooth status: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *SystemTool) setBluetooth(enable bool) (*ToolResult, error) {
	// blueutil is required: brew install blueutil
	state := "0"
	if enable {
		state = "1"
	}
	cmd := exec.Command("blueutil", "-p", state)
	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set Bluetooth (try: brew install blueutil): %v", err), IsError: true}, nil
	}
	if enable {
		return &ToolResult{Content: "Bluetooth turned on", IsError: false}, nil
	}
	return &ToolResult{Content: "Bluetooth turned off", IsError: false}, nil
}

func (t *SystemTool) getDarkModeStatus() (*ToolResult, error) {
	script := `
		tell application "System Events"
			tell appearance preferences
				if dark mode then
					return "Dark mode: ON"
				else
					return "Dark mode: OFF"
				end if
			end tell
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get dark mode status: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *SystemTool) setDarkMode(enable bool) (*ToolResult, error) {
	script := fmt.Sprintf(`
		tell application "System Events"
			tell appearance preferences
				set dark mode to %t
			end tell
		end tell
	`, enable)
	_, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set dark mode: %v", err), IsError: true}, nil
	}
	if enable {
		return &ToolResult{Content: "Dark mode enabled", IsError: false}, nil
	}
	return &ToolResult{Content: "Dark mode disabled", IsError: false}, nil
}

func (t *SystemTool) getSystemInfo() (*ToolResult, error) {
	script := `
		set cpuInfo to do shell script "sysctl -n machdep.cpu.brand_string"
		set memInfo to do shell script "sysctl -n hw.memsize"
		set memGB to (memInfo as number) / 1073741824
		set osVer to do shell script "sw_vers -productVersion"
		set hostname to do shell script "hostname"
		set uptime to do shell script "uptime | sed 's/.*up //' | sed 's/,.*//' | xargs"

		return "Hostname: " & hostname & return & "macOS: " & osVer & return & "CPU: " & cpuInfo & return & "Memory: " & (round memGB) & " GB" & return & "Uptime: " & uptime
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get system info: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func runAppleScript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

// RPC wrapper
type SystemToolRPC struct {
	tool *SystemTool
}

func (t *SystemToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *SystemToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *SystemToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *SystemToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *SystemToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type SystemPlugin struct {
	tool *SystemTool
}

func (p *SystemPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &SystemToolRPC{tool: p.tool}, nil
}

func (p *SystemPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &SystemPlugin{tool: &SystemTool{}},
		},
	})
}
