package tools

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/chromedp/cdproto/accessibility"
	"github.com/chromedp/cdproto/cdp"
	"github.com/chromedp/cdproto/dom"
	"github.com/chromedp/chromedp"
)

// BrowserTool provides browser automation via Chrome DevTools Protocol
type BrowserTool struct {
	allocCtx context.Context
	cancel   context.CancelFunc
	timeout  time.Duration

	// Reference map for semantic snapshots: ref ID -> backend node ID
	refMu  sync.RWMutex
	refMap map[int]cdp.BackendNodeID
}

type browserInput struct {
	Action   string `json:"action"`   // navigate, click, type, screenshot, text, html, evaluate, wait, snapshot, click_ref, type_ref
	URL      string `json:"url"`      // For navigate action
	Selector string `json:"selector"` // CSS selector for element actions
	Text     string `json:"text"`     // Text to type or JS to evaluate
	Output   string `json:"output"`   // Output path for screenshot
	Timeout  int    `json:"timeout"`  // Action timeout in seconds (default: 30)
	Ref      int    `json:"ref"`      // Element ref ID from snapshot (for click_ref, type_ref)
}

// BrowserConfig configures the browser tool
type BrowserConfig struct {
	Headless bool          // Run browser headlessly (default: true)
	Timeout  time.Duration // Default timeout (default: 30s)
}

func NewBrowserTool(cfg BrowserConfig) *BrowserTool {
	if cfg.Timeout == 0 {
		cfg.Timeout = 30 * time.Second
	}

	opts := append(chromedp.DefaultExecAllocatorOptions[:],
		chromedp.Flag("headless", cfg.Headless),
		chromedp.Flag("disable-gpu", true),
		chromedp.Flag("no-sandbox", true),
		chromedp.Flag("disable-dev-shm-usage", true),
	)

	allocCtx, cancel := chromedp.NewExecAllocator(context.Background(), opts...)

	return &BrowserTool{
		allocCtx: allocCtx,
		cancel:   cancel,
		timeout:  cfg.Timeout,
		refMap:   make(map[int]cdp.BackendNodeID),
	}
}

func (t *BrowserTool) Close() {
	if t.cancel != nil {
		t.cancel()
	}
}

func (t *BrowserTool) Name() string {
	return "browser"
}

func (t *BrowserTool) Description() string {
	return "Automate browser interactions via Chrome DevTools Protocol. Navigate to URLs, click elements, type text, take screenshots, extract content, and run JavaScript."
}

func (t *BrowserTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["navigate", "click", "type", "screenshot", "text", "html", "evaluate", "wait", "snapshot", "click_ref", "type_ref"],
				"description": "Browser action: navigate (go to URL), click (click element), type (enter text), screenshot (capture page), text (get text content), html (get HTML), evaluate (run JS), wait (wait for element), snapshot (get accessibility tree), click_ref (click by ref ID), type_ref (type by ref ID)"
			},
			"url": {
				"type": "string",
				"description": "URL to navigate to (required for 'navigate' action)"
			},
			"selector": {
				"type": "string",
				"description": "CSS selector for element (required for click, type, text, html, wait actions)"
			},
			"text": {
				"type": "string",
				"description": "Text to type (for 'type' and 'type_ref' actions) or JavaScript code (for 'evaluate' action)"
			},
			"output": {
				"type": "string",
				"description": "File path to save screenshot (for 'screenshot' action). If empty, returns base64."
			},
			"timeout": {
				"type": "integer",
				"description": "Action timeout in seconds. Default: 30",
				"default": 30
			},
			"ref": {
				"type": "integer",
				"description": "Element ref ID from snapshot (required for 'click_ref' and 'type_ref' actions)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *BrowserTool) RequiresApproval() bool {
	return true // Browser automation can be dangerous
}

