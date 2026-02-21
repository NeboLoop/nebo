//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
)

// MenubarTool interacts with macOS menu bar items.
// Uses AppleScript System Events to list and click menu bar items and status items.
type MenubarTool struct{}

// NewMenubarTool creates a new menubar tool
func NewMenubarTool() *MenubarTool {
	return &MenubarTool{}
}

func (t *MenubarTool) Name() string {
	return "menubar"
}

func (t *MenubarTool) Description() string {
	return "Interact with the macOS menu bar: list menu bar items, list menu items for an app, click menu items, and access status bar (system tray) items."
}

func (t *MenubarTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: list (menu bar items), menus (list menus for app), click (click menu item), status (list status bar items), click_status (click status item)",
				"enum": ["list", "menus", "click", "status", "click_status"]
			},
			"app": {
				"type": "string",
				"description": "Application name (for menus/click actions). Defaults to frontmost app."
			},
			"menu": {
				"type": "string",
				"description": "Menu name (e.g., 'File', 'Edit')"
			},
			"item": {
				"type": "string",
				"description": "Menu item name to click (e.g., 'Save', 'Copy')"
			},
			"index": {
				"type": "integer",
				"description": "Status bar item index (1-based) for click_status"
			}
		},
		"required": ["action"]
	}`)
}

func (t *MenubarTool) RequiresApproval() bool {
	return true
}

type menubarInput struct {
	Action string `json:"action"`
	App    string `json:"app"`
	Menu   string `json:"menu"`
	Item   string `json:"item"`
	Index  int    `json:"index"`
}

func (t *MenubarTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in menubarInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	var result string
	var err error

	switch in.Action {
	case "list":
		result, err = t.listMenuBar(in.App)
	case "menus":
		result, err = t.listMenus(in.App, in.Menu)
	case "click":
		result, err = t.clickMenuItem(in.App, in.Menu, in.Item)
	case "status":
		result, err = t.listStatusItems()
	case "click_status":
		result, err = t.clickStatusItem(in.Index)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}

	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Action failed: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: result}, nil
}

func (t *MenubarTool) getAppProcess(app string) string {
	if app == "" {
		return "first process whose frontmost is true"
	}
	return fmt.Sprintf("process %q", app)
}

func (t *MenubarTool) listMenuBar(app string) (string, error) {
	script := fmt.Sprintf(`
		tell application "System Events"
			set targetProc to %s
			set appName to name of targetProc
			set menuNames to ""

			set menuBar to menu bar 1 of targetProc
			set menuItems to every menu bar item of menuBar

			repeat with m in menuItems
				set mName to name of m
				if mName is not "" then
					set menuNames to menuNames & mName & return
				end if
			end repeat

			return "Menu bar for " & appName & ":" & return & menuNames
		end tell
	`, t.getAppProcess(app))

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to list menu bar: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) listMenus(app, menu string) (string, error) {
	if menu == "" {
		return "", fmt.Errorf("menu name is required (e.g., 'File', 'Edit')")
	}

	script := fmt.Sprintf(`
		tell application "System Events"
			set targetProc to %s
			set appName to name of targetProc

			set menuBar to menu bar 1 of targetProc
			set targetMenu to menu 1 of menu bar item %q of menuBar
			set menuItemList to ""

			set items to every menu item of targetMenu
			repeat with mi in items
				set miName to name of mi
				if miName is not "" then
					-- Check if enabled
					set isEnabled to enabled of mi
					set enabledStr to ""
					if not isEnabled then
						set enabledStr to " (disabled)"
					end if

					-- Check for shortcut
					set shortcut to ""
					try
						set accel to value of attribute "AXMenuItemCmdChar" of mi
						if accel is not "" then
							set shortcut to " [⌘" & accel & "]"
						end if
					end try

					set menuItemList to menuItemList & miName & shortcut & enabledStr & return
				else
					set menuItemList to menuItemList & "---" & return
				end if
			end repeat

			return appName & " > " & %q & ":" & return & menuItemList
		end tell
	`, t.getAppProcess(app), menu, menu)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to list menu items: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) clickMenuItem(app, menu, item string) (string, error) {
	if menu == "" || item == "" {
		return "", fmt.Errorf("menu and item names are required")
	}

	script := fmt.Sprintf(`
		tell application "System Events"
			set targetProc to %s
			set appName to name of targetProc

			-- Make app frontmost
			set frontmost of targetProc to true
			delay 0.2

			click menu item %q of menu 1 of menu bar item %q of menu bar 1 of targetProc
			return "Clicked " & appName & " > " & %q & " > " & %q
		end tell
	`, t.getAppProcess(app), item, menu, menu, item)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to click menu item: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) listStatusItems() (string, error) {
	script := `
		tell application "System Events"
			set statusItems to ""
			set menuExtras to menu bar items of menu bar 2 of process "SystemUIServer"

			set idx to 1
			repeat with item in menuExtras
				set itemName to ""
				try
					set itemName to description of item
				end try
				if itemName is "" then
					try
						set itemName to title of item
					end try
				end if
				if itemName is "" then
					set itemName to "(unnamed)"
				end if
				set statusItems to statusItems & idx & ". " & itemName & return
				set idx to idx + 1
			end repeat

			if statusItems is "" then
				return "No status bar items found"
			end if
			return "Status Bar Items:" & return & statusItems
		end tell
	`

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to list status items: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func (t *MenubarTool) clickStatusItem(index int) (string, error) {
	if index < 1 {
		return "", fmt.Errorf("index is required (1-based, use status action to see available items)")
	}

	script := fmt.Sprintf(`
		tell application "System Events"
			set menuExtras to menu bar items of menu bar 2 of process "SystemUIServer"

			if %d > (count of menuExtras) then
				return "Index %d out of range (only " & (count of menuExtras) & " status items)"
			end if

			set targetItem to item %d of menuExtras
			click targetItem

			set itemName to ""
			try
				set itemName to description of targetItem
			end try
			if itemName is "" then
				set itemName to "(item " & %d & ")"
			end if

			return "Clicked status bar item: " & itemName
		end tell
	`, index, index, index, index)

	out, err := exec.Command("osascript", "-e", script).Output()
	if err != nil {
		return "", fmt.Errorf("failed to click status item: %v", err)
	}

	return strings.TrimSpace(string(out)), nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewMenubarTool(),
		Platforms: []string{PlatformDarwin},
		Category:  "automation",
	})
}
