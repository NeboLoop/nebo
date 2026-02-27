package browser

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/playwright-community/playwright-go"
)

// Session manages a browser connection and its pages.
type Session struct {
	mu sync.RWMutex

	profile  *ResolvedProfile
	pw       *playwright.Playwright
	browser  playwright.Browser
	contexts map[string]playwright.BrowserContext

	// pages maps targetID to Page wrapper
	pages map[string]*Page

	// activeTargetID is the currently focused page's target ID
	activeTargetID string

	closed bool
}

// Page wraps a Playwright page with state tracking.
type Page struct {
	mu sync.RWMutex

	targetID string
	page     playwright.Page
	session  *Session
	state    *PageState
	refs     *RefCache
	closed   bool
}

// PageState tracks page state for debugging and context.
type PageState struct {
	URL             string           `json:"url"`
	Title           string           `json:"title"`
	ConsoleMessages []ConsoleMessage `json:"console_messages,omitempty"`
	Errors          []PageError      `json:"errors,omitempty"`
}

// ConsoleMessage represents a browser console message.
type ConsoleMessage struct {
	Type      string    `json:"type"`
	Text      string    `json:"text"`
	Timestamp time.Time `json:"timestamp"`
}

// PageError represents a page error.
type PageError struct {
	Message   string    `json:"message"`
	Timestamp time.Time `json:"timestamp"`
}

// RefCache caches element refs for stable cross-call references.
type RefCache struct {
	mu sync.RWMutex

	refs       map[string]*RoleRef
	bySelector map[string]string
	nextID     int
	createdAt  time.Time
}

// RoleRef is a stable reference to a DOM element.
type RoleRef struct {
	Ref      string `json:"ref"`
	Role     string `json:"role"`
	Name     string `json:"name"`
	Nth      int    `json:"nth,omitempty"`
	Selector string `json:"selector"`
}

var (
	// Global session manager
	sessionsMu sync.RWMutex
	sessions   = make(map[string]*Session)

	// Playwright instance (singleton)
	pwOnce     sync.Once
	pwInstance *playwright.Playwright
	pwErr      error
)

// getPlaywright returns the singleton Playwright instance.
func getPlaywright() (*playwright.Playwright, error) {
	pwOnce.Do(func() {
		// Install browsers if needed
		if err := playwright.Install(); err != nil {
			pwErr = fmt.Errorf("failed to install playwright browsers: %w", err)
			return
		}

		pw, err := playwright.Run()
		if err != nil {
			pwErr = fmt.Errorf("failed to start playwright: %w", err)
			return
		}
		pwInstance = pw
	})

	return pwInstance, pwErr
}

// GetOrCreateSession gets or creates a session for a profile.
func GetOrCreateSession(ctx context.Context, profile *ResolvedProfile) (*Session, error) {
	sessionsMu.Lock()
	defer sessionsMu.Unlock()

	if session, ok := sessions[profile.Name]; ok && !session.closed {
		// Verify the browser connection is still alive
		if session.browser != nil && session.browser.IsConnected() {
			return session, nil
		}
		// Connection died (e.g., server restart, extension reconnect) — clean up stale session
		session.closed = true
		delete(sessions, profile.Name)
	}

	session, err := newSession(ctx, profile)
	if err != nil {
		return nil, err
	}

	sessions[profile.Name] = session
	return session, nil
}

// GetSessionIfExists returns the session for a profile if it exists, or nil.
func GetSessionIfExists(profileName string) *Session {
	sessionsMu.RLock()
	defer sessionsMu.RUnlock()

	session, ok := sessions[profileName]
	if !ok || session.closed {
		return nil
	}
	return session
}

// CloseSession closes a session by profile name.
func CloseSession(profileName string) error {
	sessionsMu.Lock()
	defer sessionsMu.Unlock()

	session, ok := sessions[profileName]
	if !ok {
		return nil
	}

	delete(sessions, profileName)
	return session.Close()
}

// CloseAllSessions closes all active sessions.
func CloseAllSessions() {
	sessionsMu.Lock()
	defer sessionsMu.Unlock()

	for name, session := range sessions {
		_ = session.Close()
		delete(sessions, name)
	}
}

