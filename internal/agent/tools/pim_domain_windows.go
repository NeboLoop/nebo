//go:build windows

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewPIMDomainTool(
			NewMailTool(),
			NewContactsTool(),
			NewCalendarTool(),
			NewRemindersTool(),
			nil,
		),
		Platforms: []string{PlatformWindows},
		Category:  "productivity",
	})
}
