package apps

import (
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// ---------------------------------------------------------------------------
// Fix A: Per-app launch mutex
// ---------------------------------------------------------------------------

func TestAppLaunchMutex_SameAppReturnsSameMutex(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	mu1 := rt.appLaunchMutex("com.test.voice")
	mu2 := rt.appLaunchMutex("com.test.voice")

	if mu1 != mu2 {
		t.Error("same app ID should return the same mutex pointer")
	}
}

func TestAppLaunchMutex_DifferentAppsGetDifferentMutexes(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	mu1 := rt.appLaunchMutex("com.test.voice")
	mu2 := rt.appLaunchMutex("com.test.whatsapp")

	if mu1 == mu2 {
		t.Error("different app IDs should return different mutex pointers")
	}
}

func TestAppLaunchMutex_SerializesConcurrentLaunches(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	const appID = "com.test.voice"
	mu := rt.appLaunchMutex(appID)

	var concurrentCount int32
	var maxConcurrent int32
	var wg sync.WaitGroup

	for i := 0; i < 10; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			mu.Lock()
			defer mu.Unlock()

			cur := atomic.AddInt32(&concurrentCount, 1)
			// Track max concurrency
			for {
				old := atomic.LoadInt32(&maxConcurrent)
				if cur <= old || atomic.CompareAndSwapInt32(&maxConcurrent, old, cur) {
					break
				}
			}
			// Simulate work
			time.Sleep(5 * time.Millisecond)
			atomic.AddInt32(&concurrentCount, -1)
		}()
	}

	wg.Wait()

	if max := atomic.LoadInt32(&maxConcurrent); max != 1 {
		t.Errorf("max concurrent executions = %d, want 1 (mutex should serialize)", max)
	}
}

func TestAppLaunchMutex_DifferentAppsRunInParallel(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	var concurrentCount int32
	var maxConcurrent int32
	var wg sync.WaitGroup

	apps := []string{"com.test.voice", "com.test.whatsapp", "com.test.telegram"}

	for _, appID := range apps {
		wg.Add(1)
		go func(id string) {
			defer wg.Done()
			mu := rt.appLaunchMutex(id)
			mu.Lock()
			defer mu.Unlock()

			cur := atomic.AddInt32(&concurrentCount, 1)
			for {
				old := atomic.LoadInt32(&maxConcurrent)
				if cur <= old || atomic.CompareAndSwapInt32(&maxConcurrent, old, cur) {
					break
				}
			}
			time.Sleep(50 * time.Millisecond)
			atomic.AddInt32(&concurrentCount, -1)
		}(appID)
	}

	wg.Wait()

	if max := atomic.LoadInt32(&maxConcurrent); max < 2 {
		t.Errorf("max concurrent executions = %d, want >= 2 (different apps should run in parallel)", max)
	}
}

// ---------------------------------------------------------------------------
// Fix C: Watcher suppression
// ---------------------------------------------------------------------------

func TestSuppressWatcher_BasicFlow(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	appID := "com.test.voice"

	// Not suppressed initially
	if rt.IsWatcherSuppressed(appID) {
		t.Error("should not be suppressed before calling SuppressWatcher")
	}

	// Suppress for 1 second
	rt.SuppressWatcher(appID, 1*time.Second)

	if !rt.IsWatcherSuppressed(appID) {
		t.Error("should be suppressed after calling SuppressWatcher")
	}

	// Clear it
	rt.ClearWatcherSuppression(appID)

	if rt.IsWatcherSuppressed(appID) {
		t.Error("should not be suppressed after clearing")
	}
}

func TestSuppressWatcher_AutoExpires(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	appID := "com.test.voice"

	// Suppress for 50ms
	rt.SuppressWatcher(appID, 50*time.Millisecond)

	if !rt.IsWatcherSuppressed(appID) {
		t.Error("should be suppressed immediately")
	}

	// Wait for expiry
	time.Sleep(100 * time.Millisecond)

	if rt.IsWatcherSuppressed(appID) {
		t.Error("suppression should auto-expire after duration")
	}
}

func TestSuppressWatcher_ExpiryCleanup(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	appID := "com.test.voice"

	// Suppress for 10ms
	rt.SuppressWatcher(appID, 10*time.Millisecond)
	time.Sleep(50 * time.Millisecond)

	// This call should auto-clean the entry
	rt.IsWatcherSuppressed(appID)

	// Verify the entry was actually deleted from sync.Map
	_, loaded := rt.restarting.Load(appID)
	if loaded {
		t.Error("expired entry should be deleted from sync.Map on check")
	}
}

func TestSuppressWatcher_IndependentApps(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	app1 := "com.test.voice"
	app2 := "com.test.whatsapp"

	rt.SuppressWatcher(app1, 1*time.Second)

	if !rt.IsWatcherSuppressed(app1) {
		t.Error("app1 should be suppressed")
	}
	if rt.IsWatcherSuppressed(app2) {
		t.Error("app2 should NOT be suppressed — suppression is per-app")
	}
}

func TestSuppressWatcher_ConcurrentAccess(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	var wg sync.WaitGroup
	appID := "com.test.voice"

	// Concurrent suppress/check/clear from many goroutines
	for i := 0; i < 50; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			switch i % 3 {
			case 0:
				rt.SuppressWatcher(appID, 100*time.Millisecond)
			case 1:
				rt.IsWatcherSuppressed(appID)
			case 2:
				rt.ClearWatcherSuppression(appID)
			}
		}(i)
	}

	wg.Wait()
	// No panics = pass. sync.Map must handle this safely.
}

func TestSuppressWatcher_OverwriteExtendsDuration(t *testing.T) {
	rt := NewRuntime(t.TempDir(), DefaultSandboxConfig())

	appID := "com.test.voice"

	// Suppress for 30ms
	rt.SuppressWatcher(appID, 30*time.Millisecond)

	// Wait 20ms, then re-suppress for another 200ms
	time.Sleep(20 * time.Millisecond)
	rt.SuppressWatcher(appID, 200*time.Millisecond)

	// At 40ms from start, original would have expired, but new one is active
	time.Sleep(20 * time.Millisecond)

	if !rt.IsWatcherSuppressed(appID) {
		t.Error("re-suppression should extend the window")
	}
}
