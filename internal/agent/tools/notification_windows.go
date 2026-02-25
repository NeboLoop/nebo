//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// NotificationTool provides Windows notification capabilities.
// Uses PowerShell for toast notifications and SAPI for speech.
type NotificationTool struct{}

// NewNotificationTool creates a new notification tool
func NewNotificationTool() *NotificationTool {
	return &NotificationTool{}
}

func (t *NotificationTool) Name() string {
	return "notification"
}

func (t *NotificationTool) Description() string {
	return "Display notifications: send toast notifications with title/body, show message boxes, speak text aloud."
}

func (t *NotificationTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: send (notification), alert (dialog), speak (text-to-speech)",
				"enum": ["send", "alert", "speak"]
			},
			"title": {
				"type": "string",
				"description": "Notification/alert title"
			},
			"message": {
				"type": "string",
				"description": "Notification body or text to speak"
			},
			"voice": {
				"type": "string",
				"description": "Voice for speak action. Use 'list' to see available voices."
			}
		},
		"required": ["action"]
	}`)
}

func (t *NotificationTool) RequiresApproval() bool {
	return false
}

type notificationInput struct {
	Action  string `json:"action"`
	Title   string `json:"title"`
	Message string `json:"message"`
	Voice   string `json:"voice"`
}

func (t *NotificationTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in notificationInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "send":
		result, err = t.sendNotification(ctx, in)
	case "alert":
		result, err = t.showAlert(ctx, in)
	case "speak":
		result, err = t.speak(ctx, in)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func (t *NotificationTool) sendNotification(ctx context.Context, in notificationInput) (string, error) {
	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	title := in.Title
	if title == "" {
		title = "Nebo"
	}

	// PowerShell script for Windows toast notification
	script := fmt.Sprintf(`
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null

$template = @"
<toast>
    <visual>
        <binding template="ToastText02">
            <text id="1">%s</text>
            <text id="2">%s</text>
        </binding>
    </visual>
</toast>
"@

$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier("Nebo").Show($toast)
`, escapePS(title), escapePS(in.Message))

	_, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		// Fallback to BurntToast if available
		fallback := fmt.Sprintf(`New-BurntToastNotification -Text '%s', '%s'`,
			escapePS(title), escapePS(in.Message))
		if _, err2 := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", fallback).Output(); err2 != nil {
			return "", fmt.Errorf("failed to send notification: %v", err)
		}
	}

	return fmt.Sprintf("Notification sent: %s - %s", title, in.Message), nil
}

func (t *NotificationTool) showAlert(ctx context.Context, in notificationInput) (string, error) {
	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	title := in.Title
	if title == "" {
		title = "Nebo Alert"
	}

	// Use Windows Forms MessageBox
	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.MessageBox]::Show('%s', '%s', 'OK', 'Information')
`, escapePS(in.Message), escapePS(title))

	_, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to show alert: %v", err)
	}

	return fmt.Sprintf("Alert shown: %s", title), nil
}

func (t *NotificationTool) speak(ctx context.Context, in notificationInput) (string, error) {
	// Handle list voices request
	if in.Voice == "list" {
		script := `
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$synth.GetInstalledVoices() | ForEach-Object { $_.VoiceInfo.Name }
`
		out, err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Output()
		if err != nil {
			return "", fmt.Errorf("failed to list voices: %v", err)
		}
		return fmt.Sprintf("Available voices:\n%s", strings.TrimSpace(string(out))), nil
	}

	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	var script string
	if in.Voice != "" {
		script = fmt.Sprintf(`
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$synth.SelectVoice('%s')
$synth.Speak('%s')
`, escapePS(in.Voice), escapePS(in.Message))
	} else {
		script = fmt.Sprintf(`
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
$synth.Speak('%s')
`, escapePS(in.Message))
	}

	if err := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script).Run(); err != nil {
		return "", fmt.Errorf("failed to speak: %v", err)
	}

	voice := in.Voice
	if voice == "" {
		voice = "default"
	}
	return fmt.Sprintf("Spoke with voice '%s': %s", voice, in.Message), nil
}

func escapePS(s string) string {
	s = strings.ReplaceAll(s, "'", "''")
	return s
}
