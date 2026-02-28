//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// NotificationTool provides macOS notification capabilities.
// Uses osascript for notifications and the say command for speech.
type NotificationTool struct{}

// NewNotificationTool creates a new notification tool
func NewNotificationTool() *NotificationTool {
	return &NotificationTool{}
}

func (t *NotificationTool) Name() string {
	return "notification"
}

func (t *NotificationTool) Description() string {
	return "Display notifications: send notifications with title/body/sound, show alert dialogs, speak text aloud, check Do Not Disturb status."
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
				"description": "Notification subtitle (optional)"
			},
			"sound": {
				"type": "string",
				"description": "Sound name: default, Basso, Blow, Bottle, Frog, Funk, Glass, Hero, Morse, Ping, Pop, Purr, Sosumi, Submarine, Tink"
			},
			"voice": {
				"type": "string",
				"description": "Voice for speak action (e.g., 'Alex', 'Samantha'). Use 'list' to see available voices."
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
	Text     string `json:"text"`
	Subtitle string `json:"subtitle"`
	Sound    string `json:"sound"`
	Voice    string `json:"voice"`
}

func (in *notificationInput) normalize() {
	if in.Message == "" && in.Text != "" {
		in.Message = in.Text
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

	title := escapeAppleScriptString(in.Title)
	message := escapeAppleScriptString(in.Message)
	subtitle := escapeAppleScriptString(in.Subtitle)

	if title == "" {
		title = "Nebo"
	}

	var script strings.Builder
	script.WriteString(fmt.Sprintf(`display notification "%s" with title "%s"`, message, title))

	if subtitle != "" {
		script.WriteString(fmt.Sprintf(` subtitle "%s"`, subtitle))
	}

	if in.Sound != "" {
		script.WriteString(fmt.Sprintf(` sound name "%s"`, in.Sound))
	}

	_, err := exec.Command("osascript", "-e", script.String()).Output()
	if err != nil {
		return "", fmt.Errorf("failed to send notification: %v", err)
	}

	return fmt.Sprintf("Notification sent: %s - %s", title, in.Message), nil
}

func (t *NotificationTool) showAlert(in notificationInput) (string, error) {
	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	title := escapeAppleScriptString(in.Title)
	message := escapeAppleScriptString(in.Message)

	if title == "" {
		title = "Nebo Alert"
	}

	script := fmt.Sprintf(`display alert "%s" message "%s" as informational`, title, message)

	_, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to show alert: %v", err)
	}

	return fmt.Sprintf("Alert shown: %s", title), nil
}

func (t *NotificationTool) speak(in notificationInput) (string, error) {
	// Handle list voices request
	if in.Voice == "list" {
		out, err := exec.Command("say", "-v", "?").Output()
		if err != nil {
			return "", fmt.Errorf("failed to list voices: %v", err)
		}

		lines := strings.Split(string(out), "\n")
		var voices []string
		for _, line := range lines {
			if line != "" {
				parts := strings.Fields(line)
				if len(parts) > 0 {
					voices = append(voices, parts[0])
				}
			}
		}

		if len(voices) > 20 {
			voices = voices[:20]
		}

		return fmt.Sprintf("Available voices (first 20):\n%s", strings.Join(voices, ", ")), nil
	}

	if in.Message == "" {
		return "", fmt.Errorf("message is required")
	}

	args := []string{}
	if in.Voice != "" {
		args = append(args, "-v", in.Voice)
	}
	args = append(args, in.Message)

	cmd := exec.Command("say", args...)
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("failed to speak: %v", err)
	}

	voice := in.Voice
	if voice == "" {
		voice = "default"
	}

	return fmt.Sprintf("Spoke with voice '%s': %s", voice, in.Message), nil
}

func (t *NotificationTool) getDNDStatus() (string, error) {
	// Try newer Focus mode approach (macOS 12+)
	out, _ := exec.Command("bash", "-c",
		"defaults -currentHost read ~/Library/Preferences/ByHost/com.apple.notificationcenterui doNotDisturb 2>/dev/null || echo 0").Output()

	result := strings.TrimSpace(string(out))
	if result == "1" {
		return "Do Not Disturb: ON", nil
	}
	return "Do Not Disturb: OFF", nil
}

func escapeAppleScriptString(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `"`, `\"`)
	return s
}

