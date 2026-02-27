//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// NotificationTool provides Linux notification capabilities.
// Uses notify-send for notifications and spd-say/espeak for speech.
type NotificationTool struct{}

// NewNotificationTool creates a new notification tool
func NewNotificationTool() *NotificationTool {
	return &NotificationTool{}
}

func (t *NotificationTool) Name() string {
	return "notification"
}

func (t *NotificationTool) Description() string {
	return "Display notifications: send notifications with title/body/urgency, show alert dialogs, speak text aloud, check Do Not Disturb status."
}

func (t *NotificationTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: send (notification), alert (dialog), speak (text-to-speech), dnd_status (check Do Not Disturb)",
				"enum": ["send", "alert", "speak", "dnd_status"]
			},
			"title": {
				"type": "string",
				"description": "Notification/alert title"
			},
			"message": {
				"type": "string",
				"description": "Notification body or text to speak"
			},
			"subtitle": {
				"type": "string",
				"description": "Notification subtitle (optional, appended to message)"
			},
			"urgency": {
				"type": "string",
				"description": "Notification urgency: low, normal, critical",
				"enum": ["low", "normal", "critical"]
			},
			"icon": {
				"type": "string",
				"description": "Icon name or path for the notification (e.g., 'dialog-information', 'dialog-warning')"
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
	Action   string `json:"action"`
	Title    string `json:"title"`
	Message  string `json:"message"`
	Subtitle string `json:"subtitle"`
	Urgency  string `json:"urgency"`
	Icon     string `json:"icon"`
	Voice    string `json:"voice"`
	Text     string `json:"text"` // alias for message (system domain schema uses "text")

func (in *notificationInput) normalize() {
	if in.Message == "" && in.Text != "" {
		in.Message = in.Text
	}
}
}

func (t *NotificationTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in notificationInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}
	in.normalize()

	var result string
	var err error

	switch in.Action {
	case "send":
		result, err = t.sendNotification(in)
	case "alert":
		result, err = t.showAlert(in)
	case "speak":
		result, err = t.speak(in)
	case "dnd_status":
		result, err = t.getDNDStatus()
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result, IsError: false}, nil
}

func (t *NotificationTool) sendNotification(in notificationInput) (string, error) {
	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	title := in.Title
	if title == "" {
		title = "Nebo"
	}

	// Build message with optional subtitle
	message := in.Message
	if in.Subtitle != "" {
		message = in.Subtitle + "\n" + message
	}

	// Build notify-send command
	args := []string{title, message}

	// Add urgency if specified
	if in.Urgency != "" {
		args = append([]string{"-u", in.Urgency}, args...)
	}

	// Add icon if specified
	if in.Icon != "" {
		args = append([]string{"-i", in.Icon}, args...)
	}

	_, err := exec.Command("notify-send", args...).Output()
	if err != nil {
		return "", fmt.Errorf("failed to send notification (is libnotify-bin installed?): %v", err)
	}

	return fmt.Sprintf("Notification sent: %s - %s", title, in.Message), nil
}

func (t *NotificationTool) showAlert(in notificationInput) (string, error) {
	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	title := in.Title
	if title == "" {
		title = "Nebo Alert"
	}

	// Try zenity first (GTK), then kdialog (KDE), then notify-send as fallback
	if _, err := exec.LookPath("zenity"); err == nil {
		args := []string{"--info", "--title=" + title, "--text=" + in.Message}
		_, err := exec.Command("zenity", args...).Output()
		if err != nil {
			return "", fmt.Errorf("failed to show zenity alert: %v", err)
		}
		return fmt.Sprintf("Alert shown: %s", title), nil
	}

	if _, err := exec.LookPath("kdialog"); err == nil {
		args := []string{"--msgbox", in.Message, "--title", title}
		_, err := exec.Command("kdialog", args...).Output()
		if err != nil {
			return "", fmt.Errorf("failed to show kdialog alert: %v", err)
		}
		return fmt.Sprintf("Alert shown: %s", title), nil
	}

	// Fall back to notify-send with critical urgency
	args := []string{"-u", "critical", title, in.Message}
	_, err := exec.Command("notify-send", args...).Output()
	if err != nil {
		return "", fmt.Errorf("failed to show alert (no dialog tool available): %v", err)
	}

	return fmt.Sprintf("Alert shown (via notification): %s", title), nil
}

func (t *NotificationTool) speak(in notificationInput) (string, error) {
	// Handle list voices request
	if in.Voice == "list" {
		// Try spd-say first
		if _, err := exec.LookPath("spd-say"); err == nil {
			out, err := exec.Command("spd-say", "-L").Output()
			if err != nil {
				return "", fmt.Errorf("failed to list voices: %v", err)
			}
			return fmt.Sprintf("Available voices (spd-say):\n%s", strings.TrimSpace(string(out))), nil
		}

		// Try espeak
		if _, err := exec.LookPath("espeak"); err == nil {
			out, err := exec.Command("espeak", "--voices").Output()
			if err != nil {
				return "", fmt.Errorf("failed to list voices: %v", err)
			}
			lines := strings.Split(string(out), "\n")
			if len(lines) > 21 {
				lines = lines[:21]
			}
			return fmt.Sprintf("Available voices (espeak):\n%s", strings.Join(lines, "\n")), nil
		}

		return "No speech synthesis available. Install speech-dispatcher (spd-say) or espeak.", nil
	}

	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	// Try spd-say first (speech-dispatcher)
	if _, err := exec.LookPath("spd-say"); err == nil {
		args := []string{}
		if in.Voice != "" {
			args = append(args, "-t", in.Voice)
		}
		args = append(args, in.Message)

		cmd := exec.Command("spd-say", args...)
		if err := cmd.Run(); err != nil {
			return "", fmt.Errorf("failed to speak with spd-say: %v", err)
		}

		voice := in.Voice
		if voice == "" {
			voice = "default"
		}
		return fmt.Sprintf("Spoke with voice '%s': %s", voice, in.Message), nil
	}

	// Try espeak as fallback
	if _, err := exec.LookPath("espeak"); err == nil {
		args := []string{}
		if in.Voice != "" {
			args = append(args, "-v", in.Voice)
		}
		args = append(args, in.Message)

		cmd := exec.Command("espeak", args...)
		if err := cmd.Run(); err != nil {
			return "", fmt.Errorf("failed to speak with espeak: %v", err)
		}

		voice := in.Voice
		if voice == "" {
			voice = "default"
		}
		return fmt.Sprintf("Spoke with voice '%s': %s", voice, in.Message), nil
	}

	return "", fmt.Errorf("no speech synthesis available. Install speech-dispatcher (spd-say) or espeak")
}

func (t *NotificationTool) getDNDStatus() (string, error) {
	// GNOME: Check if notifications are paused
	out, err := exec.Command("gsettings", "get", "org.gnome.desktop.notifications", "show-banners").Output()
	if err == nil {
		result := strings.TrimSpace(string(out))
		if result == "false" {
			return "Do Not Disturb: ON (notifications paused)", nil
		}
		return "Do Not Disturb: OFF", nil
	}

	// KDE: Check DND status via dbus
	out, err = exec.Command("qdbus", "org.freedesktop.Notifications", "/org/freedesktop/Notifications",
		"org.freedesktop.Notifications.Inhibited").Output()
	if err == nil {
		result := strings.TrimSpace(string(out))
		if result == "true" {
			return "Do Not Disturb: ON", nil
		}
		return "Do Not Disturb: OFF", nil
	}

	return "Do Not Disturb: Unknown (could not detect desktop environment)", nil
}

