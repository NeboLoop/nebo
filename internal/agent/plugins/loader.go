package plugins

import (
	"context"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"

	"github.com/nebolabs/nebo/internal/agent/comm"

	"github.com/hashicorp/go-plugin"
)

// LoadedPlugin represents a loaded plugin and its client
type LoadedPlugin struct {
	Name        string
	Type        string // "tool", "channel", or "comm"
	Path        string
	Client      *plugin.Client
	RawClient   interface{}
	ToolImpl    ToolPlugin    // Set if Type == "tool"
	ChannelImpl ChannelPlugin // Set if Type == "channel"
	CommImpl    comm.CommPlugin // Set if Type == "comm"
}

// Loader manages plugin discovery, loading, and lifecycle
type Loader struct {
	mu         sync.RWMutex
	pluginDir  string
	plugins    map[string]*LoadedPlugin
	tools      map[string]*LoadedPlugin    // Quick lookup by tool name
	channels   map[string]*LoadedPlugin    // Quick lookup by channel ID
	comms      map[string]*LoadedPlugin    // Quick lookup by comm plugin name
	watcher    *Watcher

	// Callbacks invoked when a comm plugin is loaded/unloaded.
	// Used by agent.go to auto-register with CommPluginManager.
	onCommLoaded   func(comm.CommPlugin)
	onCommUnloaded func(name string)
}

// NewLoader creates a new plugin loader
func NewLoader(pluginDir string) *Loader {
	return &Loader{
		pluginDir: pluginDir,
		plugins:   make(map[string]*LoadedPlugin),
		tools:     make(map[string]*LoadedPlugin),
		channels:  make(map[string]*LoadedPlugin),
		comms:     make(map[string]*LoadedPlugin),
	}
}

// LoadAll discovers and loads all plugins from the plugin directory
func (l *Loader) LoadAll() error {
	l.mu.Lock()
	defer l.mu.Unlock()

	// Ensure directory exists
	if _, err := os.Stat(l.pluginDir); os.IsNotExist(err) {
		log.Printf("[plugins] Plugin directory does not exist: %s", l.pluginDir)
		return nil
	}

	// Find all executables in the plugin directories
	toolsDir := filepath.Join(l.pluginDir, "tools")
	channelsDir := filepath.Join(l.pluginDir, "channels")

	// Load tool plugins
	if _, err := os.Stat(toolsDir); err == nil {
		entries, _ := os.ReadDir(toolsDir)
		for _, entry := range entries {
			if entry.IsDir() {
				continue
			}
			path := filepath.Join(toolsDir, entry.Name())
			if err := l.loadPlugin(path, "tool"); err != nil {
				log.Printf("[plugins] Failed to load tool plugin %s: %v", path, err)
			}
		}
	}

	// Load channel plugins
	if _, err := os.Stat(channelsDir); err == nil {
		entries, _ := os.ReadDir(channelsDir)
		for _, entry := range entries {
			if entry.IsDir() {
				continue
			}
			path := filepath.Join(channelsDir, entry.Name())
			if err := l.loadPlugin(path, "channel"); err != nil {
				log.Printf("[plugins] Failed to load channel plugin %s: %v", path, err)
			}
		}
	}

	// Load comm plugins
	commDir := filepath.Join(l.pluginDir, "comm")
	if _, err := os.Stat(commDir); err == nil {
		entries, _ := os.ReadDir(commDir)
		for _, entry := range entries {
			if entry.IsDir() {
				continue
			}
			path := filepath.Join(commDir, entry.Name())
			if err := l.loadPlugin(path, "comm"); err != nil {
				log.Printf("[plugins] Failed to load comm plugin %s: %v", path, err)
			}
		}
	}

	log.Printf("[plugins] Loaded %d tool plugins, %d channel plugins, %d comm plugins",
		len(l.tools), len(l.channels), len(l.comms))
	return nil
}

// loadPlugin loads a single plugin (must hold lock)
func (l *Loader) loadPlugin(path string, pluginType string) error {
	// Check if executable
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if info.Mode()&0111 == 0 {
		return fmt.Errorf("not executable: %s", path)
	}

	// Create plugin client
	client := plugin.NewClient(&plugin.ClientConfig{
		HandshakeConfig: Handshake,
		Plugins:         PluginMap,
		Cmd:             exec.Command(path),
		AllowedProtocols: []plugin.Protocol{
			plugin.ProtocolNetRPC,
		},
	})

	// Connect via RPC
	rpcClient, err := client.Client()
	if err != nil {
		client.Kill()
		return fmt.Errorf("failed to connect to plugin: %w", err)
	}

	// Request the plugin
	raw, err := rpcClient.Dispense(pluginType)
	if err != nil {
		client.Kill()
		return fmt.Errorf("failed to dispense %s plugin: %w", pluginType, err)
	}

	loaded := &LoadedPlugin{
		Path:      path,
		Type:      pluginType,
		Client:    client,
		RawClient: raw,
	}

	// Get plugin name and register
	switch pluginType {
	case "tool":
		tool, ok := raw.(ToolPlugin)
		if !ok {
			client.Kill()
			return fmt.Errorf("plugin does not implement ToolPlugin interface")
		}
		loaded.Name = tool.Name()
		loaded.ToolImpl = tool
		l.tools[loaded.Name] = loaded

	case "channel":
		channel, ok := raw.(ChannelPlugin)
		if !ok {
			client.Kill()
			return fmt.Errorf("plugin does not implement ChannelPlugin interface")
		}
		loaded.Name = channel.ID()
		loaded.ChannelImpl = channel
		l.channels[loaded.Name] = loaded

	case "comm":
		commPlugin, ok := raw.(comm.CommPlugin)
		if !ok {
			client.Kill()
			return fmt.Errorf("plugin does not implement comm.CommPlugin interface")
		}
		loaded.Name = commPlugin.Name()
		loaded.CommImpl = commPlugin
		l.comms[loaded.Name] = loaded
		if l.onCommLoaded != nil {
			l.onCommLoaded(commPlugin)
		}
	}

	l.plugins[path] = loaded
	log.Printf("[plugins] Loaded %s plugin: %s", pluginType, loaded.Name)
	return nil
}

