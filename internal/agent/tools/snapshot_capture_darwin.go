//go:build darwin && !ios

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

// CaptureAppWindow captures a specific app's window screenshot on macOS.
// Uses screencapture -l <windowID> for high-fidelity capture.
func CaptureAppWindow(app string, windowIndex int) (image.Image, Rect, error) {
	// Get window ID and bounds via AppleScript
	windowID, bounds, err := getWindowInfo(app, windowIndex)
	if err != nil {
		return nil, Rect{}, fmt.Errorf("failed to get window info for %s: %w", app, err)
	}

	// Use screencapture -l for window-level capture (ships with macOS)
	tmpFile := filepath.Join(os.TempDir(), fmt.Sprintf("nebo_capture_%d.png", windowID))
	defer os.Remove(tmpFile)

	cmd := exec.Command("screencapture", "-l", strconv.Itoa(windowID), "-x", tmpFile)
	if out, err := cmd.CombinedOutput(); err != nil {
		return nil, Rect{}, fmt.Errorf("screencapture failed: %v (%s)", err, string(out))
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

// getWindowInfo retrieves the CGWindowID and bounds for an app window.
func getWindowInfo(app string, windowIndex int) (int, Rect, error) {
	// AppleScript to get window ID and position/size
	script := fmt.Sprintf(`
tell application "System Events"
	tell process "%s"
		set frontmost to true
		delay 0.1
		set winCount to count of windows
		if winCount is 0 then
			error "No windows found for %s"
		end if
		set winIdx to %d
		if winIdx < 1 then set winIdx to 1
		if winIdx > winCount then set winIdx to winCount
		set win to window winIdx
		set winPos to position of win
		set winSize to size of win
		set winTitle to name of win
		return (item 1 of winPos as text) & "," & (item 2 of winPos as text) & "," & (item 1 of winSize as text) & "," & (item 2 of winSize as text) & "," & winTitle
	end tell
end tell`, app, app, windowIndex)

	out, err := exec.Command("osascript", "-e", script).CombinedOutput()
	if err != nil {
		return 0, Rect{}, fmt.Errorf("AppleScript failed: %v (%s)", err, strings.TrimSpace(string(out)))
	}

	parts := strings.SplitN(strings.TrimSpace(string(out)), ",", 5)
	if len(parts) < 4 {
		return 0, Rect{}, fmt.Errorf("unexpected AppleScript output: %s", string(out))
	}

	x, _ := strconv.Atoi(strings.TrimSpace(parts[0]))
	y, _ := strconv.Atoi(strings.TrimSpace(parts[1]))
	w, _ := strconv.Atoi(strings.TrimSpace(parts[2]))
	h, _ := strconv.Atoi(strings.TrimSpace(parts[3]))
	bounds := Rect{X: x, Y: y, Width: w, Height: h}

	// Get the CGWindowID using the window list
	windowID, err := getCGWindowID(app, bounds)
	if err != nil {
		return 0, bounds, err
	}

	return windowID, bounds, nil
}

// getCGWindowID gets the CGWindowID for a specific app window using Python/CoreGraphics.
func getCGWindowID(app string, bounds Rect) (int, error) {
	script := fmt.Sprintf(`
import Quartz
import sys

windows = Quartz.CGWindowListCopyWindowInfo(
    Quartz.kCGWindowListOptionOnScreenOnly | Quartz.kCGWindowListExcludeDesktopElements,
    Quartz.kCGNullWindowID
)

target_app = "%s"
target_x, target_y = %d, %d

for w in windows:
    owner = w.get(Quartz.kCGWindowOwnerName, "")
    if target_app.lower() not in owner.lower():
        continue
    b = w.get(Quartz.kCGWindowBounds, {})
    wx = int(b.get("X", -1))
    wy = int(b.get("Y", -1))
    if abs(wx - target_x) < 5 and abs(wy - target_y) < 5:
        print(w[Quartz.kCGWindowNumber])
        sys.exit(0)

# Fallback: first window of the app
for w in windows:
    owner = w.get(Quartz.kCGWindowOwnerName, "")
    if target_app.lower() in owner.lower():
        layer = w.get(Quartz.kCGWindowLayer, -1)
        if layer == 0:
            print(w[Quartz.kCGWindowNumber])
            sys.exit(0)

print(-1)
`, app, bounds.X, bounds.Y)

	cmd := exec.Command("python3", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return 0, fmt.Errorf("failed to get CGWindowID: %v (%s)", err, string(out))
	}

	id, err := strconv.Atoi(strings.TrimSpace(string(out)))
	if err != nil || id <= 0 {
		return 0, fmt.Errorf("could not find window ID for %s", app)
	}

	return id, nil
}

// ListAppWindows returns info about all windows for an app.
func ListAppWindows(app string) ([]string, error) {
	script := fmt.Sprintf(`tell application "System Events"
	tell process "%s"
		set winList to ""
		repeat with win in windows
			set winList to winList & name of win & "\n"
		end repeat
		return winList
	end tell
end tell`, app)

	out, err := exec.Command("osascript", "-e", script).CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("failed to list windows: %v", err)
	}

	var windows []string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if line != "" {
			windows = append(windows, line)
		}
	}
	return windows, nil
}
