//go:build windows

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewOrganizerDomainTool(
			NewMailTool(),
			NewContactsTool(),
			NewCalendarTool(),
			NewRemindersTool(),
		),
		Platforms: []string{PlatformWindows},
		Category:  "productivity",
	})
}
