// Package updater checks for new Nebo releases via a CDN-hosted version manifest.
package updater

import (
	"bufio"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

const (
	// CDN-hosted version manifest URL
	releaseURL = "https://cdn.neboloop.com/releases/version.json"
	// HTTP timeout for the update check
	timeout = 5 * time.Second
)

// Result contains the outcome of an update check.
type Result struct {
	Available      bool   `json:"available"`
	CurrentVersion string `json:"current_version"`
	LatestVersion  string `json:"latest_version"`
	ReleaseURL     string `json:"release_url,omitempty"`
	PublishedAt    string `json:"published_at,omitempty"`
}

// versionManifest is the JSON structure served from the CDN.
type versionManifest struct {
	Version     string `json:"version"`
	ReleaseURL  string `json:"release_url"`
	PublishedAt string `json:"published_at"`
}

// Check fetches the CDN-hosted version manifest and compares the latest tag
// against currentVersion. It returns a Result indicating whether an update is
// available. The function respects the provided context for cancellation and
// applies its own 5-second timeout on top.
func Check(ctx context.Context, currentVersion string) (*Result, error) {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, releaseURL, nil)
	if err != nil {
		return nil, fmt.Errorf("updater: create request: %w", err)
	}
	req.Header.Set("User-Agent", "nebo/"+currentVersion)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("updater: fetch release: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("updater: version check returned %d", resp.StatusCode)
	}

	var manifest versionManifest
	if err := json.NewDecoder(resp.Body).Decode(&manifest); err != nil {
		return nil, fmt.Errorf("updater: decode response: %w", err)
	}

	latest := normalizeVersion(manifest.Version)
	current := normalizeVersion(currentVersion)

	return &Result{
		Available:      latest != current && current != "dev" && isNewer(latest, current),
		CurrentVersion: currentVersion,
		LatestVersion:  manifest.Version,
		ReleaseURL:     manifest.ReleaseURL,
		PublishedAt:    manifest.PublishedAt,
	}, nil
}

// normalizeVersion strips the leading "v" prefix for comparison.
func normalizeVersion(v string) string {
	return strings.TrimPrefix(strings.TrimSpace(v), "v")
}

// isNewer does a simple semver comparison (major.minor.patch).
// Returns true if latest > current.
func isNewer(latest, current string) bool {
	lParts := splitVersion(latest)
	cParts := splitVersion(current)

	for i := 0; i < 3; i++ {
		if lParts[i] > cParts[i] {
			return true
		}
		if lParts[i] < cParts[i] {
			return false
		}
	}
	return false
}

// splitVersion parses "1.2.3" into [1, 2, 3]. Returns [0,0,0] on failure.
func splitVersion(v string) [3]int {
	var parts [3]int
	fmt.Sscanf(v, "%d.%d.%d", &parts[0], &parts[1], &parts[2])
	return parts
}

// NotifyFunc is called when a new version is detected. It receives the check
// result and should push it to the frontend (e.g. via websocket event).
type NotifyFunc func(result *Result)

// BackgroundChecker periodically checks for updates and notifies the frontend
// when a new version is available. It deduplicates notifications so the user
// is only notified once per new version.
type BackgroundChecker struct {
	version        string
	interval       time.Duration
	notify         NotifyFunc
	lastNotified   string // last version we notified about
	mu             sync.Mutex
}

// NewBackgroundChecker creates a checker that runs every interval and calls
// notify when a new version is detected. It only notifies once per version.
func NewBackgroundChecker(currentVersion string, interval time.Duration, notify NotifyFunc) *BackgroundChecker {
	return &BackgroundChecker{
		version:  currentVersion,
		interval: interval,
		notify:   notify,
	}
}

// Run starts the periodic check loop. It performs an initial check after a
// short delay, then rechecks every interval. Blocks until ctx is cancelled.
func (b *BackgroundChecker) Run(ctx context.Context) {
	// Initial check after 30s (let the app finish booting)
	select {
	case <-time.After(30 * time.Second):
	case <-ctx.Done():
		return
	}

	b.check(ctx)

	ticker := time.NewTicker(b.interval)
	defer ticker.Stop()

	for {
		select {
		case <-ticker.C:
			b.check(ctx)
		case <-ctx.Done():
			return
		}
	}
}

// check performs a single update check and notifies if a new version is found
// that we haven't already notified about.
func (b *BackgroundChecker) check(ctx context.Context) {
	result, err := Check(ctx, b.version)
	if err != nil || result == nil || !result.Available {
		return
	}

	b.mu.Lock()
	alreadyNotified := b.lastNotified == result.LatestVersion
	if !alreadyNotified {
		b.lastNotified = result.LatestVersion
	}
	b.mu.Unlock()

	if !alreadyNotified && b.notify != nil {
		b.notify(result)
	}
}

// releaseDownloadURL is the base URL for downloading release assets from the CDN.
const releaseDownloadURL = "https://cdn.neboloop.com/releases"

