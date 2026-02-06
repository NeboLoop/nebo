//go:build !desktop

package cli

import "fmt"

// RunDesktop falls back to headless mode when built without desktop support.
// Build with -tags desktop for native window + system tray.
func RunDesktop() {
	fmt.Println("Desktop mode not available in this build. Running headless...")
	RunAll()
}
