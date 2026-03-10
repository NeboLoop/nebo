use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Settings tool: system settings like volume, brightness, wifi, bluetooth, battery.
/// Cross-platform: macOS, Linux, Windows.
pub struct SettingsTool;

impl SettingsTool {
    pub fn new() -> Self {
        Self
    }
}

impl DynTool for SettingsTool {
    fn name(&self) -> &str {
        "settings"
    }

    fn description(&self) -> String {
        "Read and control system settings.\n\n\
         Resources:\n\
         - volume: get, set (value 0-100)\n\
         - brightness: get, set (value 0-100)\n\
         - wifi: status, toggle\n\
         - bluetooth: status, toggle\n\
         - battery: status\n\
         - darkmode: status, toggle\n\
         - sleep: trigger\n\
         - lock: trigger\n\
         - info: get\n\
         - mute: trigger\n\
         - unmute: trigger\n\n\
         Examples:\n  \
         settings(resource: \"volume\", action: \"set\", value: 50)\n  \
         settings(resource: \"brightness\", action: \"get\")\n  \
         settings(resource: \"battery\", action: \"status\")\n  \
         settings(resource: \"darkmode\", action: \"toggle\")\n  \
         settings(resource: \"sleep\", action: \"trigger\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "System setting resource",
                    "enum": ["volume", "brightness", "wifi", "bluetooth", "battery",
                             "darkmode", "sleep", "lock", "info", "mute", "unmute"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["get", "set", "status", "toggle", "trigger"]
                },
                "value": {
                    "type": "integer",
                    "description": "Value to set (0-100 for volume/brightness)"
                }
            },
            "required": ["resource", "action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let resource = input["resource"].as_str().unwrap_or("");
            let action = input["action"].as_str().unwrap_or("");

            match resource {
                "volume" => handle_volume(action, &input).await,
                "brightness" => handle_brightness(action, &input).await,
                "wifi" => handle_wifi(action).await,
                "bluetooth" => handle_bluetooth(action).await,
                "battery" => handle_battery().await,
                "darkmode" => handle_darkmode(action).await,
                "sleep" => handle_sleep().await,
                "lock" => handle_lock().await,
                "info" => handle_info().await,
                "mute" => handle_mute(true).await,
                "unmute" => handle_mute(false).await,
                _ => ToolResult::error(format!(
                    "Unknown resource '{}'. Use: volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute, unmute",
                    resource
                )),
            }
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// macOS implementations
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn handle_volume(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "get" => run_osascript("output volume of (get volume settings)").await,
        "set" => {
            let value = input["value"].as_i64().unwrap_or(50).clamp(0, 100);
            run_osascript(&format!("set volume output volume {}", value)).await
        }
        _ => ToolResult::error(format!("Unknown volume action '{}'. Use: get, set", action)),
    }
}

#[cfg(target_os = "macos")]
async fn handle_brightness(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "get" => match macos_native::brightness_get() {
            Ok(b) => ToolResult::ok(format!("{:.0}%", b * 100.0)),
            Err(e) => ToolResult::error(format!("Failed to get brightness: {}", e)),
        },
        "set" => {
            let value = input["value"].as_i64().unwrap_or(50).clamp(0, 100);
            let normalized = (value as f32) / 100.0;
            match macos_native::brightness_set(normalized) {
                Ok(_) => ToolResult::ok(format!("Brightness set to {}%", value)),
                Err(e) => ToolResult::error(format!("Failed to set brightness: {}", e)),
            }
        }
        _ => ToolResult::error(format!("Unknown brightness action '{}'. Use: get, set", action)),
    }
}

