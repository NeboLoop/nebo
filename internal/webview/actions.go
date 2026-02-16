package webview

import (
	"context"
	"encoding/json"
	"fmt"
	"time"
)

const defaultTimeout = 15 * time.Second

// updateWindowTitle extracts the page title from a GetInfo response
// and updates the Window's tracked title (fixes BUG-006).
func updateWindowTitle(win *Window, info json.RawMessage) {
	var parsed struct {
		Title string `json:"title"`
	}
	if json.Unmarshal(info, &parsed) == nil && parsed.Title != "" {
		win.Title = parsed.Title
		win.Handle.SetTitle(parsed.Title)
	}
}

// newRequestID generates a unique request ID for JS → Go callbacks.
func newRequestID() string {
	return fmt.Sprintf("req-%d", time.Now().UnixNano())
}

// Navigate opens a URL in a window and returns page info.
func Navigate(ctx context.Context, m *Manager, windowID, url string, timeout time.Duration) (json.RawMessage, error) {
	win, err := m.GetWindow(windowID)
	if err != nil {
		return nil, err
	}

	win.Handle.SetURL(url)
	win.URL = url

	// Wait for page to load and Wails runtime to re-inject,
	// then re-inject fingerprint (JS context resets on navigation).
	time.Sleep(1500 * time.Millisecond)
	if win.Fingerprint != nil {
		win.Handle.ExecJS(win.Fingerprint.InjectJS())
	}

	info, err := GetInfo(ctx, m, windowID, timeout)
	if err != nil {
		return nil, err
	}

	// Update tracked title from page response
	updateWindowTitle(win, info)
	return info, nil
}

// GetInfo returns page metadata (url, title, scroll position).
func GetInfo(ctx context.Context, m *Manager, windowID string, timeout time.Duration) (json.RawMessage, error) {
	win, err := m.GetWindow(windowID)
	if err != nil {
		return nil, err
	}

	reqID := newRequestID()
	cbURL := m.CallbackURL()
	js := pageInfoJS(reqID, cbURL)

	ch := collector.Register(reqID)
	win.Handle.ExecJS(js)

	if timeout <= 0 {
		timeout = defaultTimeout
	}

	select {
	case result := <-ch:
		collector.Cleanup(reqID)
		if result.Error != "" {
			return nil, fmt.Errorf("js error: %s", result.Error)
		}
		return result.Data, nil
	case <-time.After(timeout):
		collector.Cleanup(reqID)
		return nil, fmt.Errorf("timeout getting page info")
	case <-ctx.Done():
		collector.Cleanup(reqID)
		return nil, ctx.Err()
	}
}

// Snapshot returns a simplified accessible DOM snapshot.
func Snapshot(ctx context.Context, m *Manager, windowID string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return snapshotJS(reqID, cbURL)
	}, timeout)
}

// Click clicks an element by ref or selector with realistic cursor movement.
func Click(ctx context.Context, m *Manager, windowID, ref, selector string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return cursorClickJS(reqID, cbURL, ref, selector)
	}, timeout)
}

// Fill sets the value of an input/textarea.
func Fill(ctx context.Context, m *Manager, windowID, ref, selector, value string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return fillJS(reqID, cbURL, ref, selector, value)
	}, timeout)
}

// Type types text character by character.
func Type(ctx context.Context, m *Manager, windowID, ref, selector, text string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return typeJS(reqID, cbURL, ref, selector, text)
	}, timeout)
}

// GetText extracts text content from the page or a specific element.
func GetText(ctx context.Context, m *Manager, windowID, selector string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return getTextJS(reqID, cbURL, selector)
	}, timeout)
}

// Evaluate runs arbitrary JavaScript and returns the result.
func Evaluate(ctx context.Context, m *Manager, windowID, code string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return evalJS(reqID, cbURL, code)
	}, timeout)
}

// Scroll scrolls the page in a direction (up, down, left, right, top, bottom).
func Scroll(ctx context.Context, m *Manager, windowID, direction string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return scrollJS(reqID, cbURL, direction)
	}, timeout)
}

// Wait polls for an element's existence.
func Wait(ctx context.Context, m *Manager, windowID, selector string, timeout time.Duration) (json.RawMessage, error) {
	timeoutMs := int(timeout.Milliseconds())
	if timeoutMs <= 0 {
		timeoutMs = 10000
	}
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return waitJS(reqID, cbURL, selector, timeoutMs)
	}, timeout+2*time.Second) // extra buffer for the JS polling
}

// Hover simulates hovering over an element with realistic cursor movement.
func Hover(ctx context.Context, m *Manager, windowID, ref, selector string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return cursorHoverJS(reqID, cbURL, ref, selector)
	}, timeout)
}

// Select sets a value on a <select> element.
func Select(ctx context.Context, m *Manager, windowID, ref, selector, value string, timeout time.Duration) (json.RawMessage, error) {
	return execJS(ctx, m, windowID, func(reqID, cbURL string) string {
		return selectJS(reqID, cbURL, ref, selector, value)
	}, timeout)
}

// Back navigates back in history using JS history API, then collects page info.
func Back(ctx context.Context, m *Manager, windowID string, timeout time.Duration) (json.RawMessage, error) {
	win, err := m.GetWindow(windowID)
	if err != nil {
		return nil, err
	}
	// Fire history.back() without waiting for a callback
	win.Handle.ExecJS(`history.back();`)
	time.Sleep(1 * time.Second)
	return GetInfo(ctx, m, windowID, timeout)
}

// Forward navigates forward in history using JS history API, then collects page info.
func Forward(ctx context.Context, m *Manager, windowID string, timeout time.Duration) (json.RawMessage, error) {
	win, err := m.GetWindow(windowID)
	if err != nil {
		return nil, err
	}
	win.Handle.ExecJS(`history.forward();`)
	time.Sleep(1 * time.Second)
	return GetInfo(ctx, m, windowID, timeout)
}

// Reload reloads the current page.
func Reload(_ context.Context, m *Manager, windowID string) error {
	win, err := m.GetWindow(windowID)
	if err != nil {
		return err
	}
	win.Handle.Reload()
	return nil
}

// execJS is the core pattern: get window, generate JS with request ID, register callback, exec, wait.
func execJS(ctx context.Context, m *Manager, windowID string, jsGen func(reqID, cbURL string) string, timeout time.Duration) (json.RawMessage, error) {
	win, err := m.GetWindow(windowID)
	if err != nil {
		return nil, err
	}

	reqID := newRequestID()
	cbURL := m.CallbackURL()
	js := jsGen(reqID, cbURL)

	ch := collector.Register(reqID)
	win.Handle.ExecJS(js)

	if timeout <= 0 {
		timeout = defaultTimeout
	}

	select {
	case result := <-ch:
		collector.Cleanup(reqID)
		if result.Error != "" {
			return nil, fmt.Errorf("js error: %s", result.Error)
		}
		return result.Data, nil
	case <-time.After(timeout):
		collector.Cleanup(reqID)
		return nil, fmt.Errorf("timeout waiting for webview response")
	case <-ctx.Done():
		collector.Cleanup(reqID)
		return nil, ctx.Err()
	}
}
