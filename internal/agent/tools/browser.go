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
	Action    string `json:"action"`              // navigate, click, type, screenshot, text, html, evaluate, wait, snapshot, click_ref, type_ref
	URL       string `json:"url"`                 // For navigate action
	Selector  string `json:"selector"`            // CSS selector for element actions
	Text      string `json:"text"`                // Text to type or JS to evaluate
	Output    string `json:"output"`              // Output path for screenshot
	Timeout   int    `json:"timeout"`             // Action timeout in seconds (default: 30)
	Ref       int    `json:"ref"`                 // Element ref ID from snapshot (for click_ref, type_ref)
	Labels    bool   `json:"labels,omitempty"`    // Add labels to screenshot (for snapshot action)
	MaxLabels int    `json:"max_labels,omitempty"` // Limit labels (default: 100)
}

// RefBox holds ref ID and bounding coordinates for labeled screenshots
type RefBox struct {
	Ref    int     `json:"ref"`
	X      float64 `json:"x"`
	Y      float64 `json:"y"`
	Width  float64 `json:"w"`
	Height float64 `json:"h"`
}

// browserErrorHint maps error patterns to actionable hints
var browserErrorHints = map[string]string{
	"element not found":    "Run 'snapshot' first to refresh element refs",
	"node not found":       "Run 'snapshot' first to refresh element refs",
	"timeout":              "Page may be slow to load. Try increasing timeout or check network",
	"not interactable":     "Element may be hidden or disabled. Use snapshot to verify visibility",
	"ref not found":        "Invalid ref ID. Run snapshot to get current refs",
	"context canceled":     "Browser session may have closed. Reconnect with navigate",
	"context deadline":     "Operation timed out. Try increasing timeout parameter",
	"no clickable area":    "Element has no visible area. It may be hidden or zero-sized",
	"failed to get box":    "Element may have been removed from page. Run snapshot to refresh",
	"failed to resolve":    "Element reference is stale. Run snapshot to get fresh refs",
	"failed to focus":      "Element cannot receive focus. It may be disabled or hidden",
}

