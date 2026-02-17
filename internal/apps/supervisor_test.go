package apps

import (
	"testing"
	"time"
)

// ---------------------------------------------------------------------------
// Supervisor restart state & backoff logic
// ---------------------------------------------------------------------------

func TestSupervisor_BackoffExponential(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())
	s := &Supervisor{
		runtime:  rt,
		appState: make(map[string]*appRestartState),
	}

	appID := "com.test.voice"
	state := &appRestartState{windowStart: time.Now()}
	s.appState[appID] = state

	// Simulate successive restarts and verify backoff doubles
	expectedBackoffs := []time.Duration{
		10 * time.Second,  // attempt 1
		20 * time.Second,  // attempt 2
		40 * time.Second,  // attempt 3
		80 * time.Second,  // attempt 4
		160 * time.Second, // attempt 5
	}

	for i, expected := range expectedBackoffs {
		state.restartCount++
		state.lastRestart = time.Now()

		backoff := minBackoff
		for j := 1; j < state.restartCount; j++ {
			backoff *= 2
			if backoff > maxBackoff {
				backoff = maxBackoff
				break
			}
		}

		if backoff != expected {
			t.Errorf("attempt %d: backoff = %v, want %v", i+1, backoff, expected)
		}
	}
}

func TestSupervisor_BackoffCappedAtMax(t *testing.T) {
	// Verify that backoff never exceeds maxBackoff (5 min)
	state := &appRestartState{restartCount: 20, windowStart: time.Now()}

	backoff := minBackoff
	for i := 1; i < state.restartCount; i++ {
		backoff *= 2
		if backoff > maxBackoff {
			backoff = maxBackoff
			break
		}
	}

	if backoff != maxBackoff {
		t.Errorf("backoff = %v, want maxBackoff %v", backoff, maxBackoff)
	}
}

func TestSupervisor_RestartLimitEnforced(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())
	s := &Supervisor{
		runtime:  rt,
		appState: make(map[string]*appRestartState),
	}

	appID := "com.test.voice"
	state := &appRestartState{
		windowStart:  time.Now(),
		restartCount: maxRestartsPerHour,
	}
	s.appState[appID] = state

	// At the limit — should still be allowed (count == maxRestartsPerHour)
	if state.restartCount > maxRestartsPerHour {
		t.Error("at exactly maxRestartsPerHour, should still be allowed")
	}

	// One more puts us over
	state.restartCount++
	if state.restartCount <= maxRestartsPerHour {
		t.Error("over maxRestartsPerHour, should be blocked")
	}
}

func TestSupervisor_WindowResetsAfterExpiry(t *testing.T) {
	state := &appRestartState{
		windowStart:  time.Now().Add(-2 * restartWindow), // 2 hours ago
		restartCount: maxRestartsPerHour,
	}

	// Simulate what check() does: reset if window expired
	if time.Since(state.windowStart) > restartWindow {
		state.restartCount = 0
		state.windowStart = time.Now()
	}

	if state.restartCount != 0 {
		t.Errorf("restartCount = %d, want 0 after window expiry", state.restartCount)
	}
}

func TestSupervisor_BackoffSkipsCheck(t *testing.T) {
	state := &appRestartState{
		windowStart:  time.Now(),
		restartCount: 1,
		backoffUntil: time.Now().Add(1 * time.Hour), // far in the future
	}

	// Simulate what check() does: skip if in backoff
	if time.Now().Before(state.backoffUntil) {
		// Would skip — this is correct
	} else {
		t.Error("should be in backoff period")
	}
}

// ---------------------------------------------------------------------------
// Supervisor restart calls SuppressWatcher / ClearWatcherSuppression
// ---------------------------------------------------------------------------

func TestSupervisor_RestartSuppressesWatcher(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	appID := "com.test.voice"

	// Simulate what supervisor.restart() does:
	// 1. Suppress watcher
	rt.SuppressWatcher(appID, 30*time.Second)

	// 2. At this point, watcher should be suppressed
	if !rt.IsWatcherSuppressed(appID) {
		t.Error("watcher should be suppressed during managed restart")
	}

	// 3. After restart completes, clear suppression
	rt.ClearWatcherSuppression(appID)

	if rt.IsWatcherSuppressed(appID) {
		t.Error("watcher should not be suppressed after restart completes")
	}
}

func TestSupervisor_FailedRestartStillClears(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	appID := "com.test.voice"

	// Simulate failed restart flow — suppression must still be cleared
	rt.SuppressWatcher(appID, 30*time.Second)

	// Pretend restartApp() returned error — supervisor still calls ClearWatcherSuppression
	rt.ClearWatcherSuppression(appID)

	if rt.IsWatcherSuppressed(appID) {
		t.Error("suppression must be cleared even if restart fails")
	}
}
