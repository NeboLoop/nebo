//go:build linux

package tools

func init() {
	RegisterCapability(&Capability{
		Tool: NewSystemDomainTool(SystemDomainOpts{
			App:       NewAppTool(),
			Notify:    NewNotificationTool(),
			Clipboard: NewClipboardTool(),
			Settings:  NewSystemTool(),
			Music:     NewMusicTool(),
			Search:    NewSpotlightTool(),
			Keychain:  NewKeychainTool(),
		}),
		Platforms: []string{PlatformLinux},
		Category:  "system",
	})
}
