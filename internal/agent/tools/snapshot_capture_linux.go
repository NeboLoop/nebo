//go:build linux

package tools

import (
	"fmt"
	"image"
	"image/png"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

// CaptureAppWindow captures a specific app's window screenshot on Linux.
// Uses xdotool to find the window ID, then import (ImageMagick) for capture.
func CaptureAppWindow(app string, windowIndex int) (image.Image, Rect, error) {
	// Find window ID
	windowID, err := findWindowID(app, windowIndex)
	if err != nil {
		return nil, Rect{}, err
	}

	// Get window geometry
	bounds, err := getWindowGeometry(windowID)
	if err != nil {
		return nil, Rect{}, err
	}

	// Try import (ImageMagick) first, then scrot/gnome-screenshot
	tmpFile := filepath.Join(os.TempDir(), "nebo_capture.png")
	defer os.Remove(tmpFile)

	captured := false

	// Method 1: ImageMagick import
	if _, err := exec.LookPath("import"); err == nil {
		cmd := exec.Command("import", "-window", windowID, tmpFile)
		if _, err := cmd.CombinedOutput(); err == nil {
			captured = true
		}
	}

	// Method 2: scrot with focused window
	if !captured {
		if _, err := exec.LookPath("scrot"); err == nil {
			// Focus the window first
			exec.Command("xdotool", "windowactivate", "--sync", windowID).Run()
			cmd := exec.Command("scrot", "-u", tmpFile)
			if _, err := cmd.CombinedOutput(); err == nil {
				captured = true
			}
		}
	}

	// Method 3: gnome-screenshot
	if !captured {
		if _, err := exec.LookPath("gnome-screenshot"); err == nil {
			exec.Command("xdotool", "windowactivate", "--sync", windowID).Run()
			cmd := exec.Command("gnome-screenshot", "-w", "-f", tmpFile)
			if _, err := cmd.CombinedOutput(); err == nil {
				captured = true
			}
		}
	}

	if !captured {
		return nil, Rect{}, fmt.Errorf("no screenshot tool available. Install one of: imagemagick, scrot, gnome-screenshot")
	}

	f, err := os.Open(tmpFile)
	if err != nil {
		return nil, Rect{}, fmt.Errorf("failed to open capture: %w", err)
	}
	defer f.Close()

	img, err := png.Decode(f)
	if err != nil {
		return nil, Rect{}, fmt.Errorf("failed to decode capture: %w", err)
	}

	return img, bounds, nil
}

func findWindowID(app string, windowIndex int) (string, error) {
	cmd := exec.Command("xdotool", "search", "--name", app)
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("no window found for %s", app)
	}

	ids := strings.Split(strings.TrimSpace(string(out)), "\n")
	if len(ids) == 0 || ids[0] == "" {
		return "", fmt.Errorf("no window found for %s", app)
	}

	idx := windowIndex - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(ids) {
		idx = len(ids) - 1
	}

	return ids[idx], nil
}

func getWindowGeometry(windowID string) (Rect, error) {
	cmd := exec.Command("xdotool", "getwindowgeometry", "--shell", windowID)
	out, err := cmd.Output()
	if err != nil {
		return Rect{}, fmt.Errorf("failed to get geometry: %v", err)
	}

	var x, y, w, h int
	for _, line := range strings.Split(string(out), "\n") {
		parts := strings.SplitN(line, "=", 2)
		if len(parts) != 2 {
			continue
		}
		val, _ := strconv.Atoi(strings.TrimSpace(parts[1]))
		switch strings.TrimSpace(parts[0]) {
		case "X":
			x = val
		case "Y":
			y = val
		case "WIDTH":
			w = val
		case "HEIGHT":
			h = val
		}
	}

	return Rect{X: x, Y: y, Width: w, Height: h}, nil
}

// ListAppWindows returns window titles for an app.
func ListAppWindows(app string) ([]string, error) {
	cmd := exec.Command("xdotool", "search", "--name", app)
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("no windows found for %s", app)
	}

	var windows []string
	for _, id := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if id == "" {
			continue
		}
		nameCmd := exec.Command("xdotool", "getwindowname", id)
		nameOut, _ := nameCmd.Output()
		name := strings.TrimSpace(string(nameOut))
		if name != "" {
			windows = append(windows, name)
		}
	}
	return windows, nil
}