func (t *BrowserTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params browserInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Failed to parse input: %v", err),
			IsError: true,
		}, nil
	}

	timeout := t.timeout
	if params.Timeout > 0 {
		timeout = time.Duration(params.Timeout) * time.Second
	}

	// Create browser context
	browserCtx, cancel := chromedp.NewContext(t.allocCtx)
	defer cancel()

	// Add timeout
	browserCtx, cancel = context.WithTimeout(browserCtx, timeout)
	defer cancel()

	var result string
	var err error

	switch params.Action {
	case "navigate":
		result, err = t.navigate(browserCtx, params.URL)
	case "click":
		result, err = t.click(browserCtx, params.Selector)
	case "type":
		result, err = t.typeText(browserCtx, params.Selector, params.Text)
	case "screenshot":
		result, err = t.screenshot(browserCtx, params.Output)
	case "text":
		result, err = t.getText(browserCtx, params.Selector)
	case "html":
		result, err = t.getHTML(browserCtx, params.Selector)
	case "evaluate":
		result, err = t.evaluate(browserCtx, params.Text)
	case "wait":
		result, err = t.waitFor(browserCtx, params.Selector)
	case "snapshot":
		result, err = t.snapshot(browserCtx)
	case "click_ref":
		result, err = t.clickRef(browserCtx, params.Ref)
	case "type_ref":
		result, err = t.typeRef(browserCtx, params.Ref, params.Text)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s", params.Action),
			IsError: true,
		}, nil
	}

	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Browser action failed: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: result,
		IsError: false,
	}, nil
}

func (t *BrowserTool) navigate(ctx context.Context, url string) (string, error) {
	if url == "" {
		return "", fmt.Errorf("URL is required for navigate action")
	}

	var title string
	err := chromedp.Run(ctx,
		chromedp.Navigate(url),
		chromedp.WaitReady("body"),
		chromedp.Title(&title),
	)
	if err != nil {
		return "", err
	}

	return fmt.Sprintf("Navigated to: %s\nPage title: %s", url, title), nil
}

func (t *BrowserTool) click(ctx context.Context, selector string) (string, error) {
	if selector == "" {
		return "", fmt.Errorf("selector is required for click action")
	}

	err := chromedp.Run(ctx,
		chromedp.WaitVisible(selector),
		chromedp.Click(selector),
	)
	if err != nil {
		return "", err
	}

	return fmt.Sprintf("Clicked element: %s", selector), nil
}

func (t *BrowserTool) typeText(ctx context.Context, selector, text string) (string, error) {
	if selector == "" {
		return "", fmt.Errorf("selector is required for type action")
	}
	if text == "" {
		return "", fmt.Errorf("text is required for type action")
	}

	err := chromedp.Run(ctx,
		chromedp.WaitVisible(selector),
		chromedp.Clear(selector),
		chromedp.SendKeys(selector, text),
	)
	if err != nil {
		return "", err
	}

	return fmt.Sprintf("Typed text into element: %s", selector), nil
}

func (t *BrowserTool) screenshot(ctx context.Context, outputPath string) (string, error) {
	var buf []byte
	err := chromedp.Run(ctx,
		chromedp.FullScreenshot(&buf, 90), // 90% quality
	)
	if err != nil {
		return "", err
	}

	if outputPath == "" {
		// Return as base64
		b64 := base64.StdEncoding.EncodeToString(buf)
		return fmt.Sprintf("Screenshot captured (%d bytes)\ndata:image/png;base64,%s", len(buf), b64), nil
	}

	// Expand ~ to home directory
	if strings.HasPrefix(outputPath, "~/") {
		homeDir, _ := os.UserHomeDir()
		outputPath = filepath.Join(homeDir, outputPath[2:])
	}

	// Ensure directory exists
	dir := filepath.Dir(outputPath)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", fmt.Errorf("failed to create directory: %w", err)
	}

	if err := os.WriteFile(outputPath, buf, 0644); err != nil {
		return "", fmt.Errorf("failed to save screenshot: %w", err)
	}

	return fmt.Sprintf("Screenshot saved to: %s (%d bytes)", outputPath, len(buf)), nil
}

func (t *BrowserTool) getText(ctx context.Context, selector string) (string, error) {
	if selector == "" {
		// Get all visible text from body
		selector = "body"
	}

	var text string
	err := chromedp.Run(ctx,
		chromedp.WaitVisible(selector),
		chromedp.Text(selector, &text),
	)
	if err != nil {
		return "", err
	}

	// Truncate if too long
	if len(text) > 10000 {
		text = text[:10000] + "\n... (truncated)"
	}

	return text, nil
}

func (t *BrowserTool) getHTML(ctx context.Context, selector string) (string, error) {
	if selector == "" {
		selector = "html"
	}

	var html string
	err := chromedp.Run(ctx,
		chromedp.WaitReady(selector),
		chromedp.ActionFunc(func(ctx context.Context) error {
			node, err := dom.GetDocument().Do(ctx)
			if err != nil {
				return err
			}

			if selector == "html" {
				html, err = dom.GetOuterHTML().WithNodeID(node.NodeID).Do(ctx)
				return err
			}

			// Find element by selector
			var nodes []*cdp.Node
			if err := chromedp.Nodes(selector, &nodes).Do(ctx); err != nil {
				return err
			}
			if len(nodes) == 0 {
				return fmt.Errorf("no element found for selector: %s", selector)
			}
			html, err = dom.GetOuterHTML().WithNodeID(nodes[0].NodeID).Do(ctx)
			return err
		}),
	)
	if err != nil {
		return "", err
	}

	// Truncate if too long
	if len(html) > 50000 {
		html = html[:50000] + "\n... (truncated)"
	}

	return html, nil
}

