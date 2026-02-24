package webview

import (
	"fmt"
	"sync"
	"time"
)

// WindowHandle is the interface that a native webview window must satisfy.
// In desktop mode, Wails WebviewWindow implements this. In headless mode, nil.
type WindowHandle interface {
	SetURL(url string)
	ExecJS(js string)
	SetTitle(title string)
	Show()
	Hide()
	Focus()
	Close()
	SetSize(width, height int)
	Reload()
	Name() string
}

// WindowCreatorOptions configures a new native browser window.
type WindowCreatorOptions struct {
	Name   string
	Title  string
	URL    string
	Width  int
	Height int
}

// Window wraps a native webview window with metadata for the agent.
type Window struct {
	ID          string
	Title       string
	URL         string
	Owner       string // Session key that created this window (for cleanup)
	CreatedAt   time.Time
	Handle      WindowHandle
	Fingerprint *Fingerprint
}

// Manager manages native webview browser windows.
type Manager struct {
	mu sync.RWMutex

	creator     func(opts WindowCreatorOptions) WindowHandle
	windows     map[string]*Window
	owners      map[string]map[string]bool // owner -> set of window IDs
	callbackURL string // e.g. "http://localhost:27895/internal/webview/callback"
}

var (
	managerOnce sync.Once
	mgr         *Manager
)

// GetManager returns the singleton webview manager.
func GetManager() *Manager {
	managerOnce.Do(func() {
		mgr = &Manager{
			windows: make(map[string]*Window),
			owners:  make(map[string]map[string]bool),
		}
	})
	return mgr
}

// SetCreator installs the window creation callback (called from desktop.go).
func (m *Manager) SetCreator(fn func(opts WindowCreatorOptions) WindowHandle) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.creator = fn
}

// SetCallbackURL sets the base URL for JS → Go result callbacks.
func (m *Manager) SetCallbackURL(url string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.callbackURL = url
}

// CallbackURL returns the configured callback URL.
func (m *Manager) CallbackURL() string {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.callbackURL
}

// IsAvailable returns true if the native browser is available (desktop mode).
func (m *Manager) IsAvailable() bool {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return m.creator != nil
}

// CreateWindow creates a new native browser window.
// The owner parameter associates this window with a session key for cleanup.
// Pass empty string if no ownership tracking is needed.
func (m *Manager) CreateWindow(url, title, owner string) (*Window, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	if m.creator == nil {
		return nil, fmt.Errorf("native browser requires desktop mode (not available in headless)")
	}

	id := fmt.Sprintf("win-%d", time.Now().UnixNano())

	if title == "" {
		title = "Nebo Browser"
	}

	handle := m.creator(WindowCreatorOptions{
		Name:   id,
		Title:  title,
		URL:    url,
		Width:  1200,
		Height: 800,
	})

	if handle == nil {
		return nil, fmt.Errorf("failed to create window")
	}

	// Generate unique fingerprint for this window and inject it
	// before any page scripts run.
	fp := GenerateFingerprint()
	handle.ExecJS(fp.InjectJS())

	win := &Window{
		ID:          id,
		Title:       title,
		URL:         url,
		Owner:       owner,
		CreatedAt:   time.Now(),
		Handle:      handle,
		Fingerprint: fp,
	}

	m.windows[id] = win

	// Track ownership for cleanup
	if owner != "" {
		if m.owners[owner] == nil {
			m.owners[owner] = make(map[string]bool)
		}
		m.owners[owner][id] = true
	}

	return win, nil
}

// GetWindow returns a window by ID. If id is empty, returns the most recently created window.
func (m *Manager) GetWindow(id string) (*Window, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	if id == "" {
		// Return most recent window
		var latest *Window
		for _, w := range m.windows {
			if latest == nil || w.CreatedAt.After(latest.CreatedAt) {
				latest = w
			}
		}
		if latest == nil {
			return nil, fmt.Errorf("no windows open")
		}
		return latest, nil
	}

	win, ok := m.windows[id]
	if !ok {
		return nil, fmt.Errorf("window not found: %s", id)
	}
	return win, nil
}

// ListWindows returns all open windows.
func (m *Manager) ListWindows() []*Window {
	m.mu.RLock()
	defer m.mu.RUnlock()

	windows := make([]*Window, 0, len(m.windows))
	for _, w := range m.windows {
		windows = append(windows, w)
	}
	return windows
}

// CloseWindow closes and removes a window by ID.
func (m *Manager) CloseWindow(id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	win, ok := m.windows[id]
	if !ok {
		return fmt.Errorf("window not found: %s", id)
	}

	win.Handle.Close()
	delete(m.windows, id)

	// Clean up ownership tracking
	if win.Owner != "" {
		if ownerSet, ok := m.owners[win.Owner]; ok {
			delete(ownerSet, id)
			if len(ownerSet) == 0 {
				delete(m.owners, win.Owner)
			}
		}
	}

	return nil
}

// CloseWindowsByOwner closes all windows belonging to a specific owner (session key).
// Returns the number of windows closed.
func (m *Manager) CloseWindowsByOwner(owner string) int {
	m.mu.Lock()
	defer m.mu.Unlock()

	windowIDs, ok := m.owners[owner]
	if !ok {
		return 0
	}

	closed := 0
	for id := range windowIDs {
		if win, ok := m.windows[id]; ok {
			win.Handle.Close()
			delete(m.windows, id)
			closed++
		}
	}
	delete(m.owners, owner)
	return closed
}

// CloseAll closes all open windows.
func (m *Manager) CloseAll() {
	m.mu.Lock()
	defer m.mu.Unlock()

	for id, win := range m.windows {
		win.Handle.Close()
		delete(m.windows, id)
	}
}

// WindowCount returns the number of open windows.
func (m *Manager) WindowCount() int {
	m.mu.RLock()
	defer m.mu.RUnlock()
	return len(m.windows)
}