func newSession(ctx context.Context, profile *ResolvedProfile) (*Session, error) {
	pw, err := getPlaywright()
	if err != nil {
		return nil, err
	}

	session := &Session{
		profile:  profile,
		pw:       pw,
		contexts: make(map[string]playwright.BrowserContext),
		pages:    make(map[string]*Page),
	}

	// Connect to browser based on driver type
	switch profile.Driver {
	case DriverExtension:
		// Connect to extension relay via CDP with auth headers
		headers := GetRelayAuthHeaders(profile.CDPUrl)
		opts := playwright.BrowserTypeConnectOverCDPOptions{
			Headers: headers,
		}
		browser, err := pw.Chromium.ConnectOverCDP(profile.CDPUrl, opts)
		if err != nil {
			return nil, fmt.Errorf("failed to connect to CDP at %s: %w", profile.CDPUrl, err)
		}
		session.browser = browser

	case DriverNebo:
		// Connect to Nebo-managed Chrome via CDP
		// The Chrome should already be running (launched by manager)
		browser, err := pw.Chromium.ConnectOverCDP(profile.CDPUrl)
		if err != nil {
			return nil, fmt.Errorf("failed to connect to nebo browser at %s: %w", profile.CDPUrl, err)
		}
		session.browser = browser

	default:
		return nil, fmt.Errorf("unknown driver: %s", profile.Driver)
	}

	// Index existing pages
	for _, ctx := range session.browser.Contexts() {
		for _, page := range ctx.Pages() {
			targetID := getTargetID(page)
			session.pages[targetID] = &Page{
				targetID: targetID,
				page:     page,
				session:  session,
				state:    &PageState{},
				refs:     newRefCache(),
			}
			if session.activeTargetID == "" {
				session.activeTargetID = targetID
			}
		}
	}

	return session, nil
}

// Close closes the session and all its resources.
func (s *Session) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.closed {
		return nil
	}
	s.closed = true

	// Close all pages
	for _, page := range s.pages {
		page.closed = true
	}

	// Don't close the browser - it may be the user's browser
	// Just disconnect
	if s.browser != nil {
		// Playwright ConnectOverCDP doesn't have a Disconnect method,
		// but closing the browser object will close the connection
		// For user's browser (extension mode), we don't want to close it
		if s.profile.Driver == DriverNebo {
			// For managed browser, we can close it
			_ = s.browser.Close()
		}
	}

	return nil
}

// GetPage returns a page by target ID, or the active page if targetID is empty.
func (s *Session) GetPage(targetID string) (*Page, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()

	if s.closed {
		return nil, fmt.Errorf("session is closed")
	}

	if targetID == "" {
		targetID = s.activeTargetID
	}

	if targetID == "" {
		// No active page, try to get any page
		for _, page := range s.pages {
			if !page.closed {
				return page, nil
			}
		}
		return nil, fmt.Errorf("no pages available")
	}

	page, ok := s.pages[targetID]
	if !ok || page.closed {
		return nil, fmt.Errorf("page not found: %s", targetID)
	}

	return page, nil
}

// ListPages returns all open pages.
func (s *Session) ListPages() []*Page {
	s.mu.RLock()
	defer s.mu.RUnlock()

	var pages []*Page
	for _, page := range s.pages {
		if !page.closed {
			pages = append(pages, page)
		}
	}
	return pages
}

// NewPage creates a new page in the session.
func (s *Session) NewPage(ctx context.Context) (*Page, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.closed {
		return nil, fmt.Errorf("session is closed")
	}

	// Get or create a context
	var browserCtx playwright.BrowserContext
	if len(s.browser.Contexts()) > 0 {
		browserCtx = s.browser.Contexts()[0]
	} else {
		var err error
		browserCtx, err = s.browser.NewContext()
		if err != nil {
			return nil, fmt.Errorf("failed to create browser context: %w", err)
		}
	}

	pwPage, err := browserCtx.NewPage()
	if err != nil {
		return nil, fmt.Errorf("failed to create page: %w", err)
	}

	targetID := getTargetID(pwPage)
	page := &Page{
		targetID: targetID,
		page:     pwPage,
		session:  s,
		state:    &PageState{},
		refs:     newRefCache(),
	}

	s.pages[targetID] = page
	s.activeTargetID = targetID

	// Set up event listeners
	setupPageListeners(page)

	return page, nil
}

