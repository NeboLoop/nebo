package tools

import (
	"fmt"
	"sort"
	"strings"
)

// RawElement is the platform-agnostic input from accessibility backends.
// Each platform populates this from its native accessibility API.
type RawElement struct {
	Role        string
	Title       string
	Description string
	Value       string
	Position    Rect
	Actionable  bool
	Children    []RawElement
}

// rolePrefixMap maps accessibility role names to short prefixes (matching Peekaboo convention).
var rolePrefixMap = map[string]string{
	"button":      "B",
	"textfield":   "T",
	"text field":  "T",
	"link":        "L",
	"checkbox":    "C",
	"check box":   "C",
	"menu":        "M",
	"menu item":   "M",
	"menuitem":    "M",
	"slider":      "S",
	"tab":         "A",
	"tab group":   "A",
	"radio":       "R",
	"radiobutton": "R",
	"radio button": "R",
	"popup":       "P",
	"pop up button": "P",
	"combobox":    "P",
	"combo box":   "P",
	"image":       "G",
	"static text": "X",
	"text":        "X",
	"toolbar":     "O",
	"tool bar":    "O",
	"list":        "I",
	"table":       "W",
	"scroll bar":  "Z",
	"scrollbar":   "Z",
	"group":       "U",
	"window":      "N",
	"select":      "P",
	"toggle":      "C",
}

// AssignElementIDs flattens the raw element tree, filters to actionable elements,
// assigns role-prefixed IDs (B1, T1, L1, etc.), and orders by screen position.
func AssignElementIDs(tree []RawElement) []*Element {
	var flat []RawElement
	flattenTree(tree, &flat)

	// Filter to actionable elements with valid bounds
	var actionable []RawElement
	for _, raw := range flat {
		if !raw.Actionable {
			continue
		}
		if raw.Position.Width <= 0 || raw.Position.Height <= 0 {
			continue
		}
		actionable = append(actionable, raw)
	}

	// Sort by screen position: top-to-bottom, left-to-right
	sort.Slice(actionable, func(i, j int) bool {
		a, b := actionable[i].Position, actionable[j].Position
		if a.Y != b.Y {
			// Group elements within 10px vertical band as same row
			if abs(a.Y-b.Y) > 10 {
				return a.Y < b.Y
			}
		}
		return a.X < b.X
	})

	// Assign role-prefixed IDs
	counters := make(map[string]int)
	elements := make([]*Element, 0, len(actionable))
	for _, raw := range actionable {
		prefix := rolePrefix(raw.Role)
		counters[prefix]++
		id := fmt.Sprintf("%s%d", prefix, counters[prefix])

		label := raw.Title
		if label == "" {
			label = raw.Description
		}

		elements = append(elements, &Element{
			ID:         id,
			Role:       raw.Role,
			Label:      label,
			Bounds:     raw.Position,
			Value:      raw.Value,
			Actionable: raw.Actionable,
		})
	}

	return elements
}

// FormatElementList formats a list of elements as a human-readable string.
func FormatElementList(elements []*Element) string {
	if len(elements) == 0 {
		return "No actionable elements found."
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Elements (%d actionable):\n", len(elements)))
	for _, elem := range elements {
		label := elem.Label
		if label == "" {
			label = "(no label)"
		}
		if len(label) > 40 {
			label = label[:37] + "..."
		}
		sb.WriteString(fmt.Sprintf("  %-4s %-12s %-42s (%d, %d)\n",
			elem.ID, elem.Role, fmt.Sprintf("%q", label),
			elem.Bounds.X, elem.Bounds.Y))
	}
	return sb.String()
}

func flattenTree(nodes []RawElement, out *[]RawElement) {
	for _, node := range nodes {
		*out = append(*out, node)
		if len(node.Children) > 0 {
			flattenTree(node.Children, out)
		}
	}
}

func rolePrefix(role string) string {
	normalized := strings.ToLower(strings.TrimSpace(role))
	if prefix, ok := rolePrefixMap[normalized]; ok {
		return prefix
	}
	// Default: first uppercase letter of the role
	if len(normalized) > 0 {
		return strings.ToUpper(normalized[:1])
	}
	return "E"
}

func abs(x int) int {
	if x < 0 {
		return -x
	}
	return x
}
