// Package daemon provides background services for proactive agent behavior.
package daemon

import (
	"context"
	"fmt"
	"hash/fnv"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/neboloop/nebo/internal/defaults"
)

// HeartbeatConfig configures the heartbeat daemon
type HeartbeatConfig struct {
	Interval     time.Duration // How often to run (default: 30 minutes)
	InitialDelay time.Duration // Delay before first heartbeat (default: 0 = run immediately)
	WorkspaceDir string        // Workspace directory to look for HEARTBEAT.md
	OnHeartbeat  func(ctx context.Context, prompt string) error
	OnCronFire   func(ctx context.Context, jobName, message string) error
	IsQuietHours func() bool // Returns true during quiet hours; nil = no quiet hours
}

// HeartbeatEvent represents something that happened between heartbeat ticks.
// Events are collected and included in the next heartbeat prompt.
type HeartbeatEvent struct {
	Source    string    // "cron:daily-report", "app:weather"
	Summary  string    // Short result (truncated by caller)
	Timestamp time.Time
}

const maxEvents = 20

// Heartbeat is a background daemon that enables proactive agent behavior
type Heartbeat struct {
	mu      sync.Mutex
	cfg     HeartbeatConfig
	ctx     context.Context
	stopCh  chan struct{}
	doneCh  chan struct{}
	running bool

	wakeCh         chan string // buffered, reason label for logging
	events         []HeartbeatEvent
	eventsMu       sync.Mutex
	lastPromptHash uint64
}

// NewHeartbeat creates a new heartbeat daemon
func NewHeartbeat(cfg HeartbeatConfig) *Heartbeat {
	if cfg.Interval == 0 {
		cfg.Interval = 30 * time.Minute
	}
	return &Heartbeat{
		cfg:    cfg,
		stopCh: make(chan struct{}),
		doneCh: make(chan struct{}),
		wakeCh: make(chan string, 8),
	}
}

// Start begins the heartbeat loop
func (h *Heartbeat) Start(ctx context.Context) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.running {
		return
	}
	h.ctx = ctx
	h.running = true
	go h.run(ctx)
}

// Stop gracefully stops the heartbeat daemon
func (h *Heartbeat) Stop() {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.stopLocked()
}

// stopLocked stops the daemon. Caller must hold h.mu.
func (h *Heartbeat) stopLocked() {
	if !h.running {
		return
	}
	close(h.stopCh)
	<-h.doneCh
	h.running = false
}

// SetInterval updates the heartbeat interval at runtime.
// If the daemon is running, it restarts with the new interval.
func (h *Heartbeat) SetInterval(d time.Duration) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if d == h.cfg.Interval {
		return
	}
	fmt.Printf("[heartbeat] Interval changed: %s -> %s\n", h.cfg.Interval, d)
	h.cfg.Interval = d
	if h.running {
		h.stopLocked()
		h.stopCh = make(chan struct{})
		h.doneCh = make(chan struct{})
		h.wakeCh = make(chan string, 8)
		h.running = true
		go h.run(h.ctx)
	}
}

// Wake triggers an immediate heartbeat tick. Non-blocking — if a wake is
// already pending, the call is silently dropped.
func (h *Heartbeat) Wake(reason string) {
	select {
	case h.wakeCh <- reason:
	default:
		// Channel full — tick already pending
	}
}

// Enqueue adds an event to be included in the next heartbeat prompt.
// Oldest events are dropped if the buffer exceeds maxEvents.
func (h *Heartbeat) Enqueue(event HeartbeatEvent) {
	h.eventsMu.Lock()
	defer h.eventsMu.Unlock()
	h.events = append(h.events, event)
	if len(h.events) > maxEvents {
		h.events = h.events[len(h.events)-maxEvents:]
	}
}

// drainEvents returns all queued events and clears the buffer.
func (h *Heartbeat) drainEvents() []HeartbeatEvent {
	h.eventsMu.Lock()
	defer h.eventsMu.Unlock()
	if len(h.events) == 0 {
		return nil
	}
	out := h.events
	h.events = nil
	return out
}

// run is the main heartbeat loop, aligned to clock boundaries
func (h *Heartbeat) run(ctx context.Context) {
	defer close(h.doneCh)

	// Wait for initial delay before first heartbeat (allows agent to connect)
	if h.cfg.InitialDelay > 0 {
		select {
		case <-ctx.Done():
			return
		case <-h.stopCh:
			return
		case <-time.After(h.cfg.InitialDelay):
			// Continue to first heartbeat
		}
	}

	// Clock-aligned loop: fire at :00, :05, :10, etc. for a 5m interval
	for {
		next := nextAlignedTime(time.Now(), h.cfg.Interval)
		fmt.Printf("[heartbeat] Next tick at %s (in %s)\n", next.Format("15:04:05"), time.Until(next).Round(time.Second))

		select {
		case <-ctx.Done():
			return
		case <-h.stopCh:
			return
		case reason := <-h.wakeCh:
			fmt.Printf("[heartbeat] Wake: %s\n", reason)
			h.tick(ctx, true) // woken: bypass dedup + quiet hours
		case <-time.After(time.Until(next)):
			if h.cfg.IsQuietHours != nil && h.cfg.IsQuietHours() {
				fmt.Println("[heartbeat] Quiet hours, skipping")
				continue
			}
			h.tick(ctx, false) // timer: dedup check applies
		}
	}
}