func (t *BrowserTool) evaluate(ctx context.Context, js string) (string, error) {
	if js == "" {
		return "", fmt.Errorf("JavaScript code is required for evaluate action")
	}

	var result any
	err := chromedp.Run(ctx,
		chromedp.Evaluate(js, &result),
	)
	if err != nil {
		return "", err
	}

	// Convert result to string
	switch v := result.(type) {
	case string:
		return v, nil
	case nil:
		return "undefined", nil
	default:
		jsonResult, err := json.MarshalIndent(result, "", "  ")
		if err != nil {
			return fmt.Sprintf("%v", result), nil
		}
		return string(jsonResult), nil
	}
}

func (t *BrowserTool) waitFor(ctx context.Context, selector string) (string, error) {
	if selector == "" {
		return "", fmt.Errorf("selector is required for wait action")
	}

	start := time.Now()
	err := chromedp.Run(ctx,
		chromedp.WaitVisible(selector),
	)
	if err != nil {
		return "", err
	}

	return fmt.Sprintf("Element '%s' appeared after %v", selector, time.Since(start).Round(time.Millisecond)), nil
}

// snapshot captures the accessibility tree and returns a text representation
func (t *BrowserTool) snapshot(ctx context.Context) (string, error) {
	var nodes []*accessibility.Node

	err := chromedp.Run(ctx,
		chromedp.ActionFunc(func(ctx context.Context) error {
			tree, err := accessibility.GetFullAXTree().Do(ctx)
			if err != nil {
				return err
			}
			nodes = tree
			return nil
		}),
	)
	if err != nil {
		return "", fmt.Errorf("failed to get accessibility tree: %w", err)
	}

	// Clear and rebuild the ref map
	t.refMu.Lock()
	t.refMap = make(map[int]cdp.BackendNodeID)
	t.refMu.Unlock()

	// Build a text representation of the accessibility tree
	var result strings.Builder
	result.WriteString("Page Accessibility Snapshot\n")
	result.WriteString("===========================\n\n")

	refCounter := 1
	t.formatAXNodes(&result, nodes, 0, &refCounter)

	return result.String(), nil
}

// formatAXNodes recursively formats accessibility nodes
func (t *BrowserTool) formatAXNodes(result *strings.Builder, nodes []*accessibility.Node, depth int, refCounter *int) {
	for _, node := range nodes {
		if node == nil {
			continue
		}

		// Skip ignored nodes
		if node.Ignored {
			continue
		}

		// Get role and name
		role := ""
		if node.Role != nil {
			role = fmt.Sprintf("%v", node.Role.Value)
		}

		name := ""
		if node.Name != nil && node.Name.Value != nil {
			name = fmt.Sprintf("%v", node.Name.Value)
		}

		// Skip empty generic containers
		if role == "generic" && name == "" {
			continue
		}

		indent := strings.Repeat("  ", depth)

		// Check if this is an interactive element
		isInteractive := t.isInteractiveRole(role)

		if isInteractive && node.BackendDOMNodeID != 0 {
			// Store ref and display it
			ref := *refCounter
			*refCounter++

			t.refMu.Lock()
			t.refMap[ref] = node.BackendDOMNodeID
			t.refMu.Unlock()

			if name != "" {
				result.WriteString(fmt.Sprintf("%s[ref=%d] %s: %q\n", indent, ref, role, name))
			} else {
				result.WriteString(fmt.Sprintf("%s[ref=%d] %s\n", indent, ref, role))
			}
		} else {
			if name != "" {
				result.WriteString(fmt.Sprintf("%s%s: %q\n", indent, role, name))
			} else if role != "" && role != "none" && role != "generic" {
				result.WriteString(fmt.Sprintf("%s%s\n", indent, role))
			}
		}

		// Process children
		if len(node.ChildIDs) > 0 {
			// Find child nodes by ID
			childNodes := t.findNodesByIDs(nodes, node.ChildIDs)
			t.formatAXNodes(result, childNodes, depth+1, refCounter)
		}
	}
}

