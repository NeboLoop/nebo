package webview

import (
	"sync"
	"testing"
)

// mockHandle implements WindowHandle for testing.
type mockHandle struct {
	name    string
	url     string
	title   string
	jsLog   []string
	mu      sync.Mutex
	closed  bool
	visible bool
	focused bool
}

func newMockHandle(name string) *mockHandle {
	return &mockHandle{name: name, visible: true}
}

func (m *mockHandle) SetURL(url string)          { m.mu.Lock(); m.url = url; m.mu.Unlock() }
func (m *mockHandle) ExecJS(js string)            { m.mu.Lock(); m.jsLog = append(m.jsLog, js); m.mu.Unlock() }
func (m *mockHandle) SetTitle(title string)       { m.mu.Lock(); m.title = title; m.mu.Unlock() }
func (m *mockHandle) Show()                       { m.mu.Lock(); m.visible = true; m.mu.Unlock() }
func (m *mockHandle) Hide()                       { m.mu.Lock(); m.visible = false; m.mu.Unlock() }
func (m *mockHandle) Focus()                      { m.mu.Lock(); m.focused = true; m.mu.Unlock() }
func (m *mockHandle) Close()                      { m.mu.Lock(); m.closed = true; m.mu.Unlock() }
func (m *mockHandle) SetSize(width, height int)   {}
func (m *mockHandle) Reload()                     {}
func (m *mockHandle) Name() string                { return m.name }

func TestManagerCreateWindow(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}

	// No creator → error
	_, err := m.CreateWindow("https://example.com", "Test", "")
	if err == nil {
		t.Fatal("expected error when creator is nil")
	}

	// Install creator
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	// Create window
	win, err := m.CreateWindow("https://example.com", "Test Window", "")
	if err != nil {
		t.Fatalf("CreateWindow failed: %v", err)
	}
	if win.ID == "" {
		t.Error("expected non-empty window ID")
	}
	if win.URL != "https://example.com" {
		t.Errorf("expected URL https://example.com, got %s", win.URL)
	}
	if win.Title != "Test Window" {
		t.Errorf("expected title 'Test Window', got %s", win.Title)
	}
	if win.Handle == nil {
		t.Error("expected non-nil handle")
	}
}

func TestManagerGetWindow(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	// No windows → error
	_, err := m.GetWindow("")
	if err == nil {
		t.Fatal("expected error when no windows")
	}

	// Create two windows
	win1, _ := m.CreateWindow("https://one.com", "One", "")
	win2, _ := m.CreateWindow("https://two.com", "Two", "")

	// Get by ID
	got, err := m.GetWindow(win1.ID)
	if err != nil {
		t.Fatalf("GetWindow by ID failed: %v", err)
	}
	if got.ID != win1.ID {
		t.Errorf("expected %s, got %s", win1.ID, got.ID)
	}

	// Get most recent (empty ID)
	got, err = m.GetWindow("")
	if err != nil {
		t.Fatalf("GetWindow most recent failed: %v", err)
	}
	if got.ID != win2.ID {
		t.Errorf("expected most recent %s, got %s", win2.ID, got.ID)
	}

	// Get by invalid ID → error
	_, err = m.GetWindow("nonexistent")
	if err == nil {
		t.Fatal("expected error for invalid window ID")
	}
}

func TestManagerListAndClose(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	win1, _ := m.CreateWindow("https://one.com", "One", "")
	m.CreateWindow("https://two.com", "Two", "")

	if m.WindowCount() != 2 {
		t.Fatalf("expected 2 windows, got %d", m.WindowCount())
	}

	windows := m.ListWindows()
	if len(windows) != 2 {
		t.Fatalf("expected 2 windows in list, got %d", len(windows))
	}

	// Close one
	if err := m.CloseWindow(win1.ID); err != nil {
		t.Fatalf("CloseWindow failed: %v", err)
	}
	if m.WindowCount() != 1 {
		t.Fatalf("expected 1 window after close, got %d", m.WindowCount())
	}

	// Verify handle was closed
	h := win1.Handle.(*mockHandle)
	if !h.closed {
		t.Error("expected handle to be closed")
	}

	// Close nonexistent → error
	if err := m.CloseWindow("nonexistent"); err == nil {
		t.Error("expected error closing nonexistent window")
	}

	// Close all
	m.CloseAll()
	if m.WindowCount() != 0 {
		t.Fatalf("expected 0 windows after CloseAll, got %d", m.WindowCount())
	}
}

func TestManagerIsAvailable(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}

	if m.IsAvailable() {
		t.Error("expected not available when no creator")
	}

	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	if !m.IsAvailable() {
		t.Error("expected available after setting creator")
	}
}

func TestManagerCloseWindowsByOwner(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	// Create windows with different owners
	w1, _ := m.CreateWindow("https://one.com", "One", "session-a")
	w2, _ := m.CreateWindow("https://two.com", "Two", "session-a")
	w3, _ := m.CreateWindow("https://three.com", "Three", "session-b")
	m.CreateWindow("https://four.com", "Four", "") // no owner

	if m.WindowCount() != 4 {
		t.Fatalf("expected 4 windows, got %d", m.WindowCount())
	}

	// Close session-a windows
	closed := m.CloseWindowsByOwner("session-a")
	if closed != 2 {
		t.Fatalf("expected 2 closed, got %d", closed)
	}
	if m.WindowCount() != 2 {
		t.Fatalf("expected 2 remaining, got %d", m.WindowCount())
	}

	// Verify handles were closed
	if !w1.Handle.(*mockHandle).closed {
		t.Error("w1 handle should be closed")
	}
	if !w2.Handle.(*mockHandle).closed {
		t.Error("w2 handle should be closed")
	}
	if w3.Handle.(*mockHandle).closed {
		t.Error("w3 handle should NOT be closed")
	}

	// Close nonexistent owner → 0
	if m.CloseWindowsByOwner("nonexistent") != 0 {
		t.Error("expected 0 for nonexistent owner")
	}

	// Close session-b
	closed = m.CloseWindowsByOwner("session-b")
	if closed != 1 {
		t.Fatalf("expected 1 closed, got %d", closed)
	}
	if m.WindowCount() != 1 {
		t.Fatalf("expected 1 remaining (unowned), got %d", m.WindowCount())
	}
}

func TestManagerDefaultTitle(t *testing.T) {
	m := &Manager{windows: make(map[string]*Window), owners: make(map[string]map[string]bool)}
	m.SetCreator(func(opts WindowCreatorOptions) WindowHandle {
		return newMockHandle(opts.Name)
	})

	win, err := m.CreateWindow("https://example.com", "", "")
	if err != nil {
		t.Fatalf("CreateWindow failed: %v", err)
	}
	if win.Title != "Nebo Browser" {
		t.Errorf("expected default title 'Nebo Browser', got %s", win.Title)
	}
}
