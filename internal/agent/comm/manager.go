package comm

import (
	"context"
	"fmt"
	"sync"
)

// CommPluginManager manages loaded comm plugins and routes messages.
// Only one plugin is active at a time; all messages route through it.
type CommPluginManager struct {
	plugins map[string]CommPlugin
	active  CommPlugin
	handler func(CommMessage) // Handler for incoming messages
	topics  []string          // Currently subscribed topics
	mu      sync.RWMutex
}

// NewCommPluginManager creates a new comm plugin manager
func NewCommPluginManager() *CommPluginManager {
	return &CommPluginManager{
		plugins: make(map[string]CommPlugin),
	}
}

// Register adds a plugin to the manager (does not activate it)
func (m *CommPluginManager) Register(plugin CommPlugin) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.plugins[plugin.Name()] = plugin
	fmt.Printf("[Comm] Registered plugin: %s (v%s)\n", plugin.Name(), plugin.Version())
}

// Unregister removes a plugin from the manager.
// If the removed plugin was active, it is disconnected and cleared.
func (m *CommPluginManager) Unregister(name string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.active != nil && m.active.Name() == name {
		_ = m.active.Disconnect(context.Background())
		m.active = nil
	}
	delete(m.plugins, name)
	fmt.Printf("[Comm] Unregistered plugin: %s\n", name)
}

// SetActive activates a specific plugin by name.
// If another plugin is active, it is disconnected first.
func (m *CommPluginManager) SetActive(name string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	plugin, ok := m.plugins[name]
	if !ok {
		available := make([]string, 0, len(m.plugins))
		for k := range m.plugins {
			available = append(available, k)
		}
		return fmt.Errorf("comm plugin %q not found, available: %v", name, available)
	}

	// Disconnect current active plugin if different
	if m.active != nil && m.active.Name() != name {
		if err := m.active.Disconnect(context.Background()); err != nil {
			fmt.Printf("[Comm] Warning: failed to disconnect %s: %v\n", m.active.Name(), err)
		}
	}

	m.active = plugin

	// Wire the message handler into the plugin
	if m.handler != nil {
		plugin.SetMessageHandler(m.handler)
	}

	fmt.Printf("[Comm] Active plugin set to: %s\n", name)
	return nil
}

// GetActive returns the currently active plugin (may be nil)
func (m *CommPluginManager) GetActive() CommPlugin {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.active
}

// Send sends a message through the active plugin
func (m *CommPluginManager) Send(ctx context.Context, msg CommMessage) error {
	m.mu.RLock()
	active := m.active
	m.mu.RUnlock()

	if active == nil {
		return fmt.Errorf("no active comm plugin")
	}
	if !active.IsConnected() {
		return fmt.Errorf("comm plugin %q is not connected", active.Name())
	}
	return active.Send(ctx, msg)
}

// Subscribe subscribes to a topic on the active plugin
func (m *CommPluginManager) Subscribe(ctx context.Context, topic string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.active == nil {
		return fmt.Errorf("no active comm plugin")
	}
	if err := m.active.Subscribe(ctx, topic); err != nil {
		return err
	}

	// Track subscribed topics
	for _, t := range m.topics {
		if t == topic {
			return nil // Already tracked
		}
	}
	m.topics = append(m.topics, topic)
	fmt.Printf("[Comm] Subscribed to topic: %s\n", topic)
	return nil
}

// Unsubscribe unsubscribes from a topic on the active plugin
func (m *CommPluginManager) Unsubscribe(ctx context.Context, topic string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.active == nil {
		return fmt.Errorf("no active comm plugin")
	}
	if err := m.active.Unsubscribe(ctx, topic); err != nil {
		return err
	}

	// Remove from tracked topics
	for i, t := range m.topics {
		if t == topic {
			m.topics = append(m.topics[:i], m.topics[i+1:]...)
			break
		}
	}
	fmt.Printf("[Comm] Unsubscribed from topic: %s\n", topic)
	return nil
}

// SetMessageHandler sets the callback for incoming messages from all plugins
func (m *CommPluginManager) SetMessageHandler(handler func(CommMessage)) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.handler = handler

	// Wire into the active plugin if one exists
	if m.active != nil {
		m.active.SetMessageHandler(handler)
	}
}

// ListTopics returns currently subscribed topics
func (m *CommPluginManager) ListTopics() []string {
	m.mu.RLock()
	defer m.mu.RUnlock()
	result := make([]string, len(m.topics))
	copy(result, m.topics)
	return result
}

// Status returns the current status of the manager
func (m *CommPluginManager) Status(agentID string) ManagerStatus {
	m.mu.RLock()
	defer m.mu.RUnlock()

	status := ManagerStatus{
		AgentID: agentID,
		Topics:  make([]string, len(m.topics)),
	}
	copy(status.Topics, m.topics)

	if m.active != nil {
		status.PluginName = m.active.Name()
		status.Connected = m.active.IsConnected()
	}
	return status
}

// ListPlugins returns names of all registered plugins
func (m *CommPluginManager) ListPlugins() []string {
	m.mu.RLock()
	defer m.mu.RUnlock()
	names := make([]string, 0, len(m.plugins))
	for name := range m.plugins {
		names = append(names, name)
	}
	return names
}

// Shutdown disconnects all plugins
func (m *CommPluginManager) Shutdown(ctx context.Context) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	var lastErr error
	for name, plugin := range m.plugins {
		if plugin.IsConnected() {
			if err := plugin.Disconnect(ctx); err != nil {
				fmt.Printf("[Comm] Warning: failed to disconnect %s: %v\n", name, err)
				lastErr = err
			}
		}
	}
	m.active = nil
	m.topics = nil
	return lastErr
}
