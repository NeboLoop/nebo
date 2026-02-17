//go:build desktop && !darwin

package cli

// InjectWebViewMediaPermissions is a no-op on non-macOS platforms.
// Only macOS WebKit requires runtime delegate injection for media capture.
func InjectWebViewMediaPermissions() {}
