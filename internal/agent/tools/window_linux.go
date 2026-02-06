//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// WindowTool provides Linux window management via wmctrl/xdotool.
type WindowTool struct{}

func NewWindowTool() *WindowTool { return &WindowTool{} }

func (t *WindowTool) Name() string { return "window" }

func (t *WindowTool) Description() string {
	return "Manage windows: list, focus, move, resize, minimize, maximize, or close windows by app name."
}

func (t *WindowTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["list", "focus", "move", "resize", "minimize", "maximize", "close"],
				"description": "Action to perform"
			},
			"app": {"type": "string", "description": "Application name"},
			"title": {"type": "string", "description": "Window title (partial match)"},
			"x": {"type": "integer", "description": "X position for move"},
			"y": {"type": "integer", "description": "Y position for move"},
			"width": {"type": "integer", "description": "Width for resize"},
			"height": {"type": "integer", "description": "Height for resize"}
		},
		"required": ["action"]
	}`)
}

func (t *WindowTool) RequiresApproval() bool { return true }

type windowInput struct {
	Action string `json:"action"`
	App    string `json:"app"`
	Title  string `json:"title"`
	X      int    `json:"x"`
	Y      int    `json:"y"`
	Width  int    `json:"width"`
	Height int    `json:"height"`
}

func (t *WindowTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p windowInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "list":
		return t.listWindows()
	case "focus":
		return t.focusWindow(p.App, p.Title)
	case "move":
		return t.moveWindow(p.App, p.Title, p.X, p.Y)
	case "resize":
		return t.resizeWindow(p.App, p.Title, p.Width, p.Height)
	case "minimize":
		return t.minimizeWindow(p.App, p.Title)
	case "maximize":
		return t.maximizeWindow(p.App, p.Title)
	case "close":
		return t.closeWindow(p.App, p.Title)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *WindowTool) listWindows() (*ToolResult, error) {
	// Try wmctrl first (provides more info)
	if _, err := exec.LookPath("wmctrl"); err == nil {
		out, err := exec.Command("wmctrl", "-l", "-G", "-p").Output()
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
		}
		lines := strings.Split(strings.TrimSpace(string(out)), "\n")
		var sb strings.Builder
		sb.WriteString(fmt.Sprintf("Found %d windows:\n\n", len(lines)))
		for _, line := range lines {
			if line == "" {
				continue
			}
			// wmctrl -l -G -p format: window_id desktop pid x y width height hostname title
			fields := strings.Fields(line)
			if len(fields) >= 8 {
				x, _ := strconv.Atoi(fields[3])
				y, _ := strconv.Atoi(fields[4])
				w, _ := strconv.Atoi(fields[5])
				h, _ := strconv.Atoi(fields[6])
				title := strings.Join(fields[8:], " ")
				if len(title) > 50 {
					title = title[:47] + "..."
				}
				sb.WriteString(fmt.Sprintf("• %s (ID: %s)\n  Position: (%d, %d), Size: %dx%d\n\n", title, fields[0], x, y, w, h))
			}
		}
		return &ToolResult{Content: sb.String()}, nil
	}

	// Try xdotool
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", "").Output()
		if err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
		}
		windowIDs := strings.Split(strings.TrimSpace(string(out)), "\n")
		var sb strings.Builder
		count := 0
		for _, id := range windowIDs {
			if id == "" {
				continue
			}
			// Get window name
			nameOut, _ := exec.Command("xdotool", "getwindowname", id).Output()
			name := strings.TrimSpace(string(nameOut))
			if name == "" {
				continue
			}
			count++
			// Get geometry
			geoOut, _ := exec.Command("xdotool", "getwindowgeometry", "--shell", id).Output()
			x, y, w, h := parseXdotoolGeometry(string(geoOut))
			if len(name) > 50 {
				name = name[:47] + "..."
			}
			sb.WriteString(fmt.Sprintf("• %s (ID: %s)\n  Position: (%d, %d), Size: %dx%d\n\n", name, id, x, y, w, h))
		}
		return &ToolResult{Content: fmt.Sprintf("Found %d windows:\n\n%s", count, sb.String())}, nil
	}

	return &ToolResult{Content: "Window listing unavailable (install wmctrl or xdotool)", IsError: true}, nil
}

func parseXdotoolGeometry(output string) (x, y, w, h int) {
	for _, line := range strings.Split(output, "\n") {
		if val, ok := strings.CutPrefix(line, "X="); ok {
			x, _ = strconv.Atoi(val)
		} else if val, ok := strings.CutPrefix(line, "Y="); ok {
			y, _ = strconv.Atoi(val)
		} else if val, ok := strings.CutPrefix(line, "WIDTH="); ok {
			w, _ = strconv.Atoi(val)
		} else if val, ok := strings.CutPrefix(line, "HEIGHT="); ok {
			h, _ = strconv.Atoi(val)
		}
	}
	return
}

func (t *WindowTool) focusWindow(app, title string) (*ToolResult, error) {
	searchTerm := title
	if searchTerm == "" {
		searchTerm = app
	}
	if searchTerm == "" {
		return &ToolResult{Content: "App or title is required", IsError: true}, nil
	}

	// Try wmctrl first
	if _, err := exec.LookPath("wmctrl"); err == nil {
		cmd := exec.Command("wmctrl", "-a", searchTerm)
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Focused '%s'", searchTerm)}, nil
	}

	// Try xdotool
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", searchTerm).Output()
		if err != nil || strings.TrimSpace(string(out)) == "" {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]
		if err := exec.Command("xdotool", "windowactivate", windowID).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to focus: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Focused '%s'", searchTerm)}, nil
	}

	return &ToolResult{Content: "Window management unavailable (install wmctrl or xdotool)", IsError: true}, nil
}

func (t *WindowTool) moveWindow(app, title string, x, y int) (*ToolResult, error) {
	searchTerm := title
	if searchTerm == "" {
		searchTerm = app
	}
	if searchTerm == "" {
		return &ToolResult{Content: "App or title is required", IsError: true}, nil
	}

	// Try wmctrl first
	if _, err := exec.LookPath("wmctrl"); err == nil {
		// -e gravity,x,y,width,height (-1 means don't change)
		cmd := exec.Command("wmctrl", "-r", searchTerm, "-e", fmt.Sprintf("0,%d,%d,-1,-1", x, y))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Moved window to (%d, %d)", x, y)}, nil
	}

	// Try xdotool
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", searchTerm).Output()
		if err != nil || strings.TrimSpace(string(out)) == "" {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]
		if err := exec.Command("xdotool", "windowmove", windowID, strconv.Itoa(x), strconv.Itoa(y)).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to move: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Moved window to (%d, %d)", x, y)}, nil
	}

	return &ToolResult{Content: "Window management unavailable (install wmctrl or xdotool)", IsError: true}, nil
}

func (t *WindowTool) resizeWindow(app, title string, width, height int) (*ToolResult, error) {
	searchTerm := title
	if searchTerm == "" {
		searchTerm = app
	}
	if searchTerm == "" {
		return &ToolResult{Content: "App or title is required", IsError: true}, nil
	}
	if width <= 0 || height <= 0 {
		return &ToolResult{Content: "Width and height must be positive", IsError: true}, nil
	}

	// Try wmctrl first
	if _, err := exec.LookPath("wmctrl"); err == nil {
		// -e gravity,x,y,width,height (-1 means don't change)
		cmd := exec.Command("wmctrl", "-r", searchTerm, "-e", fmt.Sprintf("0,-1,-1,%d,%d", width, height))
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Resized window to %dx%d", width, height)}, nil
	}

	// Try xdotool
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", searchTerm).Output()
		if err != nil || strings.TrimSpace(string(out)) == "" {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]
		if err := exec.Command("xdotool", "windowsize", windowID, strconv.Itoa(width), strconv.Itoa(height)).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to resize: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Resized window to %dx%d", width, height)}, nil
	}

	return &ToolResult{Content: "Window management unavailable (install wmctrl or xdotool)", IsError: true}, nil
}

func (t *WindowTool) minimizeWindow(app, title string) (*ToolResult, error) {
	searchTerm := title
	if searchTerm == "" {
		searchTerm = app
	}
	if searchTerm == "" {
		return &ToolResult{Content: "App or title is required", IsError: true}, nil
	}

	// Try xdotool (wmctrl doesn't have direct minimize)
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", searchTerm).Output()
		if err != nil || strings.TrimSpace(string(out)) == "" {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]
		if err := exec.Command("xdotool", "windowminimize", windowID).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to minimize: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: "Minimized window"}, nil
	}

	// Try wmctrl with shade
	if _, err := exec.LookPath("wmctrl"); err == nil {
		// wmctrl can add shaded state
		cmd := exec.Command("wmctrl", "-r", searchTerm, "-b", "add,hidden")
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found or minimize not supported", searchTerm), IsError: true}, nil
		}
		return &ToolResult{Content: "Minimized window"}, nil
	}

	return &ToolResult{Content: "Window management unavailable (install xdotool)", IsError: true}, nil
}

func (t *WindowTool) maximizeWindow(app, title string) (*ToolResult, error) {
	searchTerm := title
	if searchTerm == "" {
		searchTerm = app
	}
	if searchTerm == "" {
		return &ToolResult{Content: "App or title is required", IsError: true}, nil
	}

	// Try wmctrl first
	if _, err := exec.LookPath("wmctrl"); err == nil {
		cmd := exec.Command("wmctrl", "-r", searchTerm, "-b", "add,maximized_vert,maximized_horz")
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		return &ToolResult{Content: "Maximized window"}, nil
	}

	// Try xdotool with getdisplaygeometry
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", searchTerm).Output()
		if err != nil || strings.TrimSpace(string(out)) == "" {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]

		// Get screen size
		sizeOut, _ := exec.Command("xdotool", "getdisplaygeometry").Output()
		parts := strings.Fields(strings.TrimSpace(string(sizeOut)))
		if len(parts) >= 2 {
			width, _ := strconv.Atoi(parts[0])
			height, _ := strconv.Atoi(parts[1])
			exec.Command("xdotool", "windowmove", windowID, "0", "0").Run()
			exec.Command("xdotool", "windowsize", windowID, strconv.Itoa(width), strconv.Itoa(height)).Run()
		}
		return &ToolResult{Content: "Maximized window"}, nil
	}

	return &ToolResult{Content: "Window management unavailable (install wmctrl or xdotool)", IsError: true}, nil
}

func (t *WindowTool) closeWindow(app, title string) (*ToolResult, error) {
	searchTerm := title
	if searchTerm == "" {
		searchTerm = app
	}
	if searchTerm == "" {
		return &ToolResult{Content: "App or title is required", IsError: true}, nil
	}

	// Try wmctrl first
	if _, err := exec.LookPath("wmctrl"); err == nil {
		cmd := exec.Command("wmctrl", "-c", searchTerm)
		if err := cmd.Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		return &ToolResult{Content: "Closed window"}, nil
	}

	// Try xdotool
	if _, err := exec.LookPath("xdotool"); err == nil {
		out, err := exec.Command("xdotool", "search", "--name", searchTerm).Output()
		if err != nil || strings.TrimSpace(string(out)) == "" {
			return &ToolResult{Content: fmt.Sprintf("Window '%s' not found", searchTerm), IsError: true}, nil
		}
		windowID := strings.Split(strings.TrimSpace(string(out)), "\n")[0]
		if err := exec.Command("xdotool", "windowclose", windowID).Run(); err != nil {
			return &ToolResult{Content: fmt.Sprintf("Failed to close: %v", err), IsError: true}, nil
		}
		return &ToolResult{Content: "Closed window"}, nil
	}

	return &ToolResult{Content: "Window management unavailable (install wmctrl or xdotool)", IsError: true}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewWindowTool(),
		Platforms: []string{PlatformLinux},
		Category:  "system",
	})
}
