//go:build windows

package tools

import (
	"context"
	"fmt"
	"image"
	"image/png"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

// CaptureAppWindow captures a specific app's window screenshot on Windows.
// Uses PowerShell with .NET PrintWindow for window-level capture.
func CaptureAppWindow(ctx context.Context, app string, windowIndex int) (image.Image, Rect, error) {
	tmpFile := filepath.Join(os.TempDir(), "nebo_capture.png")
	defer os.Remove(tmpFile)

	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @"
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public class WindowCapture {
    [DllImport("user32.dll")]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, int nFlags);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@

$proc = Get-Process -Name "%s" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) {
    $proc = Get-Process | Where-Object { $_.MainWindowTitle -like "*%s*" } | Select-Object -First 1
}
if (-not $proc -or $proc.MainWindowHandle -eq [IntPtr]::Zero) {
    Write-Error "Window not found for %s"
    exit 1
}

$hwnd = $proc.MainWindowHandle
$rect = New-Object WindowCapture+RECT
[WindowCapture]::GetWindowRect($hwnd, [ref]$rect) | Out-Null

$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

$bmp = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $graphics.GetHdc()
[WindowCapture]::PrintWindow($hwnd, $hdc, 2) | Out-Null
$graphics.ReleaseHdc($hdc)

$bmp.Save("%s")
$graphics.Dispose()
$bmp.Dispose()

Write-Output "$($rect.Left),$($rect.Top),$width,$height"
`, app, app, app, strings.ReplaceAll(tmpFile, `\`, `\\`))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return nil, Rect{}, fmt.Errorf("capture failed: %v (%s)", err, string(out))
	}

	parts := strings.Split(strings.TrimSpace(string(out)), ",")
	if len(parts) < 4 {
		return nil, Rect{}, fmt.Errorf("unexpected output: %s", string(out))
	}

	x, _ := strconv.Atoi(parts[0])
	y, _ := strconv.Atoi(parts[1])
	w, _ := strconv.Atoi(parts[2])
	h, _ := strconv.Atoi(parts[3])
	bounds := Rect{X: x, Y: y, Width: w, Height: h}

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

// ListAppWindows returns window titles for an app.
func ListAppWindows(ctx context.Context, app string) ([]string, error) {
	script := fmt.Sprintf(`
Get-Process | Where-Object { $_.ProcessName -like "*%s*" -and $_.MainWindowTitle -ne "" } | ForEach-Object { $_.MainWindowTitle }
`, app)
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
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
