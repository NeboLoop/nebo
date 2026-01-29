// Package daemon provides background services for proactive agent behavior.
package daemon

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/zeromicro/go-zero/core/logx"
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

// run is the main heartbeat loop
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

	// Run first heartbeat
	h.tick(ctx)

	ticker := time.NewTicker(h.cfg.Interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-h.stopCh:
			return
		case <-ticker.C:
			h.tick(ctx)
		}
	}
}

// tick runs one heartbeat cycle
func (h *Heartbeat) tick(ctx context.Context) {
	logx.Debug("[heartbeat] Running heartbeat check...")

	// 1. Load HEARTBEAT.md
	tasks := h.loadHeartbeatFile()

	// 2. If there are tasks and a handler, call it
	if tasks != "" && h.cfg.OnHeartbeat != nil {
		if err := h.cfg.OnHeartbeat(ctx, tasks); err != nil {
			logx.Errorf("[heartbeat] Error processing heartbeat: %v", err)
		}
	}

	logx.Debug("[heartbeat] Heartbeat complete")
}

// loadHeartbeatFile reads HEARTBEAT.md from workspace or home directory
func (h *Heartbeat) loadHeartbeatFile() string {
	paths := []string{}

	// Workspace first
	if h.cfg.WorkspaceDir != "" {
		paths = append(paths, filepath.Join(h.cfg.WorkspaceDir, "HEARTBEAT.md"))
	}

	// Home directory fallback
	if home, err := os.UserHomeDir(); err == nil {
		paths = append(paths, filepath.Join(home, ".gobot", "HEARTBEAT.md"))
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
