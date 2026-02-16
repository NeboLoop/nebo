package webview

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestCallbackCollectorDelivery(t *testing.T) {
	c := &CallbackCollector{pending: make(map[string]chan CallbackResult)}

	ch := c.Register("req-1")

	// Deliver
	c.Deliver(CallbackResult{
		RequestID: "req-1",
		Data:      json.RawMessage(`{"title":"Hello"}`),
	})

	select {
	case result := <-ch:
		if result.Error != "" {
			t.Fatalf("unexpected error: %s", result.Error)
		}
		if string(result.Data) != `{"title":"Hello"}` {
			t.Fatalf("unexpected data: %s", string(result.Data))
		}
	case <-time.After(time.Second):
		t.Fatal("timeout waiting for result")
	}
}

func TestCallbackCollectorDeliverUnknown(t *testing.T) {
	c := &CallbackCollector{pending: make(map[string]chan CallbackResult)}

	// Deliver to unknown request — should not panic
	c.Deliver(CallbackResult{RequestID: "unknown", Data: json.RawMessage(`{}`)})
}

func TestCallbackCollectorCleanup(t *testing.T) {
	c := &CallbackCollector{pending: make(map[string]chan CallbackResult)}

	c.Register("req-2")
	c.Cleanup("req-2")

	c.mu.Lock()
	_, exists := c.pending["req-2"]
	c.mu.Unlock()

	if exists {
		t.Error("expected request to be cleaned up")
	}
}

func TestWaitForResultTimeout(t *testing.T) {
	// Override the global collector for this test
	oldPending := collector.pending
	collector.pending = make(map[string]chan CallbackResult)
	defer func() { collector.pending = oldPending }()

	ctx := context.Background()
	_, err := WaitForResult(ctx, "never-delivered", 50*time.Millisecond)
	if err == nil {
		t.Fatal("expected timeout error")
	}
}

func TestWaitForResultContextCancel(t *testing.T) {
	oldPending := collector.pending
	collector.pending = make(map[string]chan CallbackResult)
	defer func() { collector.pending = oldPending }()

	ctx, cancel := context.WithCancel(context.Background())
	cancel() // cancel immediately

	_, err := WaitForResult(ctx, "cancelled", 5*time.Second)
	if err == nil {
		t.Fatal("expected context cancelled error")
	}
}

func TestWaitForResultJSError(t *testing.T) {
	oldPending := collector.pending
	collector.pending = make(map[string]chan CallbackResult)
	defer func() { collector.pending = oldPending }()

	go func() {
		time.Sleep(10 * time.Millisecond)
		collector.Deliver(CallbackResult{
			RequestID: "err-req",
			Error:     "ReferenceError: foo is not defined",
		})
	}()

	ctx := context.Background()
	_, err := WaitForResult(ctx, "err-req", time.Second)
	if err == nil {
		t.Fatal("expected JS error")
	}
	if err.Error() != "js error: ReferenceError: foo is not defined" {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestCallbackHandler(t *testing.T) {
	oldPending := collector.pending
	collector.pending = make(map[string]chan CallbackResult)
	defer func() { collector.pending = oldPending }()

	handler := CallbackHandler()

	// Register a pending request
	ch := collector.Register("handler-req")

	// POST result
	body := `{"requestId":"handler-req","data":{"url":"https://example.com"}}`
	req := httptest.NewRequest(http.MethodPost, "/internal/webview/callback", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	select {
	case result := <-ch:
		if string(result.Data) != `{"url":"https://example.com"}` {
			t.Fatalf("unexpected data: %s", string(result.Data))
		}
	case <-time.After(time.Second):
		t.Fatal("timeout waiting for result via handler")
	}
}

func TestCallbackHandlerCORSPreflight(t *testing.T) {
	handler := CallbackHandler()
	req := httptest.NewRequest(http.MethodOptions, "/internal/webview/callback", nil)
	req.Header.Set("Origin", "https://somesite.com")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d", w.Code)
	}
	if got := w.Header().Get("Access-Control-Allow-Origin"); got != "https://somesite.com" {
		t.Fatalf("expected CORS origin https://somesite.com, got %s", got)
	}
}

func TestCallbackHandlerCORSOnPost(t *testing.T) {
	oldPending := collector.pending
	collector.pending = make(map[string]chan CallbackResult)
	defer func() { collector.pending = oldPending }()

	handler := CallbackHandler()
	collector.Register("cors-req")

	body := `{"requestId":"cors-req","data":{"ok":true}}`
	req := httptest.NewRequest(http.MethodPost, "/internal/webview/callback", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Origin", "https://external-site.com")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}
	if got := w.Header().Get("Access-Control-Allow-Origin"); got != "https://external-site.com" {
		t.Fatalf("expected CORS origin https://external-site.com, got %s", got)
	}
}

func TestCallbackHandlerBadMethod(t *testing.T) {
	handler := CallbackHandler()
	req := httptest.NewRequest(http.MethodGet, "/internal/webview/callback", nil)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d", w.Code)
	}
}

func TestCallbackHandlerBadJSON(t *testing.T) {
	handler := CallbackHandler()
	req := httptest.NewRequest(http.MethodPost, "/internal/webview/callback", bytes.NewBufferString("not json"))
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", w.Code)
	}
}

func TestCallbackHandlerMissingRequestID(t *testing.T) {
	handler := CallbackHandler()
	body := `{"data":{"foo":"bar"}}`
	req := httptest.NewRequest(http.MethodPost, "/internal/webview/callback", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", w.Code)
	}
}
