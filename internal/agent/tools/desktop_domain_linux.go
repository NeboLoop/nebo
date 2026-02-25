//go:build linux

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewDesktopDomainTool(DesktopDomainOpts{
			Input:    NewDesktopTool(),
			UI:       NewAccessibilityTool(),
			Window:   NewWindowTool(),
			Shortcut: NewShortcutsTool(),
		}),
		Platforms: []string{PlatformLinux},
		Category:  "desktop",
	})
}
