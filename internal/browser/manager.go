package browser

import (
	"context"
	"fmt"
	"sync"
	"time"
)

// Manager manages browser instances and sessions.
type Manager struct {
	mu sync.RWMutex

	config   *ResolvedConfig
	browsers map[string]*RunningChrome   // profileName -> running browser
	relays   map[string]*ExtensionRelay  // profileName -> extension relay
	started  bool
}

var (
	managerOnce sync.Once
	manager     *Manager
)

// GetManager returns the singleton browser manager.
func GetManager() *Manager {
	managerOnce.Do(func() {
		manager = &Manager{
			browsers: make(map[string]*RunningChrome),
			relays:   make(map[string]*ExtensionRelay),
		}
	})
	return manager
}

// Start starts the browser manager with the given config.
func (m *Manager) Start(cfg Config) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.started {
		return nil
	}

	m.config = ResolveConfig(cfg)

	if !m.config.Enabled {
		return nil
	}

	// Extension relay is now mounted on the main nebo server at /relay
	// No standalone relay needed here

	m.started = true
	return nil
}

// Stop stops the browser manager and all browsers.
func (m *Manager) Stop() error {
	m.mu.Lock()
	defer m.mu.Unlock()

	if !m.started {
		return nil
	}

	// Close all sessions
	CloseAllSessions()

	// Stop all managed browsers
	for name, running := range m.browsers {
		if running != nil {
			_ = StopChrome(running, 5*time.Second)
		}
		delete(m.browsers, name)
	}

	// Stop all relays
	for name, relay := range m.relays {
		if relay != nil {
			_ = relay.Stop()
		}
		delete(m.relays, name)
	}

	m.started = false
	return nil
}

// Config returns the resolved config.
func (m *Manager) Config() *ResolvedConfig {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.config
}

// GetSession gets or creates a browser session for a profile.
func (m *Manager) GetSession(ctx context.Context, profileName string) (*Session, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if !m.started {
		return nil, fmt.Errorf("browser manager not started")
	}

	if profileName == "" {
		profileName = DefaultProfileName
	}

	profile := m.config.GetProfile(profileName)
	if profile == nil {
		return nil, fmt.Errorf("unknown profile: %s", profileName)
	}

	// For managed (nebo) profiles, ensure browser is running
	if profile.Driver == DriverNebo {
		if err := m.ensureBrowserRunning(profile); err != nil {
			return nil, err
		}
	}

	// Get or create session
	return GetOrCreateSession(ctx, profile)
}

// ensureBrowserRunning ensures a managed browser is running for the profile.
func (m *Manager) ensureBrowserRunning(profile *ResolvedProfile) error {
	// Check if already running
	if running, ok := m.browsers[profile.Name]; ok && running != nil {
		// Verify it's still reachable
		if IsChromeReachable(profile.CDPUrl, time.Second) {
			return nil
		}
		// Browser died, clean up
		delete(m.browsers, profile.Name)
	}

	// Launch browser
	running, err := LaunchChrome(m.config, profile)
	if err != nil {
		return fmt.Errorf("failed to launch browser for profile %s: %w", profile.Name, err)
	}

	m.browsers[profile.Name] = running
	return nil
}

// StopBrowser stops the browser for a profile.
func (m *Manager) StopBrowser(profileName string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	// Close session first
	_ = CloseSession(profileName)

	// Stop browser
	running, ok := m.browsers[profileName]
	if !ok || running == nil {
		return nil
	}

	err := StopChrome(running, 5*time.Second)
	delete(m.browsers, profileName)
	return err
}

// ListProfiles returns available profile names.
func (m *Manager) ListProfiles() []string {
	m.mu.RLock()
	defer m.mu.RUnlock()

	if m.config == nil {
		return nil
	}

	names := make([]string, 0, len(m.config.Profiles))
	for name := range m.config.Profiles {
		names = append(names, name)
	}
	return names
}

// GetProfile returns a profile by name.
func (m *Manager) GetProfile(name string) *ResolvedProfile {
	m.mu.RLock()
	defer m.mu.RUnlock()

	if m.config == nil {
		return nil
	}
	return m.config.GetProfile(name)
}

// IsBrowserRunning checks if a browser is running for a profile.
func (m *Manager) IsBrowserRunning(profileName string) bool {
	m.mu.RLock()
	defer m.mu.RUnlock()

	profile := m.config.GetProfile(profileName)
	if profile == nil {
		return false
	}

	return IsChromeReachable(profile.CDPUrl, time.Second)
}

// ProfileStatus returns status info for a profile.
type ProfileStatus struct {
	Name      string `json:"name"`
	Driver    string `json:"driver"`
	CDPUrl    string `json:"cdp_url"`
	Running   bool   `json:"running"`
	Color     string `json:"color"`
	HasPages  bool   `json:"has_pages"`
	PageCount int    `json:"page_count"`
}

// GetProfileStatus returns status for a profile.
func (m *Manager) GetProfileStatus(profileName string) (*ProfileStatus, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	if m.config == nil {
		return nil, fmt.Errorf("browser manager not configured")
	}

	profile := m.config.GetProfile(profileName)
	if profile == nil {
		return nil, fmt.Errorf("unknown profile: %s", profileName)
	}

	status := &ProfileStatus{
		Name:    profile.Name,
		Driver:  profile.Driver,
		CDPUrl:  profile.CDPUrl,
		Running: IsChromeReachable(profile.CDPUrl, time.Second),
		Color:   profile.Color,
	}

	// Check for active session
	sessionsMu.RLock()
	if session, ok := sessions[profileName]; ok && !session.closed {
		pages := session.ListPages()
		status.HasPages = len(pages) > 0
		status.PageCount = len(pages)
	}
	sessionsMu.RUnlock()

	return status, nil
}

// GetAllProfileStatuses returns status for all profiles.
func (m *Manager) GetAllProfileStatuses() []*ProfileStatus {
	m.mu.RLock()
	defer m.mu.RUnlock()

	if m.config == nil {
		return nil
	}

	statuses := make([]*ProfileStatus, 0, len(m.config.Profiles))
	for name := range m.config.Profiles {
		status, err := m.GetProfileStatus(name)
		if err == nil {
			statuses = append(statuses, status)
		}
	}
	return statuses
}
