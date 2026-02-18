package runner

import (
	"bufio"
	"fmt"
	"os"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/agent/session"
)

// File re-injection limits
const (
	MaxReinjectedFiles  = 5      // Max number of files to re-inject after compaction
	MaxTokensPerFile    = 5000   // Max tokens per re-injected file (~20,000 chars)
	MaxReinjectedTokens = 50000  // Total token budget for all re-injected files (~200,000 chars)
)

// FileAccessTracker records file paths accessed during a session.
// Thread-safe — called from tool execution goroutines.
type FileAccessTracker struct {
	mu      sync.Mutex
	entries map[string]time.Time
}

// NewFileAccessTracker creates a new tracker.
func NewFileAccessTracker() *FileAccessTracker {
	return &FileAccessTracker{entries: make(map[string]time.Time)}
}

// Track records a file access.
func (t *FileAccessTracker) Track(path string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.entries[path] = time.Now()
}

// Snapshot returns a copy of all tracked entries.
func (t *FileAccessTracker) Snapshot() map[string]time.Time {
	t.mu.Lock()
	defer t.mu.Unlock()
	snap := make(map[string]time.Time, len(t.entries))
	for k, v := range t.entries {
		snap[k] = v
	}
	return snap
}

// Clear removes all tracked entries.
func (t *FileAccessTracker) Clear() {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.entries = make(map[string]time.Time)
}

// buildFileReinjectionMessage reads the most recently accessed files and builds
// a synthetic user message containing their contents. This recovers working
// context that was lost during compaction.
//
// Returns nil if no files can be read or the tracker is empty.
func buildFileReinjectionMessage(tracker *FileAccessTracker) *session.Message {
	if tracker == nil {
		return nil
	}

	snap := tracker.Snapshot()
	if len(snap) == 0 {
		return nil
	}

	// Sort by most recent access
	type entry struct {
		path string
		time time.Time
	}
	entries := make([]entry, 0, len(snap))
	for path, t := range snap {
		entries = append(entries, entry{path: path, time: t})
	}
	sort.Slice(entries, func(i, j int) bool {
		return entries[i].time.After(entries[j].time)
	})

	// Take top N
	if len(entries) > MaxReinjectedFiles {
		entries = entries[:MaxReinjectedFiles]
	}

	var content strings.Builder
	content.WriteString("[Context recovery — recently accessed files]\n")

	totalChars := 0
	maxCharsPerFile := MaxTokensPerFile * CharsPerTokenEstimate
	maxTotalChars := MaxReinjectedTokens * CharsPerTokenEstimate
	filesIncluded := 0

	for _, e := range entries {
		if totalChars >= maxTotalChars {
			break
		}

		fileContent := readFileForReinjection(e.path, maxCharsPerFile)
		if fileContent == "" {
			continue
		}

		// Check total budget
		remaining := maxTotalChars - totalChars
		if len(fileContent) > remaining {
			fileContent = fileContent[:remaining] + "\n... (truncated)"
		}

		content.WriteString(fmt.Sprintf("\n=== %s ===\n", e.path))
		content.WriteString(fileContent)
		content.WriteString("\n")

		totalChars += len(fileContent)
		filesIncluded++
	}

	if filesIncluded == 0 {
		return nil
	}

	fmt.Printf("[Runner] Re-injecting %d recently accessed files (~%d tokens)\n",
		filesIncluded, totalChars/CharsPerTokenEstimate)

	return &session.Message{
		Role:    "user",
		Content: content.String(),
	}
}

// readFileForReinjection reads a file's contents capped at maxChars.
// Returns empty string if the file can't be read.
func readFileForReinjection(path string, maxChars int) string {
	file, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer file.Close()

	var result strings.Builder
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 256*1024), 256*1024) // 256KB line buffer

	lineNum := 0
	for scanner.Scan() {
		lineNum++
		line := scanner.Text()

		// Truncate very long lines
		const maxLineLen = 500
		if len(line) > maxLineLen {
			line = line[:maxLineLen] + "..."
		}

		formatted := fmt.Sprintf("%6d\t%s\n", lineNum, line)
		if result.Len()+len(formatted) > maxChars {
			result.WriteString("... (truncated)\n")
			break
		}
		result.WriteString(formatted)
	}

	return result.String()
}
