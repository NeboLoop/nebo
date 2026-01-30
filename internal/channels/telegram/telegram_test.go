package telegram

import (
	"testing"

	"nebo/internal/channels"
)

func TestNew(t *testing.T) {
	adapter := New()
	if adapter == nil {
		t.Error("expected non-nil adapter")
	}
}

func TestID(t *testing.T) {
	adapter := New()
	if adapter.ID() != "telegram" {
		t.Errorf("expected ID 'telegram', got '%s'", adapter.ID())
	}
}

func TestSetHandler(t *testing.T) {
	adapter := New()

	called := false
	adapter.SetHandler(func(msg channels.InboundMessage) {
		called = true
	})

	// The handler won't be called in this test since we're not connected
	// Just verify it doesn't panic
	if called {
		t.Error("handler should not have been called")
	}
}