// wrapBrowserError wraps an error with an actionable hint
func wrapBrowserError(err error, action string) error {
	if err == nil {
		return nil
	}
	errStr := err.Error()
	for pattern, hint := range browserErrorHints {
		if strings.Contains(strings.ToLower(errStr), pattern) {
			return fmt.Errorf("%s failed: %w\n\nHint: %s", action, err, hint)
		}
	}
	return fmt.Errorf("%s failed: %w", action, err)
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
				"description": "Browser action: navigate (go to URL), click (click element), type (enter text), screenshot (capture page), text (get text content), html (get HTML), evaluate (run JS), wait (wait for element), snapshot (get accessibility tree with labeled screenshot), click_ref (click by ref ID), type_ref (type by ref ID)"
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
				"description": "File path to save screenshot (for 'screenshot' and 'snapshot' actions). If empty, returns base64."
			},
			"timeout": {
				"type": "integer",
				"description": "Action timeout in seconds. Default: 30",
				"default": 30
			},
			"ref": {
				"type": "integer",
				"description": "Element ref ID from snapshot (required for 'click_ref' and 'type_ref' actions)"
			},
			"labels": {
				"type": "boolean",
				"description": "Add visual labels to screenshot showing ref IDs (for 'snapshot' action). Default: false"
			},
			"max_labels": {
				"type": "integer",
				"description": "Maximum number of labels to display (for 'snapshot' with labels). Default: 100"
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
		result, err = t.snapshot(browserCtx, params.Labels, params.MaxLabels, params.Output)
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
		wrappedErr := wrapBrowserError(err, params.Action)
		return &ToolResult{
			Content: wrappedErr.Error(),
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
// If labels=true, also captures a screenshot with visual labels
func (t *BrowserTool) snapshot(ctx context.Context, labels bool, maxLabels int, outputPath string) (string, error) {
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

	// If labels requested, capture labeled screenshot
	if labels {
		if maxLabels <= 0 {
			maxLabels = 100
		}

		screenshotResult, err := t.captureLabeledScreenshot(ctx, maxLabels, outputPath)
		if err != nil {
			result.WriteString(fmt.Sprintf("\n[Labeled screenshot failed: %v]\n", err))
		} else {
			result.WriteString("\n")
			result.WriteString(screenshotResult)
		}
	}

	return result.String(), nil
}

// captureLabeledScreenshot injects visual labels and captures screenshot
func (t *BrowserTool) captureLabeledScreenshot(ctx context.Context, maxLabels int, outputPath string) (string, error) {
	// Collect bounding boxes for all refs
	boxes, err := t.collectRefBoxes(ctx, maxLabels)
	if err != nil {
		return "", fmt.Errorf("failed to collect ref boxes: %w", err)
	}

	if len(boxes) == 0 {
		return "No interactive elements found to label", nil
	}

	// Inject overlay elements
	boxesJSON, err := json.Marshal(boxes)
	if err != nil {
		return "", fmt.Errorf("failed to serialize boxes: %w", err)
	}

	overlayJS := fmt.Sprintf(`(function(boxes) {
    // Clean up existing overlays
    document.querySelectorAll('[data-nebo-overlay]').forEach(el => el.remove());

    // Create fixed root container
    const root = document.createElement('div');
    root.setAttribute('data-nebo-overlay', '1');
    root.style.cssText = 'position:fixed;left:0;top:0;width:100%%;height:100%%;z-index:999999;pointer-events:none;';

    const scrollX = window.scrollX || 0;
    const scrollY = window.scrollY || 0;
    const viewW = window.innerWidth;
    const viewH = window.innerHeight;

    boxes.forEach(b => {
        // Adjust for scroll position (boxes are in document coords)
        const x = b.x - scrollX;
        const y = b.y - scrollY;

        // Skip if outside viewport
        if (x + b.w < 0 || x > viewW || y + b.h < 0 || y > viewH) return;

        // Bounding box
        const border = document.createElement('div');
        border.style.cssText = 'position:absolute;left:' + x + 'px;top:' + y + 'px;width:' + b.w + 'px;height:' + b.h + 'px;border:2px solid #ff6600;box-sizing:border-box;';

        // Label (positioned above the box, clamped to viewport)
        const label = document.createElement('span');
        label.textContent = b.ref;
        const labelY = y < 18 ? 2 : -16;
        label.style.cssText = 'position:absolute;top:' + labelY + 'px;left:-2px;background:#ff6600;color:white;font:bold 11px monospace;padding:1px 4px;border-radius:2px;white-space:nowrap;';

        border.appendChild(label);
        root.appendChild(border);
    });

    document.documentElement.appendChild(root);
    return boxes.length;
})(%s)`, string(boxesJSON))

	var labelCount int
	err = chromedp.Run(ctx,
		chromedp.Evaluate(overlayJS, &labelCount),
	)
	if err != nil {
		return "", fmt.Errorf("failed to inject overlays: %w", err)
	}

	// Capture screenshot
	var buf []byte
	err = chromedp.Run(ctx,
		chromedp.FullScreenshot(&buf, 90),
	)
	if err != nil {
		// Clean up overlays before returning error
		_ = chromedp.Run(ctx, chromedp.Evaluate(`document.querySelectorAll('[data-nebo-overlay]').forEach(el => el.remove())`, nil))
		return "", fmt.Errorf("failed to capture screenshot: %w", err)
	}

	// Clean up overlays
	_ = chromedp.Run(ctx, chromedp.Evaluate(`document.querySelectorAll('[data-nebo-overlay]').forEach(el => el.remove())`, nil))

	// Return or save screenshot
	if outputPath == "" {
		b64 := base64.StdEncoding.EncodeToString(buf)
		return fmt.Sprintf("Labeled screenshot (%d elements, %d bytes)\ndata:image/png;base64,%s", labelCount, len(buf), b64), nil
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

	return fmt.Sprintf("Labeled screenshot saved to: %s (%d elements, %d bytes)", outputPath, labelCount, len(buf)), nil
}

// collectRefBoxes collects bounding boxes for all refs in the refMap
func (t *BrowserTool) collectRefBoxes(ctx context.Context, maxLabels int) ([]RefBox, error) {
	t.refMu.RLock()
	refs := make(map[int]cdp.BackendNodeID, len(t.refMap))
	for k, v := range t.refMap {
		refs[k] = v
	}
	t.refMu.RUnlock()

	var boxes []RefBox
	for ref, backendID := range refs {
		if len(boxes) >= maxLabels {
			break
		}

		// Convert backend node ID to frontend node ID
		var nodeIDs []cdp.NodeID
		err := chromedp.Run(ctx,
			chromedp.ActionFunc(func(ctx context.Context) error {
				var err error
				nodeIDs, err = dom.PushNodesByBackendIDsToFrontend([]cdp.BackendNodeID{backendID}).Do(ctx)
				return err
			}),
		)
		if err != nil || len(nodeIDs) == 0 {
			continue
		}

		// Get box model
		var box *dom.BoxModel
		err = chromedp.Run(ctx,
			chromedp.ActionFunc(func(ctx context.Context) error {
				var err error
				box, err = dom.GetBoxModel().WithNodeID(nodeIDs[0]).Do(ctx)
				return err
			}),
		)
		if err != nil || box == nil || box.Content == nil || len(box.Content) < 8 {
			continue
		}

		// Calculate bounds from content quad
		minX := min(box.Content[0], box.Content[2], box.Content[4], box.Content[6])
		maxX := max(box.Content[0], box.Content[2], box.Content[4], box.Content[6])
		minY := min(box.Content[1], box.Content[3], box.Content[5], box.Content[7])
		maxY := max(box.Content[1], box.Content[3], box.Content[5], box.Content[7])

		width := maxX - minX
		height := maxY - minY

		// Skip elements smaller than 10x10 pixels
		if width < 10 || height < 10 {
			continue
		}

		boxes = append(boxes, RefBox{
			Ref:    ref,
			X:      minX,
			Y:      minY,
			Width:  width,
			Height: height,
		})
	}

	return boxes, nil
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

			// Semantic format: role "name" [ref]
			if name != "" {
				result.WriteString(fmt.Sprintf("%s%s %q [%d]\n", indent, role, name, ref))
			} else {
				result.WriteString(fmt.Sprintf("%s%s [%d]\n", indent, role, ref))
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