// findNodesByIDs finds nodes by their IDs
func (t *BrowserTool) findNodesByIDs(allNodes []*accessibility.Node, ids []accessibility.NodeID) []*accessibility.Node {
	idSet := make(map[accessibility.NodeID]bool)
	for _, id := range ids {
		idSet[id] = true
	}

	var result []*accessibility.Node
	for _, node := range allNodes {
		if node != nil && idSet[node.NodeID] {
			result = append(result, node)
		}
	}
	return result
}

// isInteractiveRole checks if a role represents an interactive element
func (t *BrowserTool) isInteractiveRole(role string) bool {
	interactiveRoles := map[string]bool{
		"button":       true,
		"link":         true,
		"textbox":      true,
		"checkbox":     true,
		"radio":        true,
		"combobox":     true,
		"listbox":      true,
		"option":       true,
		"menuitem":     true,
		"menu":         true,
		"menubar":      true,
		"tab":          true,
		"tablist":      true,
		"slider":       true,
		"spinbutton":   true,
		"searchbox":    true,
		"switch":       true,
		"treeitem":     true,
		"gridcell":     true,
		"columnheader": true,
		"rowheader":    true,
	}
	return interactiveRoles[role]
}

// clickRef clicks an element by its ref ID from the snapshot
func (t *BrowserTool) clickRef(ctx context.Context, ref int) (string, error) {
	if ref <= 0 {
		return "", fmt.Errorf("ref is required for click_ref action")
	}

	t.refMu.RLock()
	backendNodeID, ok := t.refMap[ref]
	t.refMu.RUnlock()

	if !ok {
		return "", fmt.Errorf("ref %d not found - run 'snapshot' action first to get valid refs", ref)
	}

	err := chromedp.Run(ctx,
		chromedp.ActionFunc(func(ctx context.Context) error {
			// Resolve the node
			nodeIDs, err := dom.PushNodesByBackendIDsToFrontend([]cdp.BackendNodeID{backendNodeID}).Do(ctx)
			if err != nil {
				return fmt.Errorf("failed to resolve node: %w", err)
			}
			if len(nodeIDs) == 0 {
				return fmt.Errorf("node not found")
			}

			// Get the node's box model for clicking
			box, err := dom.GetBoxModel().WithNodeID(nodeIDs[0]).Do(ctx)
			if err != nil {
				return fmt.Errorf("failed to get box model: %w", err)
			}

			// Click the center of the element
			if box.Content != nil && len(box.Content) >= 4 {
				x := (box.Content[0] + box.Content[2] + box.Content[4] + box.Content[6]) / 4
				y := (box.Content[1] + box.Content[3] + box.Content[5] + box.Content[7]) / 4

				return chromedp.MouseClickXY(x, y).Do(ctx)
			}

			return fmt.Errorf("element has no clickable area")
		}),
	)

	if err != nil {
		return "", err
	}

	return fmt.Sprintf("Clicked element with ref=%d", ref), nil
}

// typeRef types text into an element by its ref ID from the snapshot
func (t *BrowserTool) typeRef(ctx context.Context, ref int, text string) (string, error) {
	if ref <= 0 {
		return "", fmt.Errorf("ref is required for type_ref action")
	}
	if text == "" {
		return "", fmt.Errorf("text is required for type_ref action")
	}

	t.refMu.RLock()
	backendNodeID, ok := t.refMap[ref]
	t.refMu.RUnlock()

	if !ok {
		return "", fmt.Errorf("ref %d not found - run 'snapshot' action first to get valid refs", ref)
	}

	err := chromedp.Run(ctx,
		chromedp.ActionFunc(func(ctx context.Context) error {
			// Resolve the node
			nodeIDs, err := dom.PushNodesByBackendIDsToFrontend([]cdp.BackendNodeID{backendNodeID}).Do(ctx)
			if err != nil {
				return fmt.Errorf("failed to resolve node: %w", err)
			}
			if len(nodeIDs) == 0 {
				return fmt.Errorf("node not found")
			}

			// Focus the element
			if err := dom.Focus().WithNodeID(nodeIDs[0]).Do(ctx); err != nil {
				return fmt.Errorf("failed to focus element: %w", err)
			}

			// Clear and type
			// First, select all existing text
			if err := chromedp.KeyEvent("a", chromedp.KeyModifiers(1)).Do(ctx); err != nil { // Ctrl+A
				return err
			}

			// Type the new text (which replaces the selection)
			return chromedp.KeyEvent(text).Do(ctx)
		}),
	)

	if err != nil {
		return "", err
	}

	return fmt.Sprintf("Typed text into element with ref=%d", ref), nil
}
