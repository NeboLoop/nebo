//go:build darwin && !ios

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewPIMDomainTool(
			NewMailTool(),
			NewContactsTool(),
			NewCalendarTool(),
			NewRemindersTool(),
		),
		Platforms: []string{PlatformDarwin},
		Category:  "productivity",
	})
}
