package browser

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// BrowserKind identifies the type of Chromium-based browser.
type BrowserKind string

const (
	BrowserChrome   BrowserKind = "chrome"
	BrowserBrave    BrowserKind = "brave"
	BrowserEdge     BrowserKind = "edge"
	BrowserChromium BrowserKind = "chromium"
	BrowserCanary   BrowserKind = "canary"
	BrowserCustom   BrowserKind = "custom"
)

// BrowserExecutable represents a found browser binary.
type BrowserExecutable struct {
	Kind BrowserKind
	Path string
}

// RunningChrome represents a running Chrome instance.
type RunningChrome struct {
	PID         int
	Executable  *BrowserExecutable
	UserDataDir string
	CDPPort     int
	StartedAt   time.Time
	cmd         *exec.Cmd
}

// FindChromeExecutable finds a Chrome/Chromium browser on the system.
func FindChromeExecutable(customPath string) (*BrowserExecutable, error) {
	if customPath != "" {
		if !fileExists(customPath) {
			return nil, fmt.Errorf("browser executable not found: %s", customPath)
		}
		return &BrowserExecutable{Kind: BrowserCustom, Path: customPath}, nil
	}

	// Try to detect default browser first
	if exe := detectDefaultChromium(); exe != nil {
		return exe, nil
	}

	// Fall back to known paths
	switch runtime.GOOS {
	case "darwin":
		return findChromeMac(), nil
	case "linux":
		return findChromeLinux(), nil
	case "windows":
		return findChromeWindows(), nil
	default:
		return nil, fmt.Errorf("unsupported platform: %s", runtime.GOOS)
	}
}

// IsChromeReachable checks if Chrome CDP is responding.
func IsChromeReachable(cdpURL string, timeout time.Duration) bool {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	versionURL := strings.TrimSuffix(cdpURL, "/") + "/json/version"
	req, err := http.NewRequestWithContext(ctx, "GET", versionURL, nil)
	if err != nil {
		return false
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return false
	}
	defer resp.Body.Close()

	return resp.StatusCode == http.StatusOK
}

