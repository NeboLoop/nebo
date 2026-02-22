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
//
// Restart policy: exponential backoff with a max of 5 restarts per hour per app.
// After exceeding the limit, the app is left stopped until Nebo restarts.
type Supervisor struct {
	registry *AppRegistry
	runtime  *Runtime
	interval time.Duration
	cancel   context.CancelFunc
	done     chan struct{}

	mu       sync.Mutex
	appState map[string]*appRestartState
}

// appRestartState tracks restart history for a single app.
type appRestartState struct {
	lastRestart  time.Time
	restartCount int       // restarts in the current window
	windowStart  time.Time // start of the current counting window
	backoffUntil time.Time // don't restart before this time
}

const (
	maxRestartsPerHour = 5
	restartWindow      = 1 * time.Hour
	minBackoff         = 10 * time.Second
	maxBackoff         = 5 * time.Minute
)

// NewSupervisor creates an app process supervisor.
func NewSupervisor(registry *AppRegistry, runtime *Runtime) *Supervisor {
	return &Supervisor{
		registry: registry,
		runtime:  runtime,
		interval: 15 * time.Second,
		appState: make(map[string]*appRestartState),
	}
}

// Start begins background monitoring.
func (s *Supervisor) Start(ctx context.Context) {
	ctx, s.cancel = context.WithCancel(ctx)
	s.done = make(chan struct{})
	go s.run(ctx)
	fmt.Println("[apps:supervisor] Started (interval: 15s, max restarts: 5/hr)")
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

		// Check backoff / restart limits
		s.mu.Lock()
		state := s.appState[appID]
		if state != nil {
			// Still in backoff period
			if time.Now().Before(state.backoffUntil) {
				s.mu.Unlock()
				continue
			}
			// Reset window if expired
			if time.Since(state.windowStart) > restartWindow {
				state.restartCount = 0
				state.windowStart = time.Now()
			}
			// Exhausted restart budget
			if state.restartCount >= maxRestartsPerHour {
				s.mu.Unlock()
				continue
			}
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
	state, ok := s.appState[appID]
	if !ok {
		state = &appRestartState{windowStart: time.Now()}
		s.appState[appID] = state
	}

	state.restartCount++
	state.lastRestart = time.Now()

	// Exponential backoff: 10s, 20s, 40s, 80s, 160s (capped at 5min)
	backoff := minBackoff
	for i := 1; i < state.restartCount; i++ {
		backoff *= 2
		if backoff > maxBackoff {
			backoff = maxBackoff
			break
		}
	}
	state.backoffUntil = time.Now().Add(backoff)

	count := state.restartCount
	s.mu.Unlock()

	if count > maxRestartsPerHour {
		// Deregister capabilities so the agent stops routing to dead gRPC connections
		manifest, _ := LoadManifest(appDir)
		if manifest != nil {
			s.registry.deregisterCapabilities(manifest)
			fmt.Printf("[apps:supervisor] App %s exceeded restart limit — deregistered capabilities: %v\n",
				appID, manifest.Provides)
		} else {
			fmt.Printf("[apps:supervisor] App %s exceeded restart limit (%d/%d in window), giving up until Nebo restarts\n",
				appID, count, maxRestartsPerHour)
		}
		return
	}

	fmt.Printf("[apps:supervisor] Restarting %s (attempt %d/%d, next backoff: %s)\n",
		appID, count, maxRestartsPerHour, backoff)

	// Suppress watcher for 30s so it doesn't fire a redundant restart
	// when the new binary starts writing to its socket file.
	s.runtime.SuppressWatcher(appID, 30*time.Second)

	if err := s.registry.restartApp(ctx, appDir); err != nil {
		fmt.Printf("[apps:supervisor] Failed to restart %s: %v\n", appID, err)
	} else {
		fmt.Printf("[apps:supervisor] App %s restarted successfully (PID will appear in next health check)\n", appID)
	}

	s.runtime.ClearWatcherSuppression(appID)
}
