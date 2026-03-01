package apps

import (
	"context"
	"fmt"
	"sort"
	"sync"
	"time"

	pb "github.com/neboloop/nebo/internal/apps/pb"
)

// hookTimeout is the maximum time a hook call can take before being skipped.
const hookTimeout = 500 * time.Millisecond

// circuitBreakerThreshold is the number of consecutive failures before
// an app's hooks are disabled until Nebo restart.
const circuitBreakerThreshold = 3

// ValidHookNames lists all recognized hook points that apps can subscribe to.
var ValidHookNames = map[string]bool{
	"tool.pre_execute":        true,
	"tool.post_execute":       true,
	"message.pre_send":        true,
	"message.post_receive":    true,
	"memory.pre_store":        true,
	"memory.pre_recall":       true,
	"session.message_append":  true,
	"prompt.system_sections":  true,
	"steering.generate":       true,
	"response.stream":         true,
}

// hookEntry represents a single app's subscription to a hook.
type hookEntry struct {
	appID    string
	hookType string // "action" or "filter"
	priority int
	client   pb.HookServiceClient
}

// HookDispatcher manages hook registrations and dispatches hook calls
// to subscribed apps in priority order with timeout and circuit breaking.
type HookDispatcher struct {
	mu       sync.RWMutex
	hooks    map[string][]*hookEntry // hook name → sorted entries
	failures map[string]int          // appID → consecutive failure count
	disabled map[string]bool         // appID → true if circuit breaker tripped
}

// NewHookDispatcher creates a new hook dispatcher.
func NewHookDispatcher() *HookDispatcher {
	return &HookDispatcher{
		hooks:    make(map[string][]*hookEntry),
		failures: make(map[string]int),
		disabled: make(map[string]bool),
	}
}

// Register adds a hook subscription for an app.
func (d *HookDispatcher) Register(appID string, reg *pb.HookRegistration, client pb.HookServiceClient) {
	if !ValidHookNames[reg.Hook] {
		fmt.Printf("[hooks] Warning: app %s tried to register unknown hook %q — skipping\n", appID, reg.Hook)
		return
	}
	if reg.Type != "action" && reg.Type != "filter" {
		fmt.Printf("[hooks] Warning: app %s hook %q has invalid type %q — skipping\n", appID, reg.Hook, reg.Type)
		return
	}

	priority := int(reg.Priority)
	if priority == 0 {
		priority = 10 // default priority
	}

	entry := &hookEntry{
		appID:    appID,
		hookType: reg.Type,
		priority: priority,
		client:   client,
	}

	d.mu.Lock()
	defer d.mu.Unlock()

	d.hooks[reg.Hook] = append(d.hooks[reg.Hook], entry)

	// Keep sorted by priority (lower = first)
	sort.Slice(d.hooks[reg.Hook], func(i, j int) bool {
		return d.hooks[reg.Hook][i].priority < d.hooks[reg.Hook][j].priority
	})

	fmt.Printf("[hooks] Registered %s hook %q for app %s (priority %d)\n", reg.Type, reg.Hook, appID, priority)
}

// UnregisterApp removes all hook subscriptions for an app.
func (d *HookDispatcher) UnregisterApp(appID string) {
	d.mu.Lock()
	defer d.mu.Unlock()

	for hookName, entries := range d.hooks {
		filtered := entries[:0]
		for _, e := range entries {
			if e.appID != appID {
				filtered = append(filtered, e)
			}
		}
		if len(filtered) == 0 {
			delete(d.hooks, hookName)
		} else {
			d.hooks[hookName] = filtered
		}
	}

	delete(d.failures, appID)
	delete(d.disabled, appID)

	fmt.Printf("[hooks] Unregistered all hooks for app %s\n", appID)
}

// HasSubscribers returns true if any app is subscribed to the given hook.
func (d *HookDispatcher) HasSubscribers(hook string) bool {
	d.mu.RLock()
	defer d.mu.RUnlock()
	entries := d.hooks[hook]
	if len(entries) == 0 {
		return false
	}
	// Check that at least one subscriber is not disabled
	for _, e := range entries {
		if !d.disabled[e.appID] {
			return true
		}
	}
	return false
}

