//go:build darwin && !ios

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewOrganizerDomainTool(
			NewMailTool(),
			NewContactsTool(),
			NewCalendarTool(),
			NewRemindersTool(),
		),
		Platforms: []string{PlatformDarwin},
		Category:  "productivity",
	})

	// Register platform-specific message resources for the MsgTool.
	RegisterMessageResourceInit("sms", NewMessagesTool())
}
