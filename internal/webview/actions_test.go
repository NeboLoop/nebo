package webview

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"
)

func TestNavigateCreatesWindowAndSetsURL(t *testing.T) {
	m := &Manager{
		windows:     make(map[string]*Window),
		callbackURL: "http://localhost:27895/internal/webview/callback",
	}

	var capturedHandle *mockHandle
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		h := newMockHandle(opts.Name)
		capturedHandle = h
		return h
	})

	// Create a window first
	win, err := m.CreateWindow("about:blank", "Test")
	if err != nil {
		t.Fatalf("CreateWindow failed: %v", err)
	}

	// Simulate async JS callback delivery — poll until a pending request exists
	go func() {
		for i := 0; i < 200; i++ {
			time.Sleep(20 * time.Millisecond)
			collector.mu.Lock()
			for reqID, ch := range collector.pending {
				ch <- CallbackResult{
					RequestID: reqID,
					Data:      json.RawMessage(`{"url":"https://example.com","title":"Example"}`),
				}
				collector.mu.Unlock()
				return
			}
			collector.mu.Unlock()
		}
	}()

	ctx := context.Background()
	result, err := Navigate(ctx, m, win.ID, "https://example.com", 5*time.Second)
	if err != nil {
		t.Fatalf("Navigate failed: %v", err)
	}

	// Verify SetURL was called
	if capturedHandle.url != "https://example.com" {
		t.Errorf("expected URL https://example.com, got %s", capturedHandle.url)
	}

	// Verify result contains page info
	if !strings.Contains(string(result), "Example") {
		t.Errorf("expected result to contain 'Example', got %s", string(result))
	}
}

func TestSnapshotExecJSAndCallback(t *testing.T) {
	m := &Manager{
		windows:     make(map[string]*Window),
		callbackURL: "http://localhost:27895/internal/webview/callback",
	}

	var handle *mockHandle
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		h := newMockHandle(opts.Name)
		handle = h
		return h
	})

	win, _ := m.CreateWindow("https://example.com", "Test")

	// Deliver result when ExecJS is called with the snapshot (not fingerprint)
	go func() {
		// Wait for ExecJS to be called with snapshot code (after fingerprint injection)
		for i := 0; i < 200; i++ {
			time.Sleep(10 * time.Millisecond)
			collector.mu.Lock()
			for reqID, ch := range collector.pending {
				ch <- CallbackResult{
					RequestID: reqID,
					Data:      json.RawMessage(`"Page: Test\nURL: https://example.com\n---\nh1: Hello World"`),
				}
				collector.mu.Unlock()
				return
			}
			collector.mu.Unlock()
		}
	}()

	ctx := context.Background()
	result, err := Snapshot(ctx, m, win.ID, 2*time.Second)
	if err != nil {
		t.Fatalf("Snapshot failed: %v", err)
	}

	// Verify ExecJS was called with snapshot code (may be after fingerprint injection)
	handle.mu.Lock()
	if len(handle.jsLog) < 2 {
		// jsLog[0] = fingerprint, jsLog[1] = snapshot
		t.Fatalf("ExecJS should have been called at least twice (fingerprint + snapshot), got %d calls", len(handle.jsLog))
	}
	snapshotJS := handle.jsLog[len(handle.jsLog)-1] // last call is the snapshot
	handle.mu.Unlock()

	if !strings.Contains(snapshotJS, "__walk") {
		t.Error("Last ExecJS call should contain snapshot walking code")
	}

	if !strings.Contains(string(result), "Hello World") {
		t.Errorf("expected snapshot to contain 'Hello World', got %s", string(result))
	}
}

func TestActionsErrorOnNoWindows(t *testing.T) {
	m := &Manager{
		windows:     make(map[string]*Window),
		callbackURL: "http://localhost:27895/internal/webview/callback",
	}

	ctx := context.Background()

	_, err := Snapshot(ctx, m, "", time.Second)
	if err == nil {
		t.Error("expected error when no windows open")
	}

	_, err = Click(ctx, m, "", "e1", "", time.Second)
	if err == nil {
		t.Error("expected error when no windows open")
	}

	_, err = GetText(ctx, m, "", "", time.Second)
	if err == nil {
		t.Error("expected error when no windows open")
	}
}

func TestReloadCallsHandle(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window)}
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	win, _ := m.CreateWindow("https://example.com", "Test")

	err := Reload(context.Background(), m, win.ID)
	if err != nil {
		t.Fatalf("Reload failed: %v", err)
	}
}
