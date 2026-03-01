//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// SettingsTool provides Windows system controls.
// Volume, brightness, sleep, lock, Wi-Fi, Bluetooth.
type SettingsTool struct{}

// NewSettingsTool creates a new system control tool
func NewSettingsTool() *SettingsTool {
	return &SettingsTool{}
}

func (t *SettingsTool) Name() string {
	return "system"
}

func (t *SettingsTool) Description() string {
	return "Control system settings: volume, brightness, Wi-Fi, sleep, lock screen, and get system info."
}

func (t *SettingsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["volume", "brightness", "sleep", "lock", "wifi", "darkmode", "info", "mute", "unmute"],
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

func (t *SettingsTool) RequiresApproval() bool {
	return false
}

type systemInput struct {
	Action string `json:"action"`
	Value  int    `json:"value"`
	Enable *bool  `json:"enable"`
}

func (t *SettingsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params systemInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "volume":
		return t.setVolume(ctx, params.Value)
	case "mute":
		return t.setMute(ctx)
	case "unmute":
		return t.setMute(ctx)
	case "brightness":
		return t.setBrightness(ctx, params.Value)
	case "sleep":
		return t.sleep(ctx)
	case "lock":
		return t.lock(ctx)
	case "wifi":
		if params.Enable != nil {
			return t.setWifi(ctx, *params.Enable)
		}
		return t.getWifiStatus(ctx)
	case "darkmode":
		if params.Enable != nil {
			return t.setDarkMode(ctx, *params.Enable)
		}
		return t.getDarkModeStatus(ctx)
	case "info":
		return t.getSystemInfo(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *SettingsTool) setVolume(ctx context.Context, level int) (*ToolResult, error) {
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

	_, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set volume: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Volume set to approximately %d%%", level), IsError: false}, nil
}

func (t *SettingsTool) setMute(ctx context.Context) (*ToolResult, error) {
	script := `
$obj = New-Object -ComObject WScript.Shell
$obj.SendKeys([char]173)
`
	_, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to toggle mute: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Audio mute toggled", IsError: false}, nil
}

func (t *SettingsTool) setBrightness(ctx context.Context, level int) (*ToolResult, error) {
	if level < 0 || level > 100 {
		return &ToolResult{Content: "Brightness must be between 0 and 100", IsError: true}, nil
	}

	script := fmt.Sprintf(`
(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods).WmiSetBrightness(1, %d)
`, level)

	_, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set brightness (may not work on desktop monitors): %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Brightness set to %d%%", level), IsError: false}, nil
}

func (t *SettingsTool) sleep(ctx context.Context) (*ToolResult, error) {
	script := `rundll32.exe powrprof.dll,SetSuspendState 0,1,0`
	if err := exec.CommandContext(ctx, "cmd", "/C", script).Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to sleep: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Putting system to sleep...", IsError: false}, nil
}

func (t *SettingsTool) lock(ctx context.Context) (*ToolResult, error) {
	if err := exec.CommandContext(ctx, "rundll32.exe", "user32.dll,LockWorkStation").Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to lock screen: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: "Screen locked", IsError: false}, nil
}

func (t *SettingsTool) getWifiStatus(ctx context.Context) (*ToolResult, error) {
	out, err := exec.CommandContext(ctx, "netsh", "wlan", "show", "interfaces").Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get Wi-Fi status: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(out)), IsError: false}, nil
}

func (t *SettingsTool) setWifi(ctx context.Context, enable bool) (*ToolResult, error) {
	state := "disabled"
	if enable {
		state = "enabled"
	}

	// Get first wireless adapter name
	script := `Get-NetAdapter -Physical | Where-Object { $_.InterfaceDescription -match 'Wireless|Wi-Fi|WiFi' } | Select-Object -First 1 -ExpandProperty Name`
	adapterOut, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to find Wi-Fi adapter: %v", err), IsError: true}, nil
	}
	adapter := strings.TrimSpace(string(adapterOut))

	var cmd *exec.Cmd
	if enable {
		cmd = exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command",
			fmt.Sprintf("Enable-NetAdapter -Name '%s' -Confirm:$false", adapter))
	} else {
		cmd = exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command",
			fmt.Sprintf("Disable-NetAdapter -Name '%s' -Confirm:$false", adapter))
	}

	if err := cmd.Run(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set Wi-Fi: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Wi-Fi %s", state), IsError: false}, nil
}

func (t *SettingsTool) getSystemInfo(ctx context.Context) (*ToolResult, error) {
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
	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get system info: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: strings.TrimSpace(string(out)), IsError: false}, nil
}

func (t *SettingsTool) getDarkModeStatus(ctx context.Context) (*ToolResult, error) {
	script := `Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -Name AppsUseLightTheme -ErrorAction SilentlyContinue | Select-Object -ExpandProperty AppsUseLightTheme`
	out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get dark mode status: %v", err), IsError: true}, nil
	}
	value := strings.TrimSpace(string(out))
	if value == "0" {
		return &ToolResult{Content: "Dark mode: ON"}, nil
	}
	return &ToolResult{Content: "Dark mode: OFF"}, nil
}

func (t *SettingsTool) setDarkMode(ctx context.Context, enable bool) (*ToolResult, error) {
	value := 1
	if enable {
		value = 0
	}
	script := fmt.Sprintf(`
Set-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -Name AppsUseLightTheme -Value %d -Force
Set-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -Name SystemUsesLightTheme -Value %d -Force
`, value, value)
	_, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to set dark mode: %v", err), IsError: true}, nil
	}
	status := "enabled"
	if !enable {
		status = "disabled"
	}
	return &ToolResult{Content: fmt.Sprintf("Dark mode %s", status)}, nil
}
