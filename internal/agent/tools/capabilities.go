// Package tools provides the agent's built-in tools and capability system.
//
// Capabilities are platform-native tools that get compiled into the binary.
// Each capability can be tagged with the platforms it supports (darwin, linux,
// windows, ios, android). The registry automatically filters tools based on
// the current platform at startup.
//
// This replaces the go-plugin architecture which doesn't work on mobile
// platforms (iOS/Android can't spawn subprocesses for RPC).
package tools

import (
	"fmt"
	"runtime"
	"sync"
)

// Platform constants for capability tagging
const (
	PlatformDarwin  = "darwin"
	PlatformLinux   = "linux"
	PlatformWindows = "windows"
	PlatformIOS     = "ios"
	PlatformAndroid = "android"
	PlatformAll     = "all" // Available everywhere
)

// Capability wraps a Tool with platform metadata
type Capability struct {
	// Tool is the underlying tool implementation
	Tool Tool

	// Platforms lists which platforms this capability supports.
	// Empty or containing "all" means available everywhere.
	Platforms []string

	// Category groups related capabilities (e.g., "system", "media", "productivity")
	Category string

	// RequiresSetup indicates if the capability needs user configuration
	RequiresSetup bool
}

// CapabilityRegistry manages platform-aware tool registration
type CapabilityRegistry struct {
	mu           sync.RWMutex
	capabilities map[string]*Capability
	platform     string
}

// NewCapabilityRegistry creates a registry for the current platform
func NewCapabilityRegistry() *CapabilityRegistry {
	return &CapabilityRegistry{
		capabilities: make(map[string]*Capability),
		platform:     detectPlatform(),
	}
}

// detectPlatform returns the current platform identifier
func detectPlatform() string {
	// runtime.GOOS returns: darwin, linux, windows, android
	// For iOS, we need to check at build time with tags
	goos := runtime.GOOS

	// iOS is darwin + ios build tag (handled at compile time)
	// This will be "darwin" on macOS and "ios" when built with ios tag
	if isIOS {
		return PlatformIOS
	}

	return goos
}

// isIOS is set to true when building with the ios build tag
// See capabilities_ios.go for the override
var isIOS = false

// Register adds a capability to the registry if it's available on the current platform
func (r *CapabilityRegistry) Register(cap *Capability) bool {
	if !r.isAvailable(cap) {
		return false
	}

	r.mu.Lock()
	defer r.mu.Unlock()

	name := cap.Tool.Name()
	if existing, ok := r.capabilities[name]; ok {
		fmt.Printf("[Capabilities] WARNING: capability %q already registered (%T), overwritten by %T\n",
			name, existing.Tool, cap.Tool)
	}
	r.capabilities[name] = cap
	return true
}

// isAvailable checks if a capability is available on the current platform
func (r *CapabilityRegistry) isAvailable(cap *Capability) bool {
	if len(cap.Platforms) == 0 {
		return true // No platforms specified = available everywhere
	}

	for _, p := range cap.Platforms {
		if p == PlatformAll || p == r.platform {
			return true
		}
	}
	return false
}

// Get returns a capability by name
func (r *CapabilityRegistry) Get(name string) (*Capability, bool) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	cap, ok := r.capabilities[name]
	return cap, ok
}

// List returns all registered capabilities
func (r *CapabilityRegistry) List() []*Capability {
	r.mu.RLock()
	defer r.mu.RUnlock()

	caps := make([]*Capability, 0, len(r.capabilities))
	for _, cap := range r.capabilities {
		caps = append(caps, cap)
	}
	return caps
}

// ListByCategory returns capabilities in a specific category
func (r *CapabilityRegistry) ListByCategory(category string) []*Capability {
	r.mu.RLock()
	defer r.mu.RUnlock()

	var caps []*Capability
	for _, cap := range r.capabilities {
		if cap.Category == category {
			caps = append(caps, cap)
		}
	}
	return caps
}

// Platform returns the current platform
func (r *CapabilityRegistry) Platform() string {
	return r.platform
}

// RegisterToToolRegistry copies all capabilities to a traditional tool registry
// This bridges the new capability system with the existing tool infrastructure
func (r *CapabilityRegistry) RegisterToToolRegistry(tr *Registry) {
	r.mu.RLock()
	defer r.mu.RUnlock()

	for _, cap := range r.capabilities {
		tr.Register(cap.Tool)
	}
}

// Global capability registry instance
var capabilities = NewCapabilityRegistry()

// RegisterCapability registers a capability with the global registry
func RegisterCapability(cap *Capability) bool {
	return capabilities.Register(cap)
}

// GetCapability returns a capability from the global registry
func GetCapability(name string) (*Capability, bool) {
	return capabilities.Get(name)
}

// ListCapabilities returns all capabilities from the global registry
func ListCapabilities() []*Capability {
	return capabilities.List()
}

// CurrentPlatform returns the detected platform
func CurrentPlatform() string {
	return capabilities.Platform()
}

// RegisterPlatformCapabilities registers all platform-specific capabilities.
// This is called from init() functions in platform-specific files.
func RegisterPlatformCapabilities(tr *Registry) {
	capabilities.RegisterToToolRegistry(tr)
}

// categoryToPermission maps capability categories to permission keys
var categoryToPermission = map[string]string{
	"productivity": "contacts",
	"system":       "system",
	"media":        "media",
	"desktop":      "desktop",
}

// RegisterPlatformCapabilitiesWithPermissions registers platform capabilities
// filtered by the given permission map. A nil map registers all capabilities.
func RegisterPlatformCapabilitiesWithPermissions(tr *Registry, permissions map[string]bool) {
	if permissions == nil {
		capabilities.RegisterToToolRegistry(tr)
		return
	}

	capabilities.mu.RLock()
	defer capabilities.mu.RUnlock()

	for _, cap := range capabilities.capabilities {
		permKey := categoryToPermission[cap.Category]
		if permKey == "" {
			// Unknown category — register by default
			tr.Register(cap.Tool)
			continue
		}
		if permissions[permKey] {
			tr.Register(cap.Tool)
		}
	}
}
