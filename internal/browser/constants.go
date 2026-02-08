// Package browser provides Playwright-based browser automation for Nebo.
// Provides stable refs and persistent sessions for browser automation.
package browser

// Default profile constants - branded for Nebo
const (
	// DefaultProfileName is the default managed browser profile name.
	DefaultProfileName = "nebo"

	// DefaultProfileColor is the Nebo brand color (gold).
	DefaultProfileColor = "#ffbe18"

	// DefaultCDPPort is the default Chrome DevTools Protocol port for managed browser.
	DefaultCDPPort = 9222

	// DefaultControlPort is the internal browser control port.
	DefaultControlPort = 9223

	// ExtensionRelayPort is the port for Chrome extension relay WebSocket server.
	// Runs on nebo's main port (27895) at /relay path.
	ExtensionRelayPort = 27895
)

// Profile driver types
const (
	// DriverNebo uses Nebo-managed browser with persistent profile.
	DriverNebo = "nebo"

	// DriverExtension uses Chrome extension relay to user's existing browser.
	DriverExtension = "extension"
)
