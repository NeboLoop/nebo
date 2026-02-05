// Package daemon provides background services for proactive agent behavior.
package daemon

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/defaults"
)

// HeartbeatConfig configures the heartbeat daemon
type HeartbeatConfig struct {
	Interval      time.Duration // How often to run (default: 30 minutes)
	InitialDelay  time.Duration // Delay before first heartbeat (default: 0 = run immediately)
	WorkspaceDir  string        // Workspace directory to look for HEARTBEAT.md
	OnHeartbeat   func(ctx context.Context, tasks string) error
	OnCronFire    func(ctx context.Context, jobName, message string) error
}

// Heartbeat is a background daemon that enables proactive agent behavior
type Heartbeat struct {
	cfg      HeartbeatConfig
	stopCh   chan struct{}
	doneCh   chan struct{}
	running  bool
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
	}
}

// Start begins the heartbeat loop
func (h *Heartbeat) Start(ctx context.Context) {
	if h.running {
		return
	}
	h.running = true

	go h.run(ctx)
}

// Stop gracefully stops the heartbeat daemon
func (h *Heartbeat) Stop() {
	if !h.running {
		return
	}
	close(h.stopCh)
	<-h.doneCh
	h.running = false
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
		case <-time.After(time.Until(next)):
			h.tick(ctx)
		}
	}
}

// nextAlignedTime returns the next clock-aligned time for the given interval.
// For a 5m interval at 00:03, returns 00:05. For 30m at 14:12, returns 14:30.
func nextAlignedTime(now time.Time, interval time.Duration) time.Time {
	return now.Truncate(interval).Add(interval)
}

// tick runs one heartbeat cycle
func (h *Heartbeat) tick(ctx context.Context) {
	fmt.Printf("[heartbeat] Tick at %s\n", time.Now().Format("15:04:05"))

	// 1. Load HEARTBEAT.md
	tasks := h.loadHeartbeatFile()
	if tasks == "" {
		fmt.Println("[heartbeat] No HEARTBEAT.md found, skipping")
		return
	}

	// 2. If there are tasks and a handler, call it
	if h.cfg.OnHeartbeat != nil {
		if err := h.cfg.OnHeartbeat(ctx, tasks); err != nil {
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

// FormatHeartbeatPrompt creates a prompt for the agent to process heartbeat tasks
func FormatHeartbeatPrompt(tasks string) string {
	return fmt.Sprintf(`You are running a scheduled heartbeat check. Review the following proactive tasks and determine if any need attention right now.

## HEARTBEAT.md Tasks

%s

---

For each task:
1. Check if the condition/trigger applies right now
2. If yes, take action (use tools as needed)
3. If the task says to notify the user, use the message tool

If no tasks need attention, respond with "HEARTBEAT_OK" and nothing else.
If you take action, briefly summarize what you did.`, tasks)
}
