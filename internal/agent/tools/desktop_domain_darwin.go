//go:build darwin && !ios

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewDesktopDomainTool(DesktopDomainOpts{
			Input:    NewDesktopTool(),
			UI:       NewAccessibilityTool(),
			Window:   NewWindowTool(),
			Menu:     NewMenubarTool(),
			Dialog:   NewDialogTool(),
			Space:    NewSpacesTool(),
			Shortcut: NewShortcutsTool(),
		}),
		Platforms: []string{PlatformDarwin},
		Category:  "desktop",
	})
}
