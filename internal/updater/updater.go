// Package updater checks for new Nebo releases via the GitHub API.
package updater

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"
)

const (
	// GitHub API endpoint for the latest release
	releaseURL = "https://api.github.com/repos/neboloop/nebo/releases/latest"
	// HTTP timeout for the update check
	timeout = 5 * time.Second
)

// Result contains the outcome of an update check.
type Result struct {
	Available      bool   `json:"available"`
	CurrentVersion string `json:"current_version"`
	LatestVersion  string `json:"latest_version"`
	ReleaseURL     string `json:"release_url,omitempty"`
	ReleaseNotes   string `json:"release_notes,omitempty"`
	PublishedAt    string `json:"published_at,omitempty"`
}

// githubRelease is the subset of fields we read from the GitHub API.
type githubRelease struct {
	TagName     string `json:"tag_name"`
	HTMLURL     string `json:"html_url"`
	Body        string `json:"body"`
	PublishedAt string `json:"published_at"`
}

// Check queries the GitHub releases API and compares the latest tag against
// currentVersion. It returns a Result indicating whether an update is available.
// The function respects the provided context for cancellation and applies its
// own 5-second timeout on top.
func Check(ctx context.Context, currentVersion string) (*Result, error) {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, releaseURL, nil)
	if err != nil {
		return nil, fmt.Errorf("updater: create request: %w", err)
	}
	req.Header.Set("Accept", "application/vnd.github.v3+json")
	req.Header.Set("User-Agent", "nebo/"+currentVersion)

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("updater: fetch release: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("updater: GitHub API returned %d", resp.StatusCode)
	}

	var release githubRelease
	if err := json.NewDecoder(resp.Body).Decode(&release); err != nil {
		return nil, fmt.Errorf("updater: decode response: %w", err)
	}

	latest := normalizeVersion(release.TagName)
	current := normalizeVersion(currentVersion)

	return &Result{
		Available:      latest != current && current != "dev" && isNewer(latest, current),
		CurrentVersion: currentVersion,
		LatestVersion:  release.TagName,
		ReleaseURL:     release.HTMLURL,
		ReleaseNotes:   truncate(release.Body, 500),
		PublishedAt:    release.PublishedAt,
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

// truncate limits a string to maxLen characters, appending "..." if truncated.
func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
