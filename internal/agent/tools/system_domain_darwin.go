//go:build darwin && !ios

package tools

func init() {
	// Register platform-specific resources for the SystemTool.
	// These become system(resource: "app", ...), system(resource: "clipboard", ...), etc.
	RegisterSystemResourceInit("app", NewOSAppTool())
	RegisterSystemResourceInit("clipboard", NewClipboardTool())
	RegisterSystemResourceInit("settings", NewSettingsTool())
	RegisterSystemResourceInit("music", NewMusicTool())
	RegisterSystemResourceInit("search", NewSpotlightTool())
	RegisterSystemResourceInit("keychain", NewKeychainTool())

	// Register notification resource for the MsgTool.
	RegisterMessageResourceInit("notify", NewNotificationTool())
}