// nextAlignedTime returns the next clock-aligned time for the given interval.
// For a 5m interval at 00:03, returns 00:05. For 30m at 14:12, returns 14:30.
func nextAlignedTime(now time.Time, interval time.Duration) time.Time {
	return now.Truncate(interval).Add(interval)
}

// tick runs one heartbeat cycle. If woken is true, dedup is bypassed.
func (h *Heartbeat) tick(ctx context.Context, woken bool) {
	fmt.Printf("[heartbeat] Tick at %s\n", time.Now().Format("15:04:05"))

	tasks := h.loadHeartbeatFile()
	events := h.drainEvents()

	if tasks == "" && len(events) == 0 {
		fmt.Println("[heartbeat] Nothing to do (no tasks, no events)")
		return
	}

	prompt := FormatHeartbeatPrompt(tasks, events)

	// Dedup: skip if prompt identical to last tick (wake bypasses)
	hash := hashPrompt(prompt)
	if !woken && hash == h.lastPromptHash {
		fmt.Println("[heartbeat] Skipping (no change since last tick)")
		return
	}
	h.lastPromptHash = hash

	if h.cfg.OnHeartbeat != nil {
		if err := h.cfg.OnHeartbeat(ctx, prompt); err != nil {
			fmt.Printf("[heartbeat] Error: %v\n", err)
		} else {
			fmt.Println("[heartbeat] Dispatched to agent")
		}
	}
}

// loadHeartbeatFile reads HEARTBEAT.md from workspace or home directory
func (h *Heartbeat) loadHeartbeatFile() string {
	paths := []string{}

	// Workspace first
	if h.cfg.WorkspaceDir != "" {
		paths = append(paths, filepath.Join(h.cfg.WorkspaceDir, "HEARTBEAT.md"))
	}

	// Data directory fallback
	if dataDir, err := defaults.DataDir(); err == nil {
		paths = append(paths, filepath.Join(dataDir, "HEARTBEAT.md"))
	}

	for _, path := range paths {
		content, err := os.ReadFile(path)
		if err == nil {
			return strings.TrimSpace(string(content))
		}
	}

	return ""
}

// FormatHeartbeatPrompt creates a prompt for the agent to process heartbeat tasks.
// Events from the queue are appended as a "Recent Events" section.
func FormatHeartbeatPrompt(tasks string, events []HeartbeatEvent) string {
	var sb strings.Builder
	sb.WriteString("You are running a scheduled heartbeat check.")

	if tasks != "" {
		sb.WriteString(" Review the following proactive tasks and determine if any need attention right now.\n\n## HEARTBEAT.md Tasks\n\n")
		sb.WriteString(tasks)
	}

	if len(events) > 0 {
		sb.WriteString("\n\n## Recent Events\n\nThese events occurred since the last heartbeat:\n")
		for _, e := range events {
			sb.WriteString(fmt.Sprintf("- **%s** (%s): %s\n", e.Source, e.Timestamp.Format("15:04"), e.Summary))
		}
	}

	sb.WriteString("\n\n---\n\nFor each task or event:\n1. Check if the condition/trigger applies right now\n2. If yes, take action (use tools as needed)\n3. If the task says to notify the user, use the message tool\n\nIf no tasks need attention, respond with \"HEARTBEAT_OK\" and nothing else.\nIf you take action, briefly summarize what you did.")

	return sb.String()
}

// IsInQuietHours checks if the current time falls within quiet hours.
// start and end are "HH:MM" strings. Handles overnight ranges (e.g., 22:00-07:00).
// Returns false if either string is empty or unparseable.
func IsInQuietHours(start, end string, now time.Time) bool {
	if start == "" || end == "" {
		return false
	}

	startMin, ok1 := parseHHMM(start)
	endMin, ok2 := parseHHMM(end)
	if !ok1 || !ok2 {
		return false
	}

	nowMin := now.Hour()*60 + now.Minute()

	if startMin <= endMin {
		// Same-day range (e.g., 09:00-17:00)
		return nowMin >= startMin && nowMin < endMin
	}
	// Overnight range (e.g., 22:00-07:00)
	return nowMin >= startMin || nowMin < endMin
}

// parseHHMM parses "HH:MM" into minutes since midnight.
func parseHHMM(s string) (int, bool) {
	parts := strings.SplitN(s, ":", 2)
	if len(parts) != 2 {
		return 0, false
	}
	h, err1 := strconv.Atoi(parts[0])
	m, err2 := strconv.Atoi(parts[1])
	if err1 != nil || err2 != nil || h < 0 || h > 23 || m < 0 || m > 59 {
		return 0, false
	}
	return h*60 + m, true
}

// hashPrompt returns FNV-1a hash of a string.
func hashPrompt(s string) uint64 {
	h := fnv.New64a()
	h.Write([]byte(s))
	return h.Sum64()
}
