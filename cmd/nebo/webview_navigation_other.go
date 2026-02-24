//go:build desktop && !darwin && !linux

package cli

// InjectWebViewNavigationHandler is a no-op on Windows.
// External URL interception uses JavaScript-based handling in +layout.svelte
// with the Navigation API (Chromium-native in WebView2).
func InjectWebViewNavigationHandler() {}
