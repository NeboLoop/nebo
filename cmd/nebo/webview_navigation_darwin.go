//go:build desktop && darwin

package cli

// InjectWebViewNavigationHandler is currently a no-op on macOS.
// External URL handling is done via the main window's injected JS.
func InjectWebViewNavigationHandler() {}
