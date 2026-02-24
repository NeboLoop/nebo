package browser

import (
	"context"
	"fmt"
	"regexp"
	"strings"
)

// SnapshotOptions configures aria snapshot generation.
type SnapshotOptions struct {
	MaxChars      int  // Max characters in snapshot (0 = unlimited)
	IncludeRefs   bool // Include element refs (e1, e2, etc.)
	IncludeHidden bool // Include hidden elements
}

// Snapshot generates an aria snapshot of the page with element refs.
// Stable refs enable consistent cross-call element references.
func (p *Page) Snapshot(ctx context.Context, opts SnapshotOptions) (string, error) {
	if p.closed {
		return "", fmt.Errorf("page is closed")
	}

	// Clear old refs before generating new snapshot
	p.refs.Clear()

	// Use Playwright's ariaSnapshot
	snapshot, err := p.page.Locator("body").AriaSnapshot()
	if err != nil {
		return "", fmt.Errorf("aria snapshot failed: %w", err)
	}

	if !opts.IncludeRefs {
		return maybeTrauncate(snapshot, opts.MaxChars), nil
	}

	// Parse snapshot and add refs
	annotated := p.annotateSnapshot(snapshot)

	return maybeTrauncate(annotated, opts.MaxChars), nil
}

// annotateSnapshot adds element refs (e1, e2, etc.) to the snapshot.
func (p *Page) annotateSnapshot(snapshot string) string {
	lines := strings.Split(snapshot, "\n")
	var result []string

	// Regex to match role lines like "- button "Submit""
	rolePattern := regexp.MustCompile(`^(\s*)-\s+(\w+)(?:\s+"([^"]*)")?(.*)$`)

	for _, line := range lines {
		match := rolePattern.FindStringSubmatch(line)
		if match == nil {
			result = append(result, line)
			continue
		}

		indent := match[1]
		role := match[2]
		name := match[3]
		rest := match[4]

		// Skip certain roles that aren't interactive
		if !isInteractiveRole(role) {
			result = append(result, line)
			continue
		}

		// Get or create ref for this element
		ref := p.refs.GetOrCreate(role, name, 1)

		// Annotate the line with the ref
		annotated := fmt.Sprintf("%s- %s", indent, role)
		if name != "" {
			annotated += fmt.Sprintf(" %q", name)
		}
		annotated += fmt.Sprintf(" [%s]", ref.Ref)
		if rest != "" {
			annotated += rest
		}

		result = append(result, annotated)
	}

	return strings.Join(result, "\n")
}

// isInteractiveRole returns true if the role represents an interactive element.
func isInteractiveRole(role string) bool {
	interactive := map[string]bool{
		"button":       true,
		"link":         true,
		"textbox":      true,
		"checkbox":     true,
		"radio":        true,
		"combobox":     true,
		"listbox":      true,
		"option":       true,
		"menuitem":     true,
		"menuitemcheckbox": true,
		"menuitemradio": true,
		"tab":          true,
		"slider":       true,
		"spinbutton":   true,
		"switch":       true,
		"searchbox":    true,
		"textarea":     true, // Not standard ARIA but common
	}
	return interactive[role]
}

func maybeTrauncate(s string, maxChars int) string {
	if maxChars <= 0 || len(s) <= maxChars {
		return s
	}
	return s[:maxChars] + "\n... (truncated)"
}

// GetText gets visible text from an element or the page.
func (p *Page) GetText(ctx context.Context, ref, selector string) (string, error) {
	if p.closed {
		return "", fmt.Errorf("page is closed")
	}

	resolvedSelector, err := p.resolveSelector(ref, selector)
	if err != nil {
		// If no selector, get all visible text
		text, err := p.page.Locator("body").InnerText()
		if err != nil {
			return "", fmt.Errorf("get text failed: %w", err)
		}
		return text, nil
	}

	locator := p.page.Locator(resolvedSelector)
	text, err := locator.InnerText()
	if err != nil {
		return "", fmt.Errorf("get text failed: %w", err)
	}

	return text, nil
}

// GetAttribute gets an attribute from an element.
func (p *Page) GetAttribute(ctx context.Context, ref, selector, attr string) (string, error) {
	if p.closed {
		return "", fmt.Errorf("page is closed")
	}

	resolvedSelector, err := p.resolveSelector(ref, selector)
	if err != nil {
		return "", err
	}

	locator := p.page.Locator(resolvedSelector)
	value, err := locator.GetAttribute(attr)
	if err != nil {
		return "", fmt.Errorf("get attribute failed: %w", err)
	}

	return value, nil
}

