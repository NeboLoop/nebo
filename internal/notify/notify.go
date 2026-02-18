package notify

import (
	"fmt"
	"os/exec"
	"runtime"
	"strings"
)

// Send displays a native OS notification.
// Falls back silently if the notification system is unavailable.
func Send(title, body string) {
	// Sanitize inputs to prevent command injection
	title = sanitize(title)
	body = sanitize(body)

	var cmd *exec.Cmd

	switch runtime.GOOS {
	case "darwin":
		script := fmt.Sprintf(`display notification %q with title %q`, body, title)
		cmd = exec.Command("osascript", "-e", script)

	case "linux":
		cmd = exec.Command("notify-send", title, body)

	case "windows":
		// PowerShell toast notification
		ps := fmt.Sprintf(`
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$textNodes = $template.GetElementsByTagName('text')
$textNodes.Item(0).AppendChild($template.CreateTextNode('%s')) > $null
$textNodes.Item(1).AppendChild($template.CreateTextNode('%s')) > $null
$toast = [Windows.UI.Notifications.ToastNotification]::new($template)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Nebo').Show($toast)
`, title, body)
		cmd = exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", ps)

	default:
		return
	}

	if err := cmd.Run(); err != nil {
		fmt.Printf("[Notify] Failed to send notification: %v\n", err)
	}
}

// sanitize removes characters that could break shell quoting.
func sanitize(s string) string {
	s = strings.ReplaceAll(s, "'", "'")
	s = strings.ReplaceAll(s, "\\", "")
	// Truncate to reasonable length for notifications
	if len(s) > 256 {
		s = s[:256] + "..."
	}
	return s
}
