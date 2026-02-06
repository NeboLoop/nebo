//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SystemTool provides Windows system controls.
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
	return "Control system settings: volume, brightness, Wi-Fi, sleep, lock screen, and get system info."
}

func (t *SystemTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["volume", "brightness", "sleep", "lock", "wifi", "info", "mute", "unmute"],
				"description": "System action to perform"
			},
			"value": {
				"type": "integer",
				"description": "Value for volume/brightness (0-100)"
			},
			"enable": {
				"type": "boolean",
				"description": "Enable or disable for wifi"
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

	// Use nircmd if available, otherwise PowerShell with audio library
	script := fmt.Sprintf(`
$obj = New-Object -ComObject WScript.Shell
$current = 0
for ($i=0; $i -lt 50; $i++) { $obj.SendKeys([char]174) }
$target = [math]::Round(%d / 2)
for ($i=0; $i -lt $target; $i++) { $obj.SendKeys([char]175) }
`, level)

	_, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Volume set to approximately %d%%", level), IsError: false}, nil
}

func (t *SystemTool) setMute(mute bool) (*ToolResult, error) {
	script := `
$obj = New-Object -ComObject WScript.Shell
$obj.SendKeys([char]173)
`
	_, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to toggle mute: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Audio mute toggled", IsError: false}, nil
}

func (t *SystemTool) setBrightness(level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Brightness must be between 0 and 100", IsError: true}, nil
	}

	script := fmt.Sprintf(`
(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods).WmiSetBrightness(1, %d)
`, level)

	_, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set brightness (may not work on desktop monitors): %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Brightness set to %d%%", level), IsError: false}, nil
}

func (t *SystemTool) sleep() (*ToolResult, error) {
	script := `rundll32.exe powrprof.dll,SetSuspendState 0,1,0`
	if err := exec.Command("cmd", "/C", script).Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to sleep: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Putting system to sleep...", IsError: false}, nil
}

func (t *SystemTool) lock() (*ToolResult, error) {
	if err := exec.Command("rundll32.exe", "user32.dll,LockWorkStation").Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to lock screen: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Screen locked", IsError: false}, nil
}

func (t *SystemTool) getWifiStatus() (*ToolResult, error) {
	out, err := exec.Command("netsh", "wlan", "show", "interfaces").Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get Wi-Fi status: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(out)), IsError: false}, nil
}

func (t *SystemTool) setWifi(enable bool) (*ToolResult, error) {
	state := "disabled"
	if enable {
		state = "enabled"
	}

	// Get first wireless adapter name
	script := `Get-NetAdapter -Physical | Where-Object { $_.InterfaceDescription -match 'Wireless|Wi-Fi|WiFi' } | Select-Object -First 1 -ExpandProperty Name`
	adapterOut, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to find Wi-Fi adapter: %v", err), IsError: true}, nil
	}
	adapter := strings.TrimSpace(string(adapterOut))

	var cmd *exec.Cmd
	if enable {
		cmd = exec.Command("powershell", "-NoProfile", "-Command",
			fmt.Sprintf("Enable-NetAdapter -Name '%s' -Confirm:$false", adapter))
	} else {
		cmd = exec.Command("powershell", "-NoProfile", "-Command",
			fmt.Sprintf("Disable-NetAdapter -Name '%s' -Confirm:$false", adapter))
	}

	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set Wi-Fi: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Wi-Fi %s", state), IsError: false}, nil
}

func (t *SystemTool) getSystemInfo() (*ToolResult, error) {
	script := `
$os = Get-WmiObject Win32_OperatingSystem
$cpu = Get-WmiObject Win32_Processor
$mem = [math]::Round($os.TotalVisibleMemorySize / 1MB, 1)
$hostname = $env:COMPUTERNAME
$uptime = (Get-Date) - (Get-CimInstance -ClassName Win32_OperatingSystem).LastBootUpTime

"Hostname: $hostname"
"Windows: $($os.Caption) $($os.Version)"
"CPU: $($cpu.Name)"
"Memory: $mem GB"
"Uptime: $($uptime.Days)d $($uptime.Hours)h $($uptime.Minutes)m"
`
	out, err := exec.Command("powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get system info: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(out)), IsError: false}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewSystemTool(),
		Platforms: []string{PlatformWindows},
		Category:  "system",
	})
}
