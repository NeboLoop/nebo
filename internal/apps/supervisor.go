package apps

import (
	"context"
	"fmt"
	"sync"
	"time"
)

// Supervisor monitors running app processes and auto-restarts any that crash
// or become unresponsive. It runs a single background goroutine that ticks
// every interval (default 15s).
type Supervisor struct {
	registry *AppRegistry
	runtime  *Runtime
	interval time.Duration
	cancel   context.CancelFunc
	done     chan struct{}

	mu          sync.Mutex
	lastRestart map[string]time.Time // appID → last restart time (cooldown)
}

// NewSupervisor creates an app process supervisor.
func NewSupervisor(registry *AppRegistry, runtime *Runtime) *Supervisor {
	return &Supervisor{
		registry:    registry,
		runtime:     runtime,
		interval:    15 * time.Second,
		lastRestart: make(map[string]time.Time),
	}
}

// Start begins background monitoring.
func (s *Supervisor) Start(ctx context.Context) {
	ctx, s.cancel = context.WithCancel(ctx)
	s.done = make(chan struct{})
	go s.run(ctx)
	fmt.Println("[apps:supervisor] Started (interval: 15s)")
}

// Stop halts background monitoring and waits for the goroutine to exit.
func (s *Supervisor) Stop() {
	if s.cancel != nil {
		s.cancel()
	}
	if s.done != nil {
		<-s.done
	}
}

func (s *Supervisor) run(ctx context.Context) {
	defer close(s.done)

	ticker := time.NewTicker(s.interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			s.check(ctx)
		}
	}
}

func (s *Supervisor) check(ctx context.Context) {
	appIDs := s.runtime.List()
	for _, appID := range appIDs {
		if ctx.Err() != nil {
			return
		}

		proc, ok := s.runtime.Get(appID)
		if !ok {
			continue // removed between List() and Get()
		}

		// Cooldown: skip apps that were just restarted (avoid tight restart loops)
		s.mu.Lock()
		if last, ok := s.lastRestart[appID]; ok && time.Since(last) < 10*time.Second {
			s.mu.Unlock()
			continue
		}
		s.mu.Unlock()

		// OS-level check: is the process still alive?
		if proc.cmd != nil && proc.cmd.Process != nil {
			if !isProcessAlive(proc.cmd.Process.Pid) {
				fmt.Printf("[apps:supervisor] App %s (PID %d) crashed, restarting...\n", appID, proc.cmd.Process.Pid)
				s.restart(ctx, appID, proc.Dir)
				continue
			}
		}

		// gRPC-level check: is the app responsive?
		healthCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
		err := proc.HealthCheck(healthCtx)
		cancel()
		if err != nil {
			fmt.Printf("[apps:supervisor] App %s health check failed: %v, restarting...\n", appID, err)
			s.restart(ctx, appID, proc.Dir)
		}
	}
}

func (s *Supervisor) restart(ctx context.Context, appID, appDir string) {
	s.mu.Lock()
	s.lastRestart[appID] = time.Now()
	s.mu.Unlock()

	if err := s.registry.restartApp(ctx, appDir); err != nil {
		fmt.Printf("[apps:supervisor] Failed to restart %s: %v\n", appID, err)
	} else {
		fmt.Printf("[apps:supervisor] App %s restarted successfully\n", appID)
	}
}
