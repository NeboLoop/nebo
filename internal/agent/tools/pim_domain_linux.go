//go:build linux

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
		Platforms: []string{PlatformLinux},
		Category:  "productivity",
	})
}
