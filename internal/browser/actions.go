package browser

import (
	"context"
	"encoding/base64"
	"fmt"
	"time"

	"github.com/playwright-community/playwright-go"
)

// ActionResult is the result of a browser action.
type ActionResult struct {
	Success  bool   `json:"success"`
	Message  string `json:"message,omitempty"`
	Snapshot string `json:"snapshot,omitempty"`
	URL      string `json:"url,omitempty"`
	Title    string `json:"title,omitempty"`
}

// NavigateOptions configures navigation.
type NavigateOptions struct {
	URL       string
	WaitUntil string // "load", "domcontentloaded", "networkidle"
	Timeout   time.Duration
}

// Navigate navigates to a URL.
func (p *Page) Navigate(ctx context.Context, opts NavigateOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	waitUntil := playwright.WaitUntilStateLoad
	switch opts.WaitUntil {
	case "domcontentloaded":
		waitUntil = playwright.WaitUntilStateDomcontentloaded
	case "networkidle":
		waitUntil = playwright.WaitUntilStateNetworkidle
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	_, err := p.page.Goto(opts.URL, playwright.PageGotoOptions{
		WaitUntil: waitUntil,
		Timeout:   playwright.Float(float64(timeout.Milliseconds())),
	})
	if err != nil {
		return nil, fmt.Errorf("navigation failed: %w", err)
	}

	// Clear refs on navigation
	p.refs.Clear()

	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Navigated to %s", opts.URL),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// ClickOptions configures click actions.
type ClickOptions struct {
	Ref      string // Element ref (e1, e2, etc.)
	Selector string // Direct selector (if no ref)
	Button   string // "left", "right", "middle"
	Count    int    // Click count (1=click, 2=double-click)
	Timeout  time.Duration
}

// Click clicks an element.
func (p *Page) Click(ctx context.Context, opts ClickOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	selector, err := p.resolveSelector(opts.Ref, opts.Selector)
	if err != nil {
		return nil, err
	}

	button := playwright.MouseButtonLeft
	switch opts.Button {
	case "right":
		button = playwright.MouseButtonRight
	case "middle":
		button = playwright.MouseButtonMiddle
	}

	clickCount := opts.Count
	if clickCount == 0 {
		clickCount = 1
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	locator := p.page.Locator(selector)
	err = locator.Click(playwright.LocatorClickOptions{
		Button:     button,
		ClickCount: playwright.Int(clickCount),
		Timeout:    playwright.Float(float64(timeout.Milliseconds())),
	})
	if err != nil {
		return nil, fmt.Errorf("click failed: %w", err)
	}

	// Wait a bit for any navigation/updates
	time.Sleep(100 * time.Millisecond)
	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Clicked %s", selector),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// TypeOptions configures type actions.
type TypeOptions struct {
	Ref      string
	Selector string
	Text     string
	Delay    time.Duration // Delay between keystrokes
	Timeout  time.Duration
}

// Type types text into an element.
func (p *Page) Type(ctx context.Context, opts TypeOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	selector, err := p.resolveSelector(opts.Ref, opts.Selector)
	if err != nil {
		return nil, err
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	locator := p.page.Locator(selector)

	typeOpts := playwright.LocatorTypeOptions{
		Timeout: playwright.Float(float64(timeout.Milliseconds())),
	}
	if opts.Delay > 0 {
		typeOpts.Delay = playwright.Float(float64(opts.Delay.Milliseconds()))
	}

	err = locator.Type(opts.Text, typeOpts)
	if err != nil {
		return nil, fmt.Errorf("type failed: %w", err)
	}

	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Typed into %s", selector),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// FillOptions configures fill actions.
type FillOptions struct {
	Ref      string
	Selector string
	Value    string
	Timeout  time.Duration
}

// Fill fills an input element (clears first, then types).
func (p *Page) Fill(ctx context.Context, opts FillOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	selector, err := p.resolveSelector(opts.Ref, opts.Selector)
	if err != nil {
		return nil, err
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	locator := p.page.Locator(selector)
	err = locator.Fill(opts.Value, playwright.LocatorFillOptions{
		Timeout: playwright.Float(float64(timeout.Milliseconds())),
	})
	if err != nil {
		return nil, fmt.Errorf("fill failed: %w", err)
	}

	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Filled %s", selector),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// SelectOptions configures select actions.
type SelectOptions struct {
	Ref      string
	Selector string
	Values   []string // Values to select
	Timeout  time.Duration
}

// Select selects options in a <select> element.
func (p *Page) Select(ctx context.Context, opts SelectOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	selector, err := p.resolveSelector(opts.Ref, opts.Selector)
	if err != nil {
		return nil, err
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	locator := p.page.Locator(selector)
	_, err = locator.SelectOption(playwright.SelectOptionValues{Values: &opts.Values}, playwright.LocatorSelectOptionOptions{
		Timeout: playwright.Float(float64(timeout.Milliseconds())),
	})
	if err != nil {
		return nil, fmt.Errorf("select failed: %w", err)
	}

	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Selected in %s", selector),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// HoverOptions configures hover actions.
type HoverOptions struct {
	Ref      string
	Selector string
	Timeout  time.Duration
}

// Hover hovers over an element.
func (p *Page) Hover(ctx context.Context, opts HoverOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	selector, err := p.resolveSelector(opts.Ref, opts.Selector)
	if err != nil {
		return nil, err
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	locator := p.page.Locator(selector)
	err = locator.Hover(playwright.LocatorHoverOptions{
		Timeout: playwright.Float(float64(timeout.Milliseconds())),
	})
	if err != nil {
		return nil, fmt.Errorf("hover failed: %w", err)
	}

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Hovered over %s", selector),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// PressOptions configures key press actions.
type PressOptions struct {
	Key     string // Key to press (Enter, Tab, Escape, etc.)
	Timeout time.Duration
}

// Press presses a keyboard key.
func (p *Page) Press(ctx context.Context, opts PressOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	err := p.page.Keyboard().Press(opts.Key)
	if err != nil {
		return nil, fmt.Errorf("press failed: %w", err)
	}

	time.Sleep(100 * time.Millisecond)
	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Pressed %s", opts.Key),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// ScrollOptions configures scroll actions.
type ScrollOptions struct {
	Ref       string
	Selector  string
	Direction string // "up", "down", "left", "right"
	Amount    int    // Pixels to scroll
}

// Scroll scrolls the page or an element.
func (p *Page) Scroll(ctx context.Context, opts ScrollOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	amount := opts.Amount
	if amount == 0 {
		amount = 500
	}

	var deltaX, deltaY int
	switch opts.Direction {
	case "up":
		deltaY = -amount
	case "down":
		deltaY = amount
	case "left":
		deltaX = -amount
	case "right":
		deltaX = amount
	default:
		deltaY = amount // Default to scroll down
	}

	if opts.Ref != "" || opts.Selector != "" {
		selector, err := p.resolveSelector(opts.Ref, opts.Selector)
		if err != nil {
			return nil, err
		}
		locator := p.page.Locator(selector)
		err = locator.ScrollIntoViewIfNeeded()
		if err != nil {
			return nil, fmt.Errorf("scroll into view failed: %w", err)
		}
	} else {
		// Scroll the page
		_, err := p.page.Evaluate(fmt.Sprintf("window.scrollBy(%d, %d)", deltaX, deltaY))
		if err != nil {
			return nil, fmt.Errorf("scroll failed: %w", err)
		}
	}

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Scrolled %s by %d", opts.Direction, amount),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// WaitOptions configures wait actions.
type WaitOptions struct {
	Ref      string
	Selector string
	State    string // "visible", "hidden", "attached", "detached"
	Timeout  time.Duration
}

// Wait waits for an element to reach a state.
func (p *Page) Wait(ctx context.Context, opts WaitOptions) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	selector, err := p.resolveSelector(opts.Ref, opts.Selector)
	if err != nil {
		return nil, err
	}

	timeout := opts.Timeout
	if timeout == 0 {
		timeout = 30 * time.Second
	}

	state := playwright.WaitForSelectorStateVisible
	switch opts.State {
	case "hidden":
		state = playwright.WaitForSelectorStateHidden
	case "attached":
		state = playwright.WaitForSelectorStateAttached
	case "detached":
		state = playwright.WaitForSelectorStateDetached
	}

	_, err = p.page.WaitForSelector(selector, playwright.PageWaitForSelectorOptions{
		State:   state,
		Timeout: playwright.Float(float64(timeout.Milliseconds())),
	})
	if err != nil {
		return nil, fmt.Errorf("wait failed: %w", err)
	}

	return &ActionResult{
		Success: true,
		Message: fmt.Sprintf("Element %s is %s", selector, opts.State),
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// ScreenshotOptions configures screenshot actions.
type ScreenshotOptions struct {
	FullPage bool
	Ref      string
	Selector string
}

// Screenshot takes a screenshot.
func (p *Page) Screenshot(ctx context.Context, opts ScreenshotOptions) (string, error) {
	if p.closed {
		return "", fmt.Errorf("page is closed")
	}

	var data []byte
	var err error

	if opts.Ref != "" || opts.Selector != "" {
		selector, serr := p.resolveSelector(opts.Ref, opts.Selector)
		if serr != nil {
			return "", serr
		}
		locator := p.page.Locator(selector)
		data, err = locator.Screenshot()
	} else {
		data, err = p.page.Screenshot(playwright.PageScreenshotOptions{
			FullPage: playwright.Bool(opts.FullPage),
		})
	}

	if err != nil {
		return "", fmt.Errorf("screenshot failed: %w", err)
	}

	return base64.StdEncoding.EncodeToString(data), nil
}

// GoBack navigates back.
func (p *Page) GoBack(ctx context.Context) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	_, err := p.page.GoBack()
	if err != nil {
		return nil, fmt.Errorf("go back failed: %w", err)
	}

	p.refs.Clear()
	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: "Navigated back",
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// GoForward navigates forward.
func (p *Page) GoForward(ctx context.Context) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	_, err := p.page.GoForward()
	if err != nil {
		return nil, fmt.Errorf("go forward failed: %w", err)
	}

	p.refs.Clear()
	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: "Navigated forward",
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// Reload reloads the page.
func (p *Page) Reload(ctx context.Context) (*ActionResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	_, err := p.page.Reload()
	if err != nil {
		return nil, fmt.Errorf("reload failed: %w", err)
	}

	p.refs.Clear()
	_ = p.UpdateState()

	return &ActionResult{
		Success: true,
		Message: "Page reloaded",
		URL:     p.state.URL,
		Title:   p.state.Title,
	}, nil
}

// resolveSelector resolves a ref or selector to a Playwright selector.
func (p *Page) resolveSelector(ref, selector string) (string, error) {
	if ref != "" {
		roleRef := p.refs.Get(ref)
		if roleRef == nil {
			return "", fmt.Errorf("ref not found: %s", ref)
		}
		return roleRef.Selector, nil
	}
	if selector == "" {
		return "", fmt.Errorf("ref or selector required")
	}
	return selector, nil
}
