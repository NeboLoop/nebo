//go:build linux

package tools

func init() {
	RegisterSystemResourceInit("app", NewOSAppTool())
	RegisterSystemResourceInit("clipboard", NewClipboardTool())
	RegisterSystemResourceInit("settings", NewSettingsTool())
	RegisterSystemResourceInit("music", NewMusicTool())
	RegisterSystemResourceInit("search", NewSpotlightTool())
	RegisterSystemResourceInit("keychain", NewKeychainTool())

	// Register notification resource for the MsgTool.
	RegisterMessageResourceInit("notify", NewNotificationTool())
}
