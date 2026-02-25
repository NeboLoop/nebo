//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SystemTool provides Linux system controls.
// Volume, brightness, sleep, lock, Wi-Fi, Bluetooth.
type SystemTool struct{}

// NewSystemTool creates a new system control tool
func NewSystemTool() *SystemTool {
	return &SystemTool{}
}

func (t *SystemTool) Name() string {
	return "system"
}

func (t *SystemTool) Description() string {
	return "Control system settings: volume, brightness, Wi-Fi, Bluetooth, sleep, lock screen, and get system info."
}

func (t *SystemTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["volume", "brightness", "sleep", "lock", "wifi", "bluetooth", "info", "mute", "unmute"],
				"description": "System action to perform"
			},
			"value": {
				"type": "integer",
				"description": "Value for volume/brightness (0-100)"
			},
			"enable": {
				"type": "boolean",
				"description": "Enable or disable for wifi/bluetooth"
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

	// Try pactl (PulseAudio) first, then amixer (ALSA)
	if _, err := exec.LookPath("pactl"); err == nil {
		cmd := exec.Command("pactl", "set-sink-volume", "@DEFAULT_SINK@", fmt.Sprintf("%d%%", level))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Volume set to %d%%", level), IsError: false}, nil
	}

	if _, err := exec.LookPath("amixer"); err == nil {
		cmd := exec.Command("amixer", "set", "Master", fmt.Sprintf("%d%%", level))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Volume set to %d%%", level), IsError: false}, nil
	}

	return &ToolResult{Content: "No audio control available (install pulseaudio-utils or alsa-utils)", IsError: true}, nil
}

func (t *SystemTool) setMute(mute bool) (*ToolResult, error) {
	state := "1"
	stateStr := "muted"
	if !mute {
		state = "0"
		stateStr = "unmuted"
	}

	// Try pactl first
	if _, err := exec.LookPath("pactl"); err == nil {
		cmd := exec.Command("pactl", "set-sink-mute", "@DEFAULT_SINK@", state)
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set mute: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Audio %s", stateStr), IsError: false}, nil
	}

	// Try amixer
	if _, err := exec.LookPath("amixer"); err == nil {
		amixerState := "mute"
		if !mute {
			amixerState = "unmute"
		}
		cmd := exec.Command("amixer", "set", "Master", amixerState)
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set mute: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Audio %s", stateStr), IsError: false}, nil
	}

	return &ToolResult{Content: "No audio control available", IsError: true}, nil
}

func (t *SystemTool) setBrightness(level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Brightness must be between 0 and 100", IsError: true}, nil
	}

	// Try brightnessctl first
	if _, err := exec.LookPath("brightnessctl"); err == nil {
		cmd := exec.Command("brightnessctl", "set", fmt.Sprintf("%d%%", level))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set brightness: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Brightness set to %d%%", level), IsError: false}, nil
	}

	// Try xbacklight
	if _, err := exec.LookPath("xbacklight"); err == nil {
		cmd := exec.Command("xbacklight", "-set", fmt.Sprintf("%d", level))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set brightness: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Brightness set to %d%%", level), IsError: false}, nil
	}

	return &ToolResult{Content: "Brightness control unavailable (install brightnessctl or xbacklight)", IsError: true}, nil
}

func (t *SystemTool) sleep() (*ToolResult, error) {
	// Try systemctl first (systemd)
	if _, err := exec.LookPath("systemctl"); err == nil {
		cmd := exec.Command("systemctl", "suspend")
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to sleep: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: "Putting system to sleep...", IsError: false}, nil
	}

	// Try pm-suspend
	if _, err := exec.LookPath("pm-suspend"); err == nil {
		cmd := exec.Command("pm-suspend")
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to sleep: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: "Putting system to sleep...", IsError: false}, nil
	}

	return &ToolResult{Content: "Sleep not available (requires systemd or pm-utils)", IsError: true}, nil
}

func (t *SystemTool) lock() (*ToolResult, error) {
	// Try various screen lockers
	lockers := [][]string{
		{"loginctl", "lock-session"},         // systemd
		{"xdg-screensaver", "lock"},          // generic X11
		{"gnome-screensaver-command", "-l"},  // GNOME
		{"xflock4"},                          // XFCE
		{"i3lock"},                           // i3
		{"slock"},                            // suckless
	}

	for _, locker := range lockers {
		if _, err := exec.LookPath(locker[0]); err == nil {
			cmd := exec.Command(locker[0], locker[1:]...)
			if err := cmd.Start(); err == nil {
				return &ToolResult{Content: "Screen locked", IsError: false}, nil
			}
		}
	}

	return &ToolResult{Content: "No screen locker found (install xdg-screensaver, gnome-screensaver, i3lock, etc.)", IsError: true}, nil
}

func (t *SystemTool) getWifiStatus() (*ToolResult, error) {
	// Try nmcli (NetworkManager)
	if _, err := exec.LookPath("nmcli"); err == nil {
		out, err := exec.Command("nmcli", "-t", "-f", "WIFI", "radio").Output()
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to get Wi-Fi status: %v", err), IsError: true}, nil
		}
		status := strings.TrimSpace(string(out))
		if status == "enabled" {
			// Get connection info
			connOut, _ := exec.Command("nmcli", "-t", "-f", "NAME,TYPE,DEVICE", "connection", "show", "--active").Output()
			return &ToolResult{Content: fmt.Sprintf("Wi-Fi: ON\n%s", string(connOut)), IsError: false}, nil
		}
		return &ToolResult{Content: "Wi-Fi: OFF", IsError: false}, nil
	}

	// Try iwctl (iwd)
	if _, err := exec.LookPath("iwctl"); err == nil {
		out, err := exec.Command("iwctl", "station", "wlan0", "show").Output()
		if err != nil {
			return &ToolResult{Content: "Wi-Fi status unavailable", IsError: false}, nil
		}
		return &ToolResult{Content: strings.TrimSpace(string(out)), IsError: false}, nil
	}

	return &ToolResult{Content: "Wi-Fi status unavailable (NetworkManager or iwd not found)", IsError: false}, nil
}

