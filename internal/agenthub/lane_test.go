package agenthub

import (
	"context"
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestEnqueueSync(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()

	var ran bool
	err := mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
		ran = true
		return nil
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !ran {
		t.Fatal("task did not run")
	}
}

func TestEnqueueSyncError(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()

	want := fmt.Errorf("task failed")
	err := mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
		return want
	})
	if err != want {
		t.Fatalf("got error %v, want %v", err, want)
	}
}

func TestEnqueueAsync(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()

	done := make(chan struct{})
	mgr.EnqueueAsync(context.Background(), "test", func(ctx context.Context) error {
		close(done)
		return nil
	})

	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("async task did not run within timeout")
	}
}

func TestConcurrencyLimit(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	var running atomic.Int32
	var maxSeen atomic.Int32
	var wg sync.WaitGroup

	for i := 0; i < 5; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
				cur := running.Add(1)
				for {
					old := maxSeen.Load()
					if cur <= old || maxSeen.CompareAndSwap(old, cur) {
						break
					}
				}
				time.Sleep(10 * time.Millisecond)
				running.Add(-1)
				return nil
			})
		}()
	}

	wg.Wait()
	if maxSeen.Load() > 1 {
		t.Fatalf("max concurrent was %d, want <=1", maxSeen.Load())
	}
}

func TestMultipleProducers(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 3)

	var completed atomic.Int32
	var wg sync.WaitGroup
	n := 10

	for i := 0; i < n; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
				time.Sleep(5 * time.Millisecond)
				completed.Add(1)
				return nil
			})
		}()
	}

	wg.Wait()
	if completed.Load() != int32(n) {
		t.Fatalf("completed %d tasks, want %d", completed.Load(), n)
	}
}

func TestNoLostWakeup(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	// Block the lane with first task
	gate := make(chan struct{})
	var order []int
	var mu sync.Mutex

	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			<-gate // block until released
			mu.Lock()
			order = append(order, 1)
			mu.Unlock()
			return nil
		})
	}()

	// Wait for first task to be active
	time.Sleep(50 * time.Millisecond)

	// Enqueue second task while at capacity — this is the race scenario
	done := make(chan struct{})
	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			mu.Lock()
			order = append(order, 2)
			mu.Unlock()
			return nil
		})
		close(done)
	}()

	// Give it a moment to enqueue
	time.Sleep(20 * time.Millisecond)

	// Release the first task
	close(gate)

	// Second task should complete without hanging
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("second task hung — lost wakeup!")
	}

	mu.Lock()
	if len(order) != 2 || order[0] != 1 || order[1] != 2 {
		t.Fatalf("unexpected order: %v", order)
	}
	mu.Unlock()
}

func TestCancelActive(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	started := make(chan struct{})
	errCh := make(chan error, 1)

	go func() {
		errCh <- mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			close(started)
			<-ctx.Done()
			return ctx.Err()
		})
	}()

	<-started
	cancelled := mgr.CancelActive("test")
	if cancelled != 1 {
		t.Fatalf("cancelled %d, want 1", cancelled)
	}

	select {
	case err := <-errCh:
		if err == nil {
			t.Fatal("expected error from cancelled task")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("cancelled task did not complete")
	}
}

func TestClearLane(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	// Block the lane
	gate := make(chan struct{})
	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			<-gate
			return nil
		})
	}()
	time.Sleep(50 * time.Millisecond)

	// Enqueue some tasks that will be queued
	for i := 0; i < 3; i++ {
		mgr.EnqueueAsync(context.Background(), "test", func(ctx context.Context) error {
			return nil
		})
	}
	time.Sleep(20 * time.Millisecond)

	removed := mgr.ClearLane("test")
	if removed != 3 {
		t.Fatalf("removed %d, want 3", removed)
	}

	// Active task should still be running
	stats := mgr.GetLaneStats()
	if s, ok := stats["test"]; ok {
		if s.Active != 1 {
			t.Fatalf("active %d, want 1", s.Active)
		}
	}

	close(gate)
}

func TestSetConcurrencyMidStream(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	// Block the lane with first task
	gate := make(chan struct{})
	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			<-gate
			return nil
		})
	}()
	time.Sleep(50 * time.Millisecond)

	// Enqueue second task — will be queued since max=1 and one is active
	secondDone := make(chan struct{})
	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			close(secondDone)
			return nil
		})
	}()
	time.Sleep(20 * time.Millisecond)

	// Increase concurrency — second task should start immediately
	mgr.SetConcurrency("test", 2)

	select {
	case <-secondDone:
	case <-time.After(5 * time.Second):
		t.Fatal("second task did not start after increasing concurrency")
	}

	close(gate)
}

func TestContextCancellation(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	// Block the lane
	gate := make(chan struct{})
	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			<-gate
			return nil
		})
	}()
	time.Sleep(50 * time.Millisecond)

	// Enqueue with a cancellable context
	ctx, cancel := context.WithCancel(context.Background())
	errCh := make(chan error, 1)
	go func() {
		errCh <- mgr.Enqueue(ctx, "test", func(ctx context.Context) error {
			return nil
		})
	}()

	time.Sleep(20 * time.Millisecond)
	cancel()

	select {
	case err := <-errCh:
		if err != context.Canceled {
			t.Fatalf("got %v, want context.Canceled", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Enqueue did not return after context cancellation")
	}

	close(gate)
}

func TestWatchdog(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()

	// We can't easily test the 15-minute watchdog, but we can verify
	// a task respects context cancellation (which the watchdog uses)
	started := make(chan struct{})
	err := mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
		close(started)
		// Simulate a long task that checks context
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(100 * time.Millisecond):
			return nil
		}
	})
	<-started
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestPanicRecovery(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()

	// Task that panics
	err := mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
		panic("test panic")
	})
	if err == nil {
		t.Fatal("expected error from panicking task")
	}
	if err.Error() != "panic in lane task: test panic" {
		t.Fatalf("unexpected error: %v", err)
	}

	// Lane should still work after panic
	var ran bool
	err = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
		ran = true
		return nil
	})
	if err != nil {
		t.Fatalf("unexpected error after panic: %v", err)
	}
	if !ran {
		t.Fatal("task did not run after panic recovery")
	}
}

func TestShutdown(t *testing.T) {
	mgr := NewLaneManager()

	// Trigger lane creation
	var ran bool
	_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
		ran = true
		return nil
	})
	if !ran {
		t.Fatal("task did not run")
	}

	mgr.Shutdown()

	// Double shutdown should not panic
	mgr.Shutdown()
}

func TestGetQueueSize(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()
	mgr.SetConcurrency("test", 1)

	gate := make(chan struct{})
	go func() {
		_ = mgr.Enqueue(context.Background(), "test", func(ctx context.Context) error {
			<-gate
			return nil
		})
	}()
	time.Sleep(50 * time.Millisecond)

	mgr.EnqueueAsync(context.Background(), "test", func(ctx context.Context) error {
		return nil
	})
	time.Sleep(20 * time.Millisecond)

	size := mgr.GetQueueSize("test")
	if size != 2 { // 1 active + 1 queued
		t.Fatalf("queue size %d, want 2", size)
	}

	close(gate)
}

func TestDefaultLaneEmpty(t *testing.T) {
	mgr := NewLaneManager()
	defer mgr.Shutdown()

	// Empty lane name should default to LaneMain
	err := mgr.Enqueue(context.Background(), "", func(ctx context.Context) error {
		return nil
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}