// DetectInstallMethod returns how Nebo was installed: "direct", "homebrew", or "package_manager".
func DetectInstallMethod() string {
	exe, err := os.Executable()
	if err != nil {
		return "direct"
	}
	resolved, err := filepath.EvalSymlinks(exe)
	if err != nil {
		resolved = exe
	}

	// Homebrew: binary lives under /opt/homebrew/ or /usr/local/Cellar/
	if strings.Contains(resolved, "/opt/homebrew/") || strings.Contains(resolved, "/usr/local/Cellar/") {
		return "homebrew"
	}

	// Linux package manager: dpkg knows about this binary
	if runtime.GOOS == "linux" {
		cmd := exec.Command("dpkg", "-S", resolved)
		if err := cmd.Run(); err == nil {
			return "package_manager"
		}
	}

	return "direct"
}

// AssetName returns the release asset filename for the current platform.
func AssetName() string {
	os := runtime.GOOS
	arch := runtime.GOARCH
	switch os {
	case "windows":
		return fmt.Sprintf("nebo-%s-%s.exe", os, arch)
	default:
		return fmt.Sprintf("nebo-%s-%s", os, arch)
	}
}

// ProgressFunc is called during download with bytes downloaded and total bytes.
type ProgressFunc func(downloaded, total int64)

// Download fetches the release binary for the given tag and streams it to a temp file.
// The progress callback is called periodically with download progress.
// Returns the path to the downloaded temp file.
func Download(ctx context.Context, tagName string, progress ProgressFunc) (string, error) {
	asset := AssetName()
	url := fmt.Sprintf("%s/%s/%s", releaseDownloadURL, tagName, asset)

	ctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return "", fmt.Errorf("updater: download request: %w", err)
	}
	req.Header.Set("User-Agent", "nebo-updater")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("updater: download: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("updater: download returned %d", resp.StatusCode)
	}

	// Create temp file in the same directory as the binary for same-filesystem rename
	tmpFile, err := os.CreateTemp("", "nebo-update-*")
	if err != nil {
		return "", fmt.Errorf("updater: create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()

	total := resp.ContentLength
	var downloaded int64

	// Stream with progress reporting
	buf := make([]byte, 32*1024)
	for {
		n, readErr := resp.Body.Read(buf)
		if n > 0 {
			if _, writeErr := tmpFile.Write(buf[:n]); writeErr != nil {
				tmpFile.Close()
				os.Remove(tmpPath)
				return "", fmt.Errorf("updater: write temp file: %w", writeErr)
			}
			downloaded += int64(n)
			if progress != nil {
				progress(downloaded, total)
			}
		}
		if readErr == io.EOF {
			break
		}
		if readErr != nil {
			tmpFile.Close()
			os.Remove(tmpPath)
			return "", fmt.Errorf("updater: read body: %w", readErr)
		}
	}

	if err := tmpFile.Close(); err != nil {
		os.Remove(tmpPath)
		return "", fmt.Errorf("updater: close temp file: %w", err)
	}

	// Make executable on unix
	if runtime.GOOS != "windows" {
		if err := os.Chmod(tmpPath, 0755); err != nil {
			os.Remove(tmpPath)
			return "", fmt.Errorf("updater: chmod: %w", err)
		}
	}

	return tmpPath, nil
}

// VerifyChecksum downloads checksums.txt from the release and verifies the
// downloaded binary's SHA256 against it. If checksums.txt is not found
// (older releases), the verification is skipped with a warning log.
func VerifyChecksum(ctx context.Context, binaryPath, tagName string) error {
	asset := AssetName()
	url := fmt.Sprintf("%s/%s/checksums.txt", releaseDownloadURL, tagName)

	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return fmt.Errorf("updater: checksum request: %w", err)
	}
	req.Header.Set("User-Agent", "nebo-updater")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("updater: fetch checksums: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		// Old release without checksums — skip verification
		return nil
	}
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("updater: checksums returned %d", resp.StatusCode)
	}

	// Parse checksums.txt: each line is "{sha256}  {filename}" or "{sha256} {filename}"
	var expectedHash string
	scanner := bufio.NewScanner(resp.Body)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		parts := strings.Fields(line)
		if len(parts) >= 2 && parts[1] == asset {
			expectedHash = parts[0]
			break
		}
	}
	if err := scanner.Err(); err != nil {
		return fmt.Errorf("updater: read checksums: %w", err)
	}

	if expectedHash == "" {
		return fmt.Errorf("updater: asset %s not found in checksums.txt", asset)
	}

	// Compute SHA256 of the downloaded file
	f, err := os.Open(binaryPath)
	if err != nil {
		return fmt.Errorf("updater: open binary for checksum: %w", err)
	}
	defer f.Close()

	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return fmt.Errorf("updater: hash binary: %w", err)
	}
	actualHash := hex.EncodeToString(h.Sum(nil))

	if !strings.EqualFold(actualHash, expectedHash) {
		return fmt.Errorf("updater: checksum mismatch: expected %s, got %s", expectedHash, actualHash)
	}

	return nil
}
