//go:build linux

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewOrganizerDomainTool(
			NewMailTool(),
			NewContactsTool(),
			NewCalendarTool(),
			NewRemindersTool(),
		),
		Platforms: []string{PlatformLinux},
		Category:  "productivity",
	})
}