#[cfg(target_os = "macos")]
async fn handle_wifi(action: &str) -> ToolResult {
    match action {
        "status" => run_command("networksetup", &["-getairportpower", "en0"]).await,
        "toggle" => {
            let output = tokio::process::Command::new("networksetup")
                .args(["-getairportpower", "en0"])
                .output()
                .await;
            match output {
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    let new_state = if text.contains("On") { "off" } else { "on" };
                    run_command("networksetup", &["-setairportpower", "en0", new_state]).await
                }
                Err(e) => ToolResult::error(format!("Failed to check WiFi: {}", e)),
            }
        }
        _ => ToolResult::error(format!("Unknown wifi action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "macos")]
async fn handle_bluetooth(action: &str) -> ToolResult {
    match action {
        "status" => {
            let on = macos_native::bluetooth_is_on();
            ToolResult::ok(if on { "Bluetooth: On" } else { "Bluetooth: Off" }.to_string())
        }
        "toggle" => {
            let currently_on = macos_native::bluetooth_is_on();
            macos_native::bluetooth_set(!currently_on);
            std::thread::sleep(std::time::Duration::from_millis(500));
            let new_state = macos_native::bluetooth_is_on();
            ToolResult::ok(format!("Bluetooth: {}", if new_state { "On" } else { "Off" }))
        }
        _ => ToolResult::error(format!("Unknown bluetooth action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "macos")]
async fn handle_battery() -> ToolResult {
    run_command("pmset", &["-g", "batt"]).await
}

#[cfg(target_os = "macos")]
async fn handle_darkmode(action: &str) -> ToolResult {
    match action {
        "status" => {
            run_osascript(
                "tell application \"System Events\" to tell appearance preferences to \
                 if dark mode then return \"Dark mode: ON\" else return \"Dark mode: OFF\"",
            )
            .await
        }
        "toggle" => {
            // Get current, then set opposite
            let check = tokio::process::Command::new("osascript")
                .args(["-e", "tell application \"System Events\" to tell appearance preferences to return dark mode"])
                .output()
                .await;
            let enable = match check {
                Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "false",
                Err(_) => true,
            };
            run_osascript(&format!(
                "tell application \"System Events\" to tell appearance preferences to set dark mode to {}",
                enable
            ))
            .await
        }
        _ => ToolResult::error(format!("Unknown darkmode action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "macos")]
async fn handle_sleep() -> ToolResult {
    run_command("pmset", &["sleepnow"]).await
}

#[cfg(target_os = "macos")]
async fn handle_lock() -> ToolResult {
    run_command("pmset", &["displaysleepnow"]).await
}

#[cfg(target_os = "macos")]
async fn handle_info() -> ToolResult {
    let script = r#"
set cpuInfo to do shell script "sysctl -n machdep.cpu.brand_string"
set memInfo to do shell script "sysctl -n hw.memsize"
set memGB to (memInfo as number) / 1073741824
set osVer to do shell script "sw_vers -productVersion"
set hostname to do shell script "hostname"
set uptime to do shell script "uptime | sed 's/.*up //' | sed 's/,.*//' | xargs"
return "Hostname: " & hostname & return & "macOS: " & osVer & return & "CPU: " & cpuInfo & return & "Memory: " & (round memGB) & " GB" & return & "Uptime: " & uptime
"#;
    run_osascript(script).await
}

#[cfg(target_os = "macos")]
async fn handle_mute(mute: bool) -> ToolResult {
    if mute {
        run_osascript("set volume with output muted").await
    } else {
        run_osascript("set volume without output muted").await
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Linux implementations
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
async fn handle_volume(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "get" => {
            // Try pactl first, then amixer
            if which("pactl") {
                run_command("pactl", &["get-sink-volume", "@DEFAULT_SINK@"]).await
            } else if which("amixer") {
                run_command("amixer", &["get", "Master"]).await
            } else {
                ToolResult::error("No audio control available (install pulseaudio-utils or alsa-utils)")
            }
        }
        "set" => {
            let value = input["value"].as_i64().unwrap_or(50).clamp(0, 100);
            let pct = format!("{}%", value);
            if which("pactl") {
                run_command("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &pct]).await
            } else if which("amixer") {
                run_command("amixer", &["set", "Master", &pct]).await
            } else {
                ToolResult::error("No audio control available (install pulseaudio-utils or alsa-utils)")
            }
        }
        _ => ToolResult::error(format!("Unknown volume action '{}'. Use: get, set", action)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_brightness(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "get" => {
            if which("brightnessctl") {
                run_command("brightnessctl", &["get"]).await
            } else if which("xbacklight") {
                run_command("xbacklight", &["-get"]).await
            } else {
                ToolResult::error("Brightness control unavailable (install brightnessctl or xbacklight)")
            }
        }
        "set" => {
            let value = input["value"].as_i64().unwrap_or(50).clamp(0, 100);
            let pct = format!("{}%", value);
            if which("brightnessctl") {
                run_command("brightnessctl", &["set", &pct]).await
            } else if which("xbacklight") {
                run_command("xbacklight", &["-set", &format!("{}", value)]).await
            } else {
                ToolResult::error("Brightness control unavailable (install brightnessctl or xbacklight)")
            }
        }
        _ => ToolResult::error(format!("Unknown brightness action '{}'. Use: get, set", action)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_wifi(action: &str) -> ToolResult {
    match action {
        "status" => {
            if which("nmcli") {
                run_command("nmcli", &["-t", "-f", "WIFI", "radio"]).await
            } else if which("iwctl") {
                run_command("iwctl", &["station", "wlan0", "show"]).await
            } else {
                ToolResult::error("Wi-Fi status unavailable (NetworkManager or iwd not found)")
            }
        }
        "toggle" => {
            if which("nmcli") {
                // Check current state
                let output = tokio::process::Command::new("nmcli")
                    .args(["-t", "-f", "WIFI", "radio"])
                    .output()
                    .await;
                let new_state = match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        if text.trim() == "enabled" { "off" } else { "on" }
                    }
                    Err(_) => "on",
                };
                run_command("nmcli", &["radio", "wifi", new_state]).await
            } else if which("rfkill") {
                // Toggle via rfkill — check current and flip
                let output = tokio::process::Command::new("rfkill")
                    .args(["list", "wifi"])
                    .output()
                    .await;
                let action = match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        if text.contains("Soft blocked: yes") { "unblock" } else { "block" }
                    }
                    Err(_) => "unblock",
                };
                run_command("rfkill", &[action, "wifi"]).await
            } else {
                ToolResult::error("Wi-Fi control unavailable (install NetworkManager or rfkill)")
            }
        }
        _ => ToolResult::error(format!("Unknown wifi action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_bluetooth(action: &str) -> ToolResult {
    match action {
        "status" => {
            if which("bluetoothctl") {
                let output = tokio::process::Command::new("bluetoothctl")
                    .arg("show")
                    .output()
                    .await;
                match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        if text.contains("Powered: yes") {
                            ToolResult::ok(format!("Bluetooth: ON\n{}", text.trim()))
                        } else {
                            ToolResult::ok("Bluetooth: OFF".to_string())
                        }
                    }
                    Err(_) => ToolResult::ok("Bluetooth unavailable".to_string()),
                }
            } else if which("rfkill") {
                let output = tokio::process::Command::new("rfkill")
                    .args(["list", "bluetooth"])
                    .output()
                    .await;
                match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        if text.contains("Soft blocked: yes") || text.contains("Hard blocked: yes") {
                            ToolResult::ok("Bluetooth: OFF (blocked)".to_string())
                        } else {
                            ToolResult::ok("Bluetooth: ON".to_string())
                        }
                    }
                    Err(_) => ToolResult::ok("Bluetooth unavailable".to_string()),
                }
            } else {
                ToolResult::ok("Bluetooth status unavailable (install bluez)".to_string())
            }
        }
        "toggle" => {
            if which("bluetoothctl") {
                // Check current state
                let output = tokio::process::Command::new("bluetoothctl")
                    .arg("show")
                    .output()
                    .await;
                let new_state = match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        if text.contains("Powered: yes") { "off" } else { "on" }
                    }
                    Err(_) => "on",
                };
                run_command("bluetoothctl", &["power", new_state]).await
            } else if which("rfkill") {
                let output = tokio::process::Command::new("rfkill")
                    .args(["list", "bluetooth"])
                    .output()
                    .await;
                let action = match output {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        if text.contains("Soft blocked: yes") { "unblock" } else { "block" }
                    }
                    Err(_) => "unblock",
                };
                run_command("rfkill", &[action, "bluetooth"]).await
            } else {
                ToolResult::error("Bluetooth control unavailable (install bluez or rfkill)")
            }
        }
        _ => ToolResult::error(format!("Unknown bluetooth action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "linux")]
async fn handle_battery() -> ToolResult {
    if which("upower") {
        run_command("upower", &["-i", "/org/freedesktop/UPower/devices/battery_BAT0"]).await
    } else {
        // Read directly from sysfs
        let capacity = std::fs::read_to_string("/sys/class/power_supply/BAT0/capacity")
            .unwrap_or_else(|_| "unknown".to_string());
        let status = std::fs::read_to_string("/sys/class/power_supply/BAT0/status")
            .unwrap_or_else(|_| "unknown".to_string());
        ToolResult::ok(format!("Battery: {}% ({})", capacity.trim(), status.trim()))
    }
}

#[cfg(target_os = "linux")]
async fn handle_darkmode(_action: &str) -> ToolResult {
    ToolResult::error("Dark mode control is not supported on Linux")
}

#[cfg(target_os = "linux")]
async fn handle_sleep() -> ToolResult {
    if which("systemctl") {
        run_command("systemctl", &["suspend"]).await
    } else if which("pm-suspend") {
        run_command("pm-suspend", &[]).await
    } else {
        ToolResult::error("Sleep not available (requires systemd or pm-utils)")
    }
}

#[cfg(target_os = "linux")]
async fn handle_lock() -> ToolResult {
    let lockers: &[&[&str]] = &[
        &["loginctl", "lock-session"],
        &["xdg-screensaver", "lock"],
        &["gnome-screensaver-command", "-l"],
        &["xflock4"],
        &["i3lock"],
        &["slock"],
    ];
    for locker in lockers {
        if which(locker[0]) {
            return run_command(locker[0], &locker[1..]).await;
        }
    }
    ToolResult::error("No screen locker found (install xdg-screensaver, i3lock, etc.)")
}

#[cfg(target_os = "linux")]
async fn handle_info() -> ToolResult {
    let mut info = String::new();

    if let Ok(hostname) = std::fs::read_to_string("/etc/hostname") {
        info.push_str(&format!("Hostname: {}\n", hostname.trim()));
    }
    // OS from /etc/os-release
    if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
        for line in os_release.lines() {
            if line.starts_with("PRETTY_NAME=") {
                let name = line.trim_start_matches("PRETTY_NAME=").trim_matches('"');
                info.push_str(&format!("OS: {}\n", name));
                break;
            }
        }
    }
    // Kernel
    if let Ok(out) = tokio::process::Command::new("uname").arg("-r").output().await {
        info.push_str(&format!("Kernel: {}\n", String::from_utf8_lossy(&out.stdout).trim()));
    }
    // CPU
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            if line.starts_with("model name") {
                if let Some(name) = line.split(':').nth(1) {
                    info.push_str(&format!("CPU: {}\n", name.trim()));
                    break;
                }
            }
        }
    }
    // Memory
    if let Ok(out) = tokio::process::Command::new("free").args(["-h"]).output().await {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.starts_with("Mem:") {
                if let Some(total) = line.split_whitespace().nth(1) {
                    info.push_str(&format!("Memory: {}\n", total));
                }
                break;
            }
        }
    }
    // Uptime
    if let Ok(out) = tokio::process::Command::new("uptime").arg("-p").output().await {
        info.push_str(&format!("Uptime: {}\n", String::from_utf8_lossy(&out.stdout).trim()));
    }

    ToolResult::ok(info)
}

#[cfg(target_os = "linux")]
async fn handle_mute(mute: bool) -> ToolResult {
    let state = if mute { "1" } else { "0" };
    let state_str = if mute { "muted" } else { "unmuted" };
    if which("pactl") {
        run_command("pactl", &["set-sink-mute", "@DEFAULT_SINK@", state]).await
    } else if which("amixer") {
        let amixer_state = if mute { "mute" } else { "unmute" };
        run_command("amixer", &["set", "Master", amixer_state]).await
    } else {
        ToolResult::error(format!("Failed to {}. No audio control available.", state_str))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Windows implementations
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
async fn handle_volume(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "get" => {
            // No clean way to get volume on Windows without COM; report approximate
            ToolResult::ok("Volume get not supported — use set to change.".to_string())
        }
        "set" => {
            let value = input["value"].as_i64().unwrap_or(50).clamp(0, 100);
            let target = value / 2;
            let script = format!(
                "$obj = New-Object -ComObject WScript.Shell; \
                 for ($i=0; $i -lt 50; $i++) {{ $obj.SendKeys([char]174) }}; \
                 $target = {}; \
                 for ($i=0; $i -lt $target; $i++) {{ $obj.SendKeys([char]175) }}",
                target
            );
            run_powershell(&script).await
        }
        _ => ToolResult::error(format!("Unknown volume action '{}'. Use: get, set", action)),
    }
}

#[cfg(target_os = "windows")]
async fn handle_brightness(action: &str, input: &serde_json::Value) -> ToolResult {
    match action {
        "get" => {
            let script = "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightness).CurrentBrightness";
            run_powershell(script).await
        }
        "set" => {
            let value = input["value"].as_i64().unwrap_or(50).clamp(0, 100);
            let script = format!(
                "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods).WmiSetBrightness(1, {})",
                value
            );
            match run_powershell(&script).await {
                r if r.is_error => ToolResult::error(format!(
                    "Failed to set brightness (may not work on desktop monitors): {}", r.content
                )),
                _ => ToolResult::ok(format!("Brightness set to {}%", value)),
            }
        }
        _ => ToolResult::error(format!("Unknown brightness action '{}'. Use: get, set", action)),
    }
}

#[cfg(target_os = "windows")]
async fn handle_wifi(action: &str) -> ToolResult {
    match action {
        "status" => run_command("netsh", &["wlan", "show", "interfaces"]).await,
        "toggle" => {
            // Find wireless adapter, then toggle
            let find_script = "Get-NetAdapter -Physical | Where-Object { \
                $_.InterfaceDescription -match 'Wireless|Wi-Fi|WiFi' } | \
                Select-Object -First 1 -ExpandProperty Name";
            let output = tokio::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", find_script])
                .output()
                .await;
            match output {
                Ok(out) => {
                    let adapter = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if adapter.is_empty() {
                        return ToolResult::error("No Wi-Fi adapter found");
                    }
                    // Check current state
                    let status_script = format!(
                        "(Get-NetAdapter -Name '{}').Status", adapter
                    );
                    let status_out = tokio::process::Command::new("powershell")
                        .args(["-NoProfile", "-Command", &status_script])
                        .output()
                        .await;
                    let currently_up = status_out
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Up")
                        .unwrap_or(false);
                    let action = if currently_up {
                        format!("Disable-NetAdapter -Name '{}' -Confirm:$false", adapter)
                    } else {
                        format!("Enable-NetAdapter -Name '{}' -Confirm:$false", adapter)
                    };
                    run_powershell(&action).await
                }
                Err(e) => ToolResult::error(format!("Failed to find Wi-Fi adapter: {}", e)),
            }
        }
        _ => ToolResult::error(format!("Unknown wifi action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "windows")]
async fn handle_bluetooth(_action: &str) -> ToolResult {
    ToolResult::error("Bluetooth control is not supported on Windows")
}

#[cfg(target_os = "windows")]
async fn handle_battery() -> ToolResult {
    let script = "Get-WmiObject Win32_Battery | Select-Object -Property EstimatedChargeRemaining, BatteryStatus | Format-List";
    run_powershell(script).await
}

#[cfg(target_os = "windows")]
async fn handle_darkmode(action: &str) -> ToolResult {
    let reg_path = "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
    match action {
        "status" => {
            let script = format!(
                "Get-ItemProperty -Path '{}' -Name AppsUseLightTheme -ErrorAction SilentlyContinue | \
                 Select-Object -ExpandProperty AppsUseLightTheme",
                reg_path
            );
            let output = tokio::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .output()
                .await;
            match output {
                Ok(out) => {
                    let val = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if val == "0" {
                        ToolResult::ok("Dark mode: ON".to_string())
                    } else {
                        ToolResult::ok("Dark mode: OFF".to_string())
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to get dark mode: {}", e)),
            }
        }
        "toggle" => {
            // Check current, then set opposite
            let check = format!(
                "Get-ItemProperty -Path '{}' -Name AppsUseLightTheme -ErrorAction SilentlyContinue | \
                 Select-Object -ExpandProperty AppsUseLightTheme",
                reg_path
            );
            let output = tokio::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &check])
                .output()
                .await;
            let new_value = match output {
                Ok(out) => {
                    if String::from_utf8_lossy(&out.stdout).trim() == "0" { 1 } else { 0 }
                }
                Err(_) => 0,
            };
            let script = format!(
                "Set-ItemProperty -Path '{}' -Name AppsUseLightTheme -Value {} -Force; \
                 Set-ItemProperty -Path '{}' -Name SystemUsesLightTheme -Value {} -Force",
                reg_path, new_value, reg_path, new_value
            );
            run_powershell(&script).await;
            ToolResult::ok(format!("Dark mode {}", if new_value == 0 { "enabled" } else { "disabled" }))
        }
        _ => ToolResult::error(format!("Unknown darkmode action '{}'. Use: status, toggle", action)),
    }
}

#[cfg(target_os = "windows")]
async fn handle_sleep() -> ToolResult {
    run_command("cmd", &["/C", "rundll32.exe powrprof.dll,SetSuspendState 0,1,0"]).await
}

#[cfg(target_os = "windows")]
async fn handle_lock() -> ToolResult {
    run_command("rundll32.exe", &["user32.dll,LockWorkStation"]).await
}

#[cfg(target_os = "windows")]
async fn handle_info() -> ToolResult {
    let script = r#"
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
"#;
    run_powershell(script).await
}

#[cfg(target_os = "windows")]
async fn handle_mute(_mute: bool) -> ToolResult {
    // Windows mute is a toggle — both mute and unmute call the same key
    let script = "$obj = New-Object -ComObject WScript.Shell; $obj.SendKeys([char]173)";
    run_powershell(script).await
}

// ═══════════════════════════════════════════════════════════════════════
// macOS native bindings (brightness via DisplayServices, bluetooth via IOBluetooth)
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
mod macos_native {
    use std::ffi::CString;
    use std::os::raw::{c_float, c_int, c_void};

    type CGDirectDisplayID = u32;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> CGDirectDisplayID;
    }

    unsafe extern "C" {
        fn dlopen(path: *const i8, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
    }

    #[link(name = "IOBluetooth", kind = "framework")]
    unsafe extern "C" {
        fn IOBluetoothPreferenceGetControllerPowerState() -> c_int;
        fn IOBluetoothPreferenceSetControllerPowerState(state: c_int);
    }

    const RTLD_NOW: c_int = 2;
    const DISPLAY_SERVICES_PATH: &str =
        "/System/Library/PrivateFrameworks/DisplayServices.framework/DisplayServices";

    type GetBrightnessFn = unsafe extern "C" fn(CGDirectDisplayID, *mut c_float) -> c_int;
    type SetBrightnessFn = unsafe extern "C" fn(CGDirectDisplayID, c_float) -> c_int;

    fn load_display_services() -> Result<(*mut c_void, GetBrightnessFn, SetBrightnessFn), String> {
        unsafe {
            let path = CString::new(DISPLAY_SERVICES_PATH).map_err(|e| format!("CString: {}", e))?;
            let handle = dlopen(path.as_ptr(), RTLD_NOW);
            if handle.is_null() {
                return Err("Failed to load DisplayServices framework".to_string());
            }
            let get_sym = CString::new("DisplayServicesGetBrightness").unwrap();
            let get_ptr = dlsym(handle, get_sym.as_ptr());
            if get_ptr.is_null() {
                return Err("DisplayServicesGetBrightness not found".to_string());
            }
            let set_sym = CString::new("DisplayServicesSetBrightness").unwrap();
            let set_ptr = dlsym(handle, set_sym.as_ptr());
            if set_ptr.is_null() {
                return Err("DisplayServicesSetBrightness not found".to_string());
            }
            let get_fn: GetBrightnessFn = std::mem::transmute(get_ptr);
            let set_fn: SetBrightnessFn = std::mem::transmute(set_ptr);
            Ok((handle, get_fn, set_fn))
        }
    }

    pub fn brightness_get() -> Result<f32, String> {
        let (_handle, get_fn, _) = load_display_services()?;
        unsafe {
            let display = CGMainDisplayID();
            let mut brightness: c_float = 0.0;
            let result = get_fn(display, &mut brightness);
            if result == 0 { Ok(brightness) } else { Err(format!("error code {}", result)) }
        }
    }

    pub fn brightness_set(value: f32) -> Result<(), String> {
        let (_handle, _, set_fn) = load_display_services()?;
        unsafe {
            let display = CGMainDisplayID();
            let result = set_fn(display, value);
            if result == 0 { Ok(()) } else { Err(format!("error code {}", result)) }
        }
    }

    pub fn bluetooth_is_on() -> bool {
        unsafe { IOBluetoothPreferenceGetControllerPowerState() == 1 }
    }

    pub fn bluetooth_set(on: bool) {
        unsafe { IOBluetoothPreferenceSetControllerPowerState(if on { 1 } else { 0 }); }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Shell helpers
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
async fn run_osascript(script: &str) -> ToolResult {
    match tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() { "OK".to_string() } else { text })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            ToolResult::error(format!("AppleScript error: {}", stderr))
        }
        Err(e) => ToolResult::error(format!("Failed to run osascript: {}", e)),
    }
}

async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    match tokio::process::Command::new(cmd).args(args).output().await {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() { "OK".to_string() } else { text })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::error(format!(
                "{}{}",
                stdout,
                if stderr.is_empty() { String::new() } else { format!("\n{}", stderr) }
            ))
        }
        Err(e) => ToolResult::error(format!("Command '{}' failed: {}", cmd, e)),
    }
}

#[cfg(target_os = "windows")]
async fn run_powershell(script: &str) -> ToolResult {
    run_command("powershell", &["-NoProfile", "-Command", script]).await
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let tool = SettingsTool::new();
        assert_eq!(tool.name(), "settings");
        assert!(tool.description().contains("volume"));
        assert!(tool.description().contains("battery"));
        assert!(tool.description().contains("darkmode"));
        assert!(tool.description().contains("sleep"));
        let schema = tool.schema();
        assert!(schema["properties"]["resource"].is_object());
    }

    #[tokio::test]
    async fn test_unknown_resource() {
        let tool = SettingsTool::new();
        let ctx = ToolContext::default();
        let input = serde_json::json!({"resource": "unknown", "action": "get"});
        let result = tool.execute_dyn(&ctx, input).await;
        assert!(result.is_error);
        assert!(result.content.contains("Unknown resource"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn test_battery_status() {
        let result = handle_battery().await;
        assert!(!result.is_error);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_brightness_get() {
        let result = macos_native::brightness_get();
        assert!(result.is_ok(), "brightness_get failed: {:?}", result);
        let value = result.unwrap();
        assert!((0.0..=1.0).contains(&value), "brightness should be 0.0-1.0, got {}", value);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_bluetooth_status() {
        let _on = macos_native::bluetooth_is_on();
    }
}
