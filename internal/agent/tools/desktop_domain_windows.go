//go:build windows

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewDesktopDomainTool(DesktopDomainOpts{
			Input:    NewDesktopTool(),
			UI:       NewAccessibilityTool(),
			Window:   NewWindowTool(),
			Shortcut: NewShortcutsTool(),
		}),
		Platforms: []string{PlatformWindows},
		Category:  "desktop",
	})
}