// GetChromeWebSocketURL gets the CDP WebSocket URL from a running Chrome.
func GetChromeWebSocketURL(cdpURL string, timeout time.Duration) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	versionURL := strings.TrimSuffix(cdpURL, "/") + "/json/version"
	req, err := http.NewRequestWithContext(ctx, "GET", versionURL, nil)
	if err != nil {
		return "", err
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	var version struct {
		WebSocketDebuggerURL string `json:"webSocketDebuggerUrl"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&version); err != nil {
		return "", err
	}

	if version.WebSocketDebuggerURL == "" {
		return "", fmt.Errorf("no webSocketDebuggerUrl in response")
	}

	return version.WebSocketDebuggerURL, nil
}

// LaunchChrome launches a Chrome instance with CDP enabled.
func LaunchChrome(config *ResolvedConfig, profile *ResolvedProfile) (*RunningChrome, error) {
	if !profile.CDPIsLoopback {
		return nil, fmt.Errorf("profile %q is remote; cannot launch local Chrome", profile.Name)
	}

	exe, err := FindChromeExecutable(config.ExecutablePath)
	if err != nil {
		return nil, err
	}
	if exe == nil {
		return nil, fmt.Errorf("no supported browser found (Chrome/Brave/Edge/Chromium)")
	}

	userDataDir := profile.UserDataDir
	if err := os.MkdirAll(userDataDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create user data dir: %w", err)
	}

	// Check if profile needs decoration
	if !IsProfileDecorated(userDataDir, profile.Name, profile.Color) {
		// Bootstrap profile if needed
		if needsBootstrap(userDataDir) {
			if err := bootstrapProfile(exe.Path, userDataDir, profile.CDPPort, config); err != nil {
				// Non-fatal, continue anyway
				fmt.Fprintf(os.Stderr, "profile bootstrap warning: %v\n", err)
			}
		}

		// Decorate profile
		if err := DecorateProfile(userDataDir, profile.Name, profile.Color); err != nil {
			fmt.Fprintf(os.Stderr, "profile decoration warning: %v\n", err)
		}
	}

	// Ensure clean exit state
	EnsureCleanExit(userDataDir)

	// Build Chrome args
	args := buildChromeArgs(userDataDir, profile.CDPPort, config)

	// Launch Chrome
	cmd := exec.Command(exe.Path, args...)
	cmd.Env = append(os.Environ(), "HOME="+os.Getenv("HOME"))

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("failed to start Chrome: %w", err)
	}

	running := &RunningChrome{
		PID:         cmd.Process.Pid,
		Executable:  exe,
		UserDataDir: userDataDir,
		CDPPort:     profile.CDPPort,
		StartedAt:   time.Now(),
		cmd:         cmd,
	}

	// Wait for CDP to be ready
	cdpURL := fmt.Sprintf("http://127.0.0.1:%d", profile.CDPPort)
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		if IsChromeReachable(cdpURL, 500*time.Millisecond) {
			return running, nil
		}
		time.Sleep(200 * time.Millisecond)
	}

	// CDP didn't come up, kill the process
	_ = cmd.Process.Kill()
	return nil, fmt.Errorf("Chrome CDP did not start on port %d within 15s", profile.CDPPort)
}

// StopChrome stops a running Chrome instance.
func StopChrome(running *RunningChrome, timeout time.Duration) error {
	if running.cmd == nil || running.cmd.Process == nil {
		return nil
	}

	// Try graceful shutdown first
	_ = running.cmd.Process.Signal(os.Interrupt)

	done := make(chan error, 1)
	go func() {
		done <- running.cmd.Wait()
	}()

	select {
	case <-done:
		return nil
	case <-time.After(timeout):
		// Force kill
		return running.cmd.Process.Kill()
	}
}

func buildChromeArgs(userDataDir string, cdpPort int, config *ResolvedConfig) []string {
	args := []string{
		fmt.Sprintf("--remote-debugging-port=%d", cdpPort),
		fmt.Sprintf("--user-data-dir=%s", userDataDir),
		"--no-first-run",
		"--no-default-browser-check",
		"--disable-sync",
		"--disable-background-networking",
		"--disable-component-update",
		"--disable-features=Translate,MediaRouter",
		"--disable-session-crashed-bubble",
		"--hide-crash-restore-bubble",
		"--password-store=basic",
	}

	if config.Headless {
		args = append(args, "--headless=new", "--disable-gpu")
	}

	if config.NoSandbox {
		args = append(args, "--no-sandbox", "--disable-setuid-sandbox")
	}

	if runtime.GOOS == "linux" {
		args = append(args, "--disable-dev-shm-usage")
	}

	// Always open a blank tab to ensure a target exists
	args = append(args, "about:blank")

	return args
}

func needsBootstrap(userDataDir string) bool {
	localState := filepath.Join(userDataDir, "Local State")
	prefs := filepath.Join(userDataDir, "Default", "Preferences")
	return !fileExists(localState) || !fileExists(prefs)
}

func bootstrapProfile(exePath, userDataDir string, cdpPort int, config *ResolvedConfig) error {
	args := buildChromeArgs(userDataDir, cdpPort, config)

	cmd := exec.Command(exePath, args...)
	cmd.Env = append(os.Environ(), "HOME="+os.Getenv("HOME"))

	if err := cmd.Start(); err != nil {
		return err
	}

	// Wait for prefs files to be created
	localState := filepath.Join(userDataDir, "Local State")
	prefs := filepath.Join(userDataDir, "Default", "Preferences")

	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if fileExists(localState) && fileExists(prefs) {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}

	// Kill the bootstrap process
	_ = cmd.Process.Signal(os.Interrupt)
	time.Sleep(500 * time.Millisecond)
	_ = cmd.Process.Kill()
	_ = cmd.Wait()

	return nil
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// macOS Chrome detection
func findChromeMac() *BrowserExecutable {
	candidates := []struct {
		kind BrowserKind
		path string
	}{
		{BrowserChrome, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"},
		{BrowserChrome, filepath.Join(os.Getenv("HOME"), "Applications/Google Chrome.app/Contents/MacOS/Google Chrome")},
		{BrowserBrave, "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"},
		{BrowserBrave, filepath.Join(os.Getenv("HOME"), "Applications/Brave Browser.app/Contents/MacOS/Brave Browser")},
		{BrowserEdge, "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"},
		{BrowserEdge, filepath.Join(os.Getenv("HOME"), "Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge")},
		{BrowserChromium, "/Applications/Chromium.app/Contents/MacOS/Chromium"},
		{BrowserChromium, filepath.Join(os.Getenv("HOME"), "Applications/Chromium.app/Contents/MacOS/Chromium")},
		{BrowserCanary, "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"},
	}

	for _, c := range candidates {
		if fileExists(c.path) {
			return &BrowserExecutable{Kind: c.kind, Path: c.path}
		}
	}
	return nil
}

// Linux Chrome detection
func findChromeLinux() *BrowserExecutable {
	candidates := []struct {
		kind BrowserKind
		path string
	}{
		{BrowserChrome, "/usr/bin/google-chrome"},
		{BrowserChrome, "/usr/bin/google-chrome-stable"},
		{BrowserChrome, "/usr/bin/chrome"},
		{BrowserBrave, "/usr/bin/brave-browser"},
		{BrowserBrave, "/usr/bin/brave-browser-stable"},
		{BrowserBrave, "/usr/bin/brave"},
		{BrowserBrave, "/snap/bin/brave"},
		{BrowserEdge, "/usr/bin/microsoft-edge"},
		{BrowserEdge, "/usr/bin/microsoft-edge-stable"},
		{BrowserChromium, "/usr/bin/chromium"},
		{BrowserChromium, "/usr/bin/chromium-browser"},
		{BrowserChromium, "/snap/bin/chromium"},
	}

	for _, c := range candidates {
		if fileExists(c.path) {
			return &BrowserExecutable{Kind: c.kind, Path: c.path}
		}
	}
	return nil
}

// Windows Chrome detection
func findChromeWindows() *BrowserExecutable {
	localAppData := os.Getenv("LOCALAPPDATA")
	programFiles := os.Getenv("ProgramFiles")
	if programFiles == "" {
		programFiles = "C:\\Program Files"
	}
	programFilesX86 := os.Getenv("ProgramFiles(x86)")
	if programFilesX86 == "" {
		programFilesX86 = "C:\\Program Files (x86)"
	}

	var candidates []struct {
		kind BrowserKind
		path string
	}

	if localAppData != "" {
		candidates = append(candidates,
			struct {
				kind BrowserKind
				path string
			}{BrowserChrome, filepath.Join(localAppData, "Google", "Chrome", "Application", "chrome.exe")},
			struct {
				kind BrowserKind
				path string
			}{BrowserBrave, filepath.Join(localAppData, "BraveSoftware", "Brave-Browser", "Application", "brave.exe")},
			struct {
				kind BrowserKind
				path string
			}{BrowserEdge, filepath.Join(localAppData, "Microsoft", "Edge", "Application", "msedge.exe")},
			struct {
				kind BrowserKind
				path string
			}{BrowserCanary, filepath.Join(localAppData, "Google", "Chrome SxS", "Application", "chrome.exe")},
		)
	}

	candidates = append(candidates,
		struct {
			kind BrowserKind
			path string
		}{BrowserChrome, filepath.Join(programFiles, "Google", "Chrome", "Application", "chrome.exe")},
		struct {
			kind BrowserKind
			path string
		}{BrowserChrome, filepath.Join(programFilesX86, "Google", "Chrome", "Application", "chrome.exe")},
		struct {
			kind BrowserKind
			path string
		}{BrowserBrave, filepath.Join(programFiles, "BraveSoftware", "Brave-Browser", "Application", "brave.exe")},
		struct {
			kind BrowserKind
			path string
		}{BrowserEdge, filepath.Join(programFiles, "Microsoft", "Edge", "Application", "msedge.exe")},
	)

	for _, c := range candidates {
		if fileExists(c.path) {
			return &BrowserExecutable{Kind: c.kind, Path: c.path}
		}
	}
	return nil
}

// detectDefaultChromium tries to detect the system's default Chromium browser.
func detectDefaultChromium() *BrowserExecutable {
	switch runtime.GOOS {
	case "darwin":
		return detectDefaultChromiumMac()
	case "linux":
		return detectDefaultChromiumLinux()
	case "windows":
		return detectDefaultChromiumWindows()
	default:
		return nil
	}
}

func detectDefaultChromiumMac() *BrowserExecutable {
	// Use osascript to get default browser bundle ID
	cmd := exec.Command("osascript", "-e", `
		use framework "AppKit"
		set ws to current application's NSWorkspace's sharedWorkspace()
		set defaultBrowser to ws's URLForApplicationToOpenURL:(current application's NSURL's URLWithString:"https://")
		if defaultBrowser is missing value then return ""
		set bundlePath to defaultBrowser's |path|() as text
		return bundlePath
	`)
	out, err := cmd.Output()
	if err != nil {
		return nil
	}

	bundlePath := strings.TrimSpace(string(out))
	if bundlePath == "" {
		return nil
	}

	// Check if it's a Chromium-based browser
	chromiumBundles := map[string]BrowserKind{
		"Google Chrome.app":        BrowserChrome,
		"Google Chrome Canary.app": BrowserCanary,
		"Brave Browser.app":        BrowserBrave,
		"Microsoft Edge.app":       BrowserEdge,
		"Chromium.app":             BrowserChromium,
		"Arc.app":                  BrowserChromium,
		"Vivaldi.app":              BrowserChromium,
		"Opera.app":                BrowserChromium,
	}

	for name, kind := range chromiumBundles {
		if strings.Contains(bundlePath, name) {
			// Find the actual executable
			exeName := strings.TrimSuffix(name, ".app")
			exePath := filepath.Join(bundlePath, "Contents", "MacOS", exeName)
			if fileExists(exePath) {
				return &BrowserExecutable{Kind: kind, Path: exePath}
			}
		}
	}

	return nil
}

func detectDefaultChromiumLinux() *BrowserExecutable {
	// Use xdg-settings to get default browser
	cmd := exec.Command("xdg-settings", "get", "default-web-browser")
	out, err := cmd.Output()
	if err != nil {
		return nil
	}

	desktopID := strings.TrimSpace(string(out))
	if desktopID == "" {
		return nil
	}

	// Map desktop IDs to browser kinds
	chromiumDesktops := map[string]BrowserKind{
		"google-chrome.desktop":        BrowserChrome,
		"google-chrome-stable.desktop": BrowserChrome,
		"brave-browser.desktop":        BrowserBrave,
		"microsoft-edge.desktop":       BrowserEdge,
		"chromium.desktop":             BrowserChromium,
		"chromium-browser.desktop":     BrowserChromium,
	}

	kind, ok := chromiumDesktops[desktopID]
	if !ok {
		return nil
	}

	// The findChromeLinux will find the actual path
	exe := findChromeLinux()
	if exe != nil {
		exe.Kind = kind
	}
	return exe
}

func detectDefaultChromiumWindows() *BrowserExecutable {
	// Read default browser from registry
	cmd := exec.Command("reg", "query",
		"HKCU\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\http\\UserChoice",
		"/v", "ProgId")
	out, err := cmd.Output()
	if err != nil {
		return nil
	}

	output := string(out)
	// Parse ProgId from output
	if strings.Contains(output, "ChromeHTML") {
		return findChromeWindows()
	}
	if strings.Contains(output, "BraveHTML") {
		exe := findChromeWindows()
		if exe != nil && exe.Kind == BrowserBrave {
			return exe
		}
	}
	if strings.Contains(output, "MSEdgeHTM") {
		exe := findChromeWindows()
		if exe != nil && exe.Kind == BrowserEdge {
			return exe
		}
	}

	return nil
}

// execCommand runs a command and returns stdout.
func execCommand(name string, args ...string) ([]byte, error) {
	cmd := exec.Command(name, args...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	err := cmd.Run()
	if err != nil {
		return nil, fmt.Errorf("%w: %s", err, stderr.String())
	}
	return stdout.Bytes(), nil
}
