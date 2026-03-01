//go:build windows

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewDesktopDomainTool(DesktopDomainOpts{
			Input:      NewDesktopTool(),
			UI:         NewAccessibilityTool(),
			Window:     NewWindowTool(),
			Menu:       NewMenubarTool(),
			Dialog:     NewDialogTool(),
			Space:      NewSpacesTool(),
			Shortcut:   NewShortcutsTool(),
			Screenshot: NewScreenshotTool(),
			TTS:        NewTTSTool(),
		}),
		Platforms: []string{PlatformWindows},
		Category:  "desktop",
	})
}
