//go:build linux

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewDesktopDomainTool(DesktopDomainOpts{
			Input:      NewDesktopTool(),
			UI:         NewAccessibilityTool(),
			Window:     NewWindowTool(),
			Shortcut:   NewShortcutsTool(),
			Screenshot: NewScreenshotTool(),
			TTS:        NewTTSTool(),
		}),
		Platforms: []string{PlatformLinux},
		Category:  "desktop",
	})
}
