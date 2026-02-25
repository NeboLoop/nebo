//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SystemTool provides macOS system controls.
// Volume, brightness, sleep, lock, Wi-Fi, Bluetooth, dark mode.
type SystemTool struct{}

// NewSystemTool creates a new system control tool
func NewSystemTool() *SystemTool {
	return &SystemTool{}
}

func (t *SystemTool) Name() string {
	return "system"
}

func (t *SystemTool) Description() string {
	return "Control system settings: volume, brightness, Wi-Fi, Bluetooth, dark mode, sleep, lock screen, and get system info."
}

func (t *SystemTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["volume", "brightness", "sleep", "lock", "wifi", "bluetooth", "darkmode", "info", "mute", "unmute"],
				"description": "System action to perform"
			},
			"value": {
				"type": "integer",
				"description": "Value for volume/brightness (0-100)"
			},
			"enable": {
				"type": "boolean",
				"description": "Enable or disable for wifi/bluetooth/darkmode"
			}
		},
		"required": ["action"]
	}`)
}

func (t *SystemTool) RequiresApproval() bool {
	return false
}

type systemInput struct {
	Action string `json:"action"`
	Value  int    `json:"value"`
	Enable *bool  `json:"enable"`
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
	if _, err := runOsascript(script); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Volume set to %d%%", level), IsError: false}, nil
}

func (t *SystemTool) setMute(mute bool) (*ToolResult, error) {
	script := `set volume with output muted`
	if !mute {
		script = `set volume without output muted`
	}
	if _, err := runOsascript(script); err != nil {
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
	// Try brightness CLI tool
	brightnessFloat := float64(level) / 100.0
	cmd := exec.Command("brightness", fmt.Sprintf("%f", brightnessFloat))
	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set brightness (try: brew install brightness): %v", err), IsError: true}, nil
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
	cmd := exec.Command("pmset", "displaysleepnow")
	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to lock screen: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Screen locked", IsError: false}, nil
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
	cmd := exec.Command("blueutil", "-p")
	output, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: "Bluetooth status unavailable (try: brew install blueutil)", IsError: false}, nil
	}
	status := strings.TrimSpace(string(output))
	if status == "1" {
		return &ToolResult{Content: "Bluetooth: ON", IsError: false}, nil
	}
	return &ToolResult{Content: "Bluetooth: OFF", IsError: false}, nil
}

func (t *SystemTool) setBluetooth(enable bool) (*ToolResult, error) {
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
	output, err := runOsascript(script)
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
	if _, err := runOsascript(script); err != nil {
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
	output, err := runOsascript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get system info: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func runOsascript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

