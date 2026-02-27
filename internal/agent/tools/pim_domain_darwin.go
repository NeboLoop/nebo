//go:build darwin && !ios

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewPIMDomainTool(
			NewMailTool(),
			NewContactsTool(),
			NewCalendarTool(),
			NewRemindersTool(),
			NewMessagesTool(),
		),
		Platforms: []string{PlatformDarwin},
		Category:  "productivity",
	})
}