// SetActivePage sets the active page.
func (s *Session) SetActivePage(targetID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if _, ok := s.pages[targetID]; !ok {
		return fmt.Errorf("page not found: %s", targetID)
	}

	s.activeTargetID = targetID
	return nil
}

// Page methods

// PlaywrightPage returns the underlying Playwright page.
func (p *Page) PlaywrightPage() playwright.Page {
	return p.page
}

// TargetID returns the page's target ID.
func (p *Page) TargetID() string {
	return p.targetID
}

// State returns the page state.
func (p *Page) State() *PageState {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.state
}

// UpdateState updates the page state from the current page.
func (p *Page) UpdateState() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.closed {
		return fmt.Errorf("page is closed")
	}

	url := p.page.URL()
	title, _ := p.page.Title()

	p.state.URL = url
	p.state.Title = title

	return nil
}

// Refs returns the ref cache.
func (p *Page) Refs() *RefCache {
	return p.refs
}

// Helper functions

func getTargetID(_ playwright.Page) string {
	// Use a stable UUID — URL-based IDs broke after navigation because
	// the URL changes but the page stays indexed under the old key.
	return fmt.Sprintf("page-%s", uuid.New().String()[:8])
}

func newRefCache() *RefCache {
	return &RefCache{
		refs:       make(map[string]*RoleRef),
		bySelector: make(map[string]string),
		nextID:     1,
		createdAt:  time.Now(),
	}
}

func setupPageListeners(page *Page) {
	pwPage := page.page

	// Console messages
	pwPage.OnConsole(func(msg playwright.ConsoleMessage) {
		page.mu.Lock()
		defer page.mu.Unlock()

		page.state.ConsoleMessages = append(page.state.ConsoleMessages, ConsoleMessage{
			Type:      msg.Type(),
			Text:      msg.Text(),
			Timestamp: time.Now(),
		})

		// Keep only last 100 messages
		if len(page.state.ConsoleMessages) > 100 {
			page.state.ConsoleMessages = page.state.ConsoleMessages[len(page.state.ConsoleMessages)-100:]
		}
	})

	// Page errors
	pwPage.OnPageError(func(err error) {
		page.mu.Lock()
		defer page.mu.Unlock()

		page.state.Errors = append(page.state.Errors, PageError{
			Message:   err.Error(),
			Timestamp: time.Now(),
		})

		// Keep only last 50 errors
		if len(page.state.Errors) > 50 {
			page.state.Errors = page.state.Errors[len(page.state.Errors)-50:]
		}
	})

	// Page close
	pwPage.OnClose(func(p playwright.Page) {
		page.mu.Lock()
		defer page.mu.Unlock()
		page.closed = true
	})
}

// RefCache methods

// Get returns a ref by ID.
func (c *RefCache) Get(refID string) *RoleRef {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.refs[refID]
}

// GetOrCreate returns an existing ref or creates a new one.
func (c *RefCache) GetOrCreate(role, name string, nth int) *RoleRef {
	c.mu.Lock()
	defer c.mu.Unlock()

	selector := buildSelector(role, name, nth)
	if existingID, ok := c.bySelector[selector]; ok {
		return c.refs[existingID]
	}

	refID := fmt.Sprintf("e%d", c.nextID)
	c.nextID++

	ref := &RoleRef{
		Ref:      refID,
		Role:     role,
		Name:     name,
		Nth:      nth,
		Selector: selector,
	}

	c.refs[refID] = ref
	c.bySelector[selector] = refID

	return ref
}

// Clear clears the ref cache.
func (c *RefCache) Clear() {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.refs = make(map[string]*RoleRef)
	c.bySelector = make(map[string]string)
	c.nextID = 1
}

// All returns all refs.
func (c *RefCache) All() []*RoleRef {
	c.mu.RLock()
	defer c.mu.RUnlock()

	refs := make([]*RoleRef, 0, len(c.refs))
	for _, ref := range c.refs {
		refs = append(refs, ref)
	}
	return refs
}

func buildSelector(role, name string, nth int) string {
	selector := fmt.Sprintf("role=%s", role)
	if name != "" {
		selector += fmt.Sprintf("[name=%q]", name)
	}
	if nth > 1 {
		selector += fmt.Sprintf(" >> nth=%d", nth-1) // Playwright nth is 0-based
	}
	return selector
}