func (t *SystemTool) setWifi(enable bool) (*ToolResult, error) {
	state := "off"
	if enable {
		state = "on"
	}

	// Try nmcli
	if _, err := exec.LookPath("nmcli"); err == nil {
		cmd := exec.Command("nmcli", "radio", "wifi", state)
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set Wi-Fi: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Wi-Fi turned %s", state), IsError: false}, nil
	}

	// Try rfkill
	if _, err := exec.LookPath("rfkill"); err == nil {
		action := "block"
		if enable {
			action = "unblock"
		}
		cmd := exec.Command("rfkill", action, "wifi")
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set Wi-Fi: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Wi-Fi turned %s", state), IsError: false}, nil
	}

	return &ToolResult{Content: "Wi-Fi control unavailable (install NetworkManager or rfkill)", IsError: true}, nil
}

func (t *SystemTool) getBluetoothStatus() (*ToolResult, error) {
	// Try bluetoothctl
	if _, err := exec.LookPath("bluetoothctl"); err == nil {
		out, err := exec.Command("bluetoothctl", "show").Output()
		if err != nil {
			return &ToolResult{Content: "Bluetooth unavailable", IsError: false}, nil
		}
		output := string(out)
		if strings.Contains(output, "Powered: yes") {
			return &ToolResult{Content: "Bluetooth: ON\n" + output, IsError: false}, nil
		}
		return &ToolResult{Content: "Bluetooth: OFF", IsError: false}, nil
	}

	// Try rfkill
	if _, err := exec.LookPath("rfkill"); err == nil {
		out, err := exec.Command("rfkill", "list", "bluetooth").Output()
		if err != nil {
			return &ToolResult{Content: "Bluetooth unavailable", IsError: false}, nil
		}
		output := string(out)
		if strings.Contains(output, "Soft blocked: yes") || strings.Contains(output, "Hard blocked: yes") {
			return &ToolResult{Content: "Bluetooth: OFF (blocked)", IsError: false}, nil
		}
		return &ToolResult{Content: "Bluetooth: ON", IsError: false}, nil
	}

	return &ToolResult{Content: "Bluetooth status unavailable (install bluez)", IsError: false}, nil
}

func (t *SystemTool) setBluetooth(enable bool) (*ToolResult, error) {
	// Try bluetoothctl
	if _, err := exec.LookPath("bluetoothctl"); err == nil {
		state := "off"
		if enable {
			state = "on"
		}
		cmd := exec.Command("bluetoothctl", "power", state)
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set Bluetooth: %v", err), IsError: true}, nil
		}
		if enable {
			return &ToolResult{Content: "Bluetooth turned on", IsError: false}, nil
		}
		return &ToolResult{Content: "Bluetooth turned off", IsError: false}, nil
	}

	// Try rfkill
	if _, err := exec.LookPath("rfkill"); err == nil {
		action := "block"
		if enable {
			action = "unblock"
		}
		cmd := exec.Command("rfkill", action, "bluetooth")
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to set Bluetooth: %v", err), IsError: true}, nil
		}
		if enable {
			return &ToolResult{Content: "Bluetooth turned on", IsError: false}, nil
		}
		return &ToolResult{Content: "Bluetooth turned off", IsError: false}, nil
	}

	return &ToolResult{Content: "Bluetooth control unavailable (install bluez or rfkill)", IsError: true}, nil
}

func (t *SystemTool) getSystemInfo() (*ToolResult, error) {
	var sb strings.Builder

	// Hostname
	hostname, _ := exec.Command("hostname").Output()
	sb.WriteString(fmt.Sprintf("Hostname: %s\n", strings.TrimSpace(string(hostname))))

	// OS info
	if osRelease, err := exec.Command("sh", "-c", "cat /etc/os-release | grep PRETTY_NAME | cut -d'\"' -f2").Output(); err == nil {
		sb.WriteString(fmt.Sprintf("OS: %s\n", strings.TrimSpace(string(osRelease))))
	}

	// Kernel
	if kernel, err := exec.Command("uname", "-r").Output(); err == nil {
		sb.WriteString(fmt.Sprintf("Kernel: %s\n", strings.TrimSpace(string(kernel))))
	}

	// CPU
	if cpu, err := exec.Command("sh", "-c", "grep 'model name' /proc/cpuinfo | head -1 | cut -d':' -f2").Output(); err == nil {
		sb.WriteString(fmt.Sprintf("CPU: %s\n", strings.TrimSpace(string(cpu))))
	}

	// Memory
	if mem, err := exec.Command("sh", "-c", "free -h | grep Mem | awk '{print $2}'").Output(); err == nil {
		sb.WriteString(fmt.Sprintf("Memory: %s\n", strings.TrimSpace(string(mem))))
	}

	// Uptime
	if uptime, err := exec.Command("uptime", "-p").Output(); err == nil {
		sb.WriteString(fmt.Sprintf("Uptime: %s\n", strings.TrimSpace(string(uptime))))
	}

	return &ToolResult{Content: sb.String(), IsError: false}, nil
}

