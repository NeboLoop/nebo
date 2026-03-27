use std::process::Command;
use tracing::warn;

/// Send a native OS notification. Falls back silently if unavailable.
///
/// Currently disabled — OS notifications are not deep-linked so clicking
/// them does nothing useful. Re-enable once tauri-plugin-notification is
/// wired up with action handling.
pub fn send(_title: &str, _body: &str) {
    // TODO: replace with tauri-plugin-notification for deep-linked notifications
}

#[cfg(target_os = "macos")]
fn send_platform(title: &str, body: &str) -> Result<(), String> {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        body, title
    );
    Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn send_platform(title: &str, body: &str) -> Result<(), String> {
    Command::new("notify-send")
        .args([title, body])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn send_platform(title: &str, body: &str) -> Result<(), String> {
    let ps = format!(
        r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$textNodes = $template.GetElementsByTagName('text')
$textNodes.Item(0).AppendChild($template.CreateTextNode('{}')) > $null
$textNodes.Item(1).AppendChild($template.CreateTextNode('{}')) > $null
$toast = [Windows.UI.Notifications.ToastNotification]::new($template)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Nebo').Show($toast)
"#,
        title, body
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn send_platform(_title: &str, _body: &str) -> Result<(), String> {
    Ok(())
}

/// Remove characters that could break shell quoting and truncate.
fn sanitize(s: &str) -> String {
    let s = s.replace('\'', "\u{2019}"); // curly quote
    let s = s.replace('\\', "");
    let s = s.replace('"', "\u{201C}"); // curly double quote
    if s.len() > 256 {
        // Find a valid char boundary at or before byte 256
        let mut end = 256;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_truncates() {
        let long = "a".repeat(300);
        let result = sanitize(&long);
        assert_eq!(result.len(), 259); // 256 + "..."
    }

    #[test]
    fn test_sanitize_removes_dangerous_chars() {
        let result = sanitize("hello'world\\test\"end");
        assert!(!result.contains('\''));
        assert!(!result.contains('\\'));
        assert!(!result.contains('"'));
    }
}