// Load loads a single plugin by path
func (l *Loader) Load(path string) error {
	l.mu.Lock()
	defer l.mu.Unlock()

	// Determine type from path
	pluginType := "tool"
	if strings.Contains(path, "channels") {
		pluginType = "channel"
	} else if strings.Contains(path, "comm") {
		pluginType = "comm"
	}

	return l.loadPlugin(path, pluginType)
}

// Unload unloads a plugin by path
func (l *Loader) Unload(path string) error {
	l.mu.Lock()
	defer l.mu.Unlock()

	loaded, ok := l.plugins[path]
	if !ok {
		return fmt.Errorf("plugin not found: %s", path)
	}

	// Remove from type-specific maps
	switch loaded.Type {
	case "tool":
		delete(l.tools, loaded.Name)
	case "channel":
		delete(l.channels, loaded.Name)
	case "comm":
		// Stop polling before killing the plugin process
		if cp, ok := loaded.RawClient.(*CommRPCClient); ok {
			cp.stopPolling()
		}
		delete(l.comms, loaded.Name)
		if l.onCommUnloaded != nil {
			l.onCommUnloaded(loaded.Name)
		}
	}

	// Kill the plugin process
	loaded.Client.Kill()
	delete(l.plugins, path)

	log.Printf("[plugins] Unloaded %s plugin: %s", loaded.Type, loaded.Name)
	return nil
}

// GetTool returns a tool plugin by name
func (l *Loader) GetTool(name string) (ToolPlugin, bool) {
	l.mu.RLock()
	defer l.mu.RUnlock()

	loaded, ok := l.tools[name]
	if !ok {
		return nil, false
	}
	return loaded.ToolImpl, true
}

// GetChannel returns a channel plugin by ID
func (l *Loader) GetChannel(id string) (ChannelPlugin, bool) {
	l.mu.RLock()
	defer l.mu.RUnlock()

	loaded, ok := l.channels[id]
	if !ok {
		return nil, false
	}
	return loaded.ChannelImpl, true
}

// ListTools returns all loaded tool names
func (l *Loader) ListTools() []string {
	l.mu.RLock()
	defer l.mu.RUnlock()

	names := make([]string, 0, len(l.tools))
	for name := range l.tools {
		names = append(names, name)
	}
	return names
}

// ListChannels returns all loaded channel IDs
func (l *Loader) ListChannels() []string {
	l.mu.RLock()
	defer l.mu.RUnlock()

	ids := make([]string, 0, len(l.channels))
	for id := range l.channels {
		ids = append(ids, id)
	}
	return ids
}

// GetComm returns a comm plugin by name
func (l *Loader) GetComm(name string) (comm.CommPlugin, bool) {
	l.mu.RLock()
	defer l.mu.RUnlock()

	loaded, ok := l.comms[name]
	if !ok {
		return nil, false
	}
	return loaded.CommImpl, true
}

// ListComms returns all loaded comm plugin names
func (l *Loader) ListComms() []string {
	l.mu.RLock()
	defer l.mu.RUnlock()

	names := make([]string, 0, len(l.comms))
	for name := range l.comms {
		names = append(names, name)
	}
	return names
}

// SetCommCallbacks sets callbacks for comm plugin load/unload events.
// Used by agent.go to auto-register discovered plugins with CommPluginManager.
func (l *Loader) SetCommCallbacks(onLoaded func(comm.CommPlugin), onUnloaded func(name string)) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.onCommLoaded = onLoaded
	l.onCommUnloaded = onUnloaded
}

// Watch starts watching for plugin changes (hot-reload)
func (l *Loader) Watch(ctx context.Context) error {
	watcher, err := NewWatcher(l.pluginDir, l)
	if err != nil {
		return err
	}
	l.watcher = watcher
	return watcher.Watch(ctx)
}

// Stop stops watching and unloads all plugins
func (l *Loader) Stop() {
	if l.watcher != nil {
		l.watcher.Stop()
	}

	l.mu.Lock()
	defer l.mu.Unlock()

	for path, loaded := range l.plugins {
		loaded.Client.Kill()
		log.Printf("[plugins] Stopped plugin: %s", loaded.Name)
		delete(l.plugins, path)
	}

	l.tools = make(map[string]*LoadedPlugin)
	l.channels = make(map[string]*LoadedPlugin)
	l.comms = make(map[string]*LoadedPlugin)
}

// Count returns the number of loaded plugins
func (l *Loader) Count() int {
	l.mu.RLock()
	defer l.mu.RUnlock()
	return len(l.plugins)
}
