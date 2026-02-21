//go:build darwin && !ios

package tools

import (
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// getUITreeWithBounds retrieves the accessibility tree for an app with element positions.
// Returns RawElement slice suitable for AssignElementIDs.
func getUITreeWithBounds(app string, windowBounds Rect) []RawElement {
	if app == "" {
		return nil
	}

	script := fmt.Sprintf(`
on getElements(elem, depth, maxD, offsetX, offsetY)
	if depth > maxD then return ""
	set result to ""
	try
		set elemRole to role of elem
		set elemTitle to ""
		set elemDesc to ""
		set elemValue to ""
		set elemPos to {0, 0}
		set elemSize to {0, 0}
		set isActionable to false
		try
			set elemTitle to title of elem
		end try
		try
			set elemDesc to description of elem
		end try
		try
			set elemValue to value of elem as text
		end try
		try
			set elemPos to position of elem
		end try
		try
			set elemSize to size of elem
		end try
		try
			set actionNames to name of actions of elem
			if actionNames is not {} then set isActionable to true
		end try

		set px to (item 1 of elemPos) as integer
		set py to (item 2 of elemPos) as integer
		set sw to (item 1 of elemSize) as integer
		set sh to (item 2 of elemSize) as integer

		-- Only include elements with valid size
		if sw > 0 and sh > 0 then
			set actionStr to "0"
			if isActionable then set actionStr to "1"
			set line to elemRole & "|" & elemTitle & "|" & elemDesc & "|" & elemValue & "|" & (px as text) & "|" & (py as text) & "|" & (sw as text) & "|" & (sh as text) & "|" & actionStr
			set result to result & line & linefeed
		end if

		try
			set children to UI elements of elem
			repeat with child in children
				set result to result & my getElements(child, depth + 1, maxD, offsetX, offsetY)
			end repeat
		end try
	end try
	return result
end getElements

tell application "System Events"
	tell process "%s"
		set output to ""
		repeat with win in windows
			set output to output & my getElements(win, 0, 5, 0, 0)
		end repeat
		return output
	end tell
end tell`, app)

	out, err := exec.Command("osascript", "-e", script).CombinedOutput()
	if err != nil {
		fmt.Printf("[screenshot:see] AppleScript accessibility error: %v\n", err)
		return nil
	}

	return parseAppleScriptElements(strings.TrimSpace(string(out)))
}

// parseAppleScriptElements parses the pipe-delimited output from the AppleScript.
func parseAppleScriptElements(output string) []RawElement {
	if output == "" {
		return nil
	}

	var elements []RawElement
	for _, line := range strings.Split(output, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		parts := strings.SplitN(line, "|", 9)
		if len(parts) < 9 {
			continue
		}

		x, _ := strconv.Atoi(parts[4])
		y, _ := strconv.Atoi(parts[5])
		w, _ := strconv.Atoi(parts[6])
		h, _ := strconv.Atoi(parts[7])
		actionable := parts[8] == "1"

		// Map AppleScript role names to simpler names
		role := normalizeRole(parts[0])

		elements = append(elements, RawElement{
			Role:        role,
			Title:       parts[1],
			Description: parts[2],
			Value:       parts[3],
			Position:    Rect{X: x, Y: y, Width: w, Height: h},
			Actionable:  actionable,
		})
	}

	return elements
}

// normalizeRole converts AppleScript role names to simpler names.
func normalizeRole(role string) string {
	roleMap := map[string]string{
		"AXButton":       "button",
		"AXTextField":    "textfield",
		"AXTextArea":     "textfield",
		"AXStaticText":   "static text",
		"AXCheckBox":     "checkbox",
		"AXRadioButton":  "radio",
		"AXPopUpButton":  "popup",
		"AXMenuButton":   "menu",
		"AXMenu":         "menu",
		"AXMenuItem":     "menu item",
		"AXSlider":       "slider",
		"AXTabGroup":     "tab",
		"AXLink":         "link",
		"AXImage":        "image",
		"AXToolbar":      "toolbar",
		"AXList":         "list",
		"AXTable":        "table",
		"AXScrollBar":    "scrollbar",
		"AXGroup":        "group",
		"AXWindow":       "window",
		"AXComboBox":     "combobox",
		"AXDisclosureTriangle": "button",
	}
	if mapped, ok := roleMap[role]; ok {
		return mapped
	}
	// Strip "AX" prefix if present
	if strings.HasPrefix(role, "AX") {
		return strings.ToLower(role[2:])
	}
	return strings.ToLower(role)
}
