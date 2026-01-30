// Package lifecycle provides event hooks for GoBot startup and shutdown.
package lifecycle

import (
	"sync"

	"nebo/internal/logging"
)

// Event types for lifecycle hooks
type Event string

const (
	EventServerStarted      Event = "server_started"
	EventAgentConnected     Event = "agent_connected"
	EventAgentDisconnected  Event = "agent_disconnected"
	EventShutdownStarted    Event = "shutdown_started"
	EventShutdownComplete   Event = "shutdown_complete"
)

// Handler is a function that handles a lifecycle event
type Handler func(event Event, data any)

// Manager manages lifecycle event subscriptions and dispatching
type Manager struct {
	mu       sync.RWMutex
	handlers map[Event][]Handler
}

// Global lifecycle manager
var global = &Manager{
	handlers: make(map[Event][]Handler),
}

// On registers a handler for a lifecycle event
func On(event Event, handler Handler) {
	global.On(event, handler)
}

// Emit dispatches an event to all registered handlers
func Emit(event Event, data any) {
	global.Emit(event, data)
}

// On registers a handler for a lifecycle event
func (m *Manager) On(event Event, handler Handler) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.handlers[event] = append(m.handlers[event], handler)
}

// Emit dispatches an event to all registered handlers
func (m *Manager) Emit(event Event, data any) {
	m.mu.RLock()
	handlers := m.handlers[event]
	m.mu.RUnlock()

	logging.Infof("[lifecycle] Emitting event: %s", event)
	for _, h := range handlers {
		// Run handlers synchronously (they can spawn goroutines if needed)
		h(event, data)
	}
}

// OnAgentConnected is a convenience function to register an agent connected handler
func OnAgentConnected(handler func(agentID string)) {
	On(EventAgentConnected, func(e Event, data any) {
		if id, ok := data.(string); ok {
			handler(id)
		}
	})
}

// OnAgentDisconnected is a convenience function to register an agent disconnected handler
func OnAgentDisconnected(handler func(agentID string)) {
	On(EventAgentDisconnected, func(e Event, data any) {
		if id, ok := data.(string); ok {
			handler(id)
		}
	})
}

// OnServerStarted is a convenience function to register a server started handler
func OnServerStarted(handler func()) {
	On(EventServerStarted, func(e Event, data any) {
		handler()
	})
}

// OnShutdown is a convenience function to register a shutdown handler
func OnShutdown(handler func()) {
	On(EventShutdownStarted, func(e Event, data any) {
		handler()
	})
}