// GetValue gets the value of an input element.
func (p *Page) GetValue(ctx context.Context, ref, selector string) (string, error) {
	if p.closed {
		return "", fmt.Errorf("page is closed")
	}

	resolvedSelector, err := p.resolveSelector(ref, selector)
	if err != nil {
		return "", err
	}

	locator := p.page.Locator(resolvedSelector)
	value, err := locator.InputValue()
	if err != nil {
		return "", fmt.Errorf("get value failed: %w", err)
	}

	return value, nil
}

// IsVisible checks if an element is visible.
func (p *Page) IsVisible(ctx context.Context, ref, selector string) (bool, error) {
	if p.closed {
		return false, fmt.Errorf("page is closed")
	}

	resolvedSelector, err := p.resolveSelector(ref, selector)
	if err != nil {
		return false, err
	}

	locator := p.page.Locator(resolvedSelector)
	visible, err := locator.IsVisible()
	if err != nil {
		return false, fmt.Errorf("is visible check failed: %w", err)
	}

	return visible, nil
}

// IsEnabled checks if an element is enabled.
func (p *Page) IsEnabled(ctx context.Context, ref, selector string) (bool, error) {
	if p.closed {
		return false, fmt.Errorf("page is closed")
	}

	resolvedSelector, err := p.resolveSelector(ref, selector)
	if err != nil {
		return false, err
	}

	locator := p.page.Locator(resolvedSelector)
	enabled, err := locator.IsEnabled()
	if err != nil {
		return false, fmt.Errorf("is enabled check failed: %w", err)
	}

	return enabled, nil
}

// IsChecked checks if a checkbox/radio is checked.
func (p *Page) IsChecked(ctx context.Context, ref, selector string) (bool, error) {
	if p.closed {
		return false, fmt.Errorf("page is closed")
	}

	resolvedSelector, err := p.resolveSelector(ref, selector)
	if err != nil {
		return false, err
	}

	locator := p.page.Locator(resolvedSelector)
	checked, err := locator.IsChecked()
	if err != nil {
		return false, fmt.Errorf("is checked failed: %w", err)
	}

	return checked, nil
}

// Count counts elements matching a selector.
func (p *Page) Count(ctx context.Context, selector string) (int, error) {
	if p.closed {
		return 0, fmt.Errorf("page is closed")
	}

	locator := p.page.Locator(selector)
	count, err := locator.Count()
	if err != nil {
		return 0, fmt.Errorf("count failed: %w", err)
	}

	return count, nil
}

// Evaluate evaluates JavaScript in the page context.
func (p *Page) Evaluate(ctx context.Context, script string) (any, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	result, err := p.page.Evaluate(script)
	if err != nil {
		return nil, fmt.Errorf("evaluate failed: %w", err)
	}

	return result, nil
}

// GetSource returns the full page HTML source.
func (p *Page) GetSource(ctx context.Context) (string, error) {
	if p.closed {
		return "", fmt.Errorf("page is closed")
	}

	content, err := p.page.Content()
	if err != nil {
		return "", fmt.Errorf("get source failed: %w", err)
	}

	return content, nil
}

// ConsoleResult holds console messages and page errors returned by GetConsoleMessages.
type ConsoleResult struct {
	Messages []ConsoleMessage `json:"messages,omitempty"`
	Errors   []PageError      `json:"errors,omitempty"`
}

// GetConsoleMessages returns captured console messages and page errors.
// If level is non-empty, only messages matching that level are returned.
// If clear is true, the captured messages and errors are cleared after reading.
func (p *Page) GetConsoleMessages(ctx context.Context, level string, clear bool) (*ConsoleResult, error) {
	if p.closed {
		return nil, fmt.Errorf("page is closed")
	}

	p.mu.Lock()
	defer p.mu.Unlock()

	// Copy messages with optional level filter
	var messages []ConsoleMessage
	for _, msg := range p.state.ConsoleMessages {
		if level == "" || strings.EqualFold(msg.Type, level) {
			messages = append(messages, ConsoleMessage{
				Type:      msg.Type,
				Text:      msg.Text,
				Timestamp: msg.Timestamp,
			})
		}
	}

	// Copy errors
	errors := make([]PageError, len(p.state.Errors))
	copy(errors, p.state.Errors)

	// Clear after read if requested
	if clear {
		p.state.ConsoleMessages = nil
		p.state.Errors = nil
	}

	return &ConsoleResult{
		Messages: messages,
		Errors:   errors,
	}, nil
}