// ApplyFilter calls all filter subscribers in priority order.
// Each filter receives the output of the previous one (chain).
// Returns the (possibly modified) payload and whether any filter set handled=true.
// If no subscribers exist or all fail, returns the original payload unchanged.
func (d *HookDispatcher) ApplyFilter(ctx context.Context, hook string, payload []byte) ([]byte, bool) {
	d.mu.RLock()
	entries := make([]*hookEntry, len(d.hooks[hook]))
	copy(entries, d.hooks[hook])
	d.mu.RUnlock()

	if len(entries) == 0 {
		return payload, false
	}

	current := payload
	for _, entry := range entries {
		if entry.hookType != "filter" {
			continue
		}
		if d.isDisabled(entry.appID) {
			continue
		}

		modified, handled, err := d.callFilter(ctx, entry, hook, current)
		if err != nil {
			d.recordFailure(entry.appID)
			fmt.Printf("[hooks] Filter %q failed for app %s: %v\n", hook, entry.appID, err)
			continue
		}
		d.recordSuccess(entry.appID)

		if handled {
			return modified, true
		}
		if modified != nil {
			current = modified
		}
	}

	return current, false
}

// DoAction calls all action subscribers (fire-and-forget with timeout).
// Actions run sequentially in priority order but their results are discarded.
func (d *HookDispatcher) DoAction(ctx context.Context, hook string, payload []byte) {
	d.mu.RLock()
	entries := make([]*hookEntry, len(d.hooks[hook]))
	copy(entries, d.hooks[hook])
	d.mu.RUnlock()

	if len(entries) == 0 {
		return
	}

	for _, entry := range entries {
		if entry.hookType != "action" {
			continue
		}
		if d.isDisabled(entry.appID) {
			continue
		}

		if err := d.callAction(ctx, entry, hook, payload); err != nil {
			d.recordFailure(entry.appID)
			fmt.Printf("[hooks] Action %q failed for app %s: %v\n", hook, entry.appID, err)
			continue
		}
		d.recordSuccess(entry.appID)
	}
}

// callFilter invokes a single filter hook with timeout.
func (d *HookDispatcher) callFilter(ctx context.Context, entry *hookEntry, hook string, payload []byte) ([]byte, bool, error) {
	callCtx, cancel := context.WithTimeout(ctx, hookTimeout)
	defer cancel()

	resp, err := entry.client.ApplyFilter(callCtx, &pb.HookRequest{
		Hook:        hook,
		Payload:     payload,
		TimestampMs: time.Now().UnixMilli(),
	})
	if err != nil {
		return nil, false, fmt.Errorf("grpc call: %w", err)
	}

	if resp.Error != "" {
		return nil, false, fmt.Errorf("app error: %s", resp.Error)
	}

	return resp.Payload, resp.Handled, nil
}

// callAction invokes a single action hook with timeout.
func (d *HookDispatcher) callAction(ctx context.Context, entry *hookEntry, hook string, payload []byte) error {
	callCtx, cancel := context.WithTimeout(ctx, hookTimeout)
	defer cancel()

	_, err := entry.client.DoAction(callCtx, &pb.HookRequest{
		Hook:        hook,
		Payload:     payload,
		TimestampMs: time.Now().UnixMilli(),
	})
	return err
}

// isDisabled returns true if the app's hooks have been disabled by the circuit breaker.
func (d *HookDispatcher) isDisabled(appID string) bool {
	d.mu.RLock()
	defer d.mu.RUnlock()
	return d.disabled[appID]
}

// recordFailure increments the failure counter and trips the circuit breaker if threshold is reached.
func (d *HookDispatcher) recordFailure(appID string) {
	d.mu.Lock()
	defer d.mu.Unlock()

	d.failures[appID]++
	if d.failures[appID] >= circuitBreakerThreshold {
		d.disabled[appID] = true
		fmt.Printf("[hooks] Circuit breaker tripped for app %s after %d consecutive failures — hooks disabled until restart\n",
			appID, d.failures[appID])
	}
}

// recordSuccess resets the failure counter for an app.
func (d *HookDispatcher) recordSuccess(appID string) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.failures[appID] = 0
}
