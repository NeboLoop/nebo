package main

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/nebolabs/nebo/internal/browser"
)

func main() {
	fmt.Println("=== Browser Test ===")

	// Parse args
	action := "snapshot"
	url := ""
	if len(os.Args) >= 2 {
		action = os.Args[1]
	}
	if len(os.Args) >= 3 {
		url = os.Args[2]
	}

	// Check relay auth
	fmt.Println("\n1. Checking relay auth...")
	cdpURL := "ws://127.0.0.1:27895/relay/cdp"
	headers := browser.GetRelayAuthHeaders(cdpURL)
	fmt.Printf("   CDPUrl: %s\n", cdpURL)
	fmt.Printf("   Headers: %v\n", headers)

	// Get profile config
	fmt.Println("\n2. Getting chrome profile...")
	profile := &browser.ResolvedProfile{
		Name:   "chrome",
		Driver: browser.DriverExtension,
		CDPUrl: cdpURL,
	}
	fmt.Printf("   Profile: %+v\n", profile)

	// Try to create session
	fmt.Println("\n3. Creating session...")
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	session, err := browser.GetOrCreateSession(ctx, profile)
	if err != nil {
		fmt.Printf("   ERROR: %v\n", err)
		return
	}
	fmt.Println("   Session created successfully!")

	// Get page
	page, err := session.GetPage("")
	if err != nil {
		fmt.Printf("   ERROR getting page: %v\n", err)
		return
	}

	switch action {
	case "navigate":
		if url == "" {
			fmt.Println("Usage: testbrowser navigate <url>")
			return
		}
		fmt.Printf("\n4. Navigating to %s...\n", url)
		result, err := page.Navigate(ctx, browser.NavigateOptions{URL: url})
		if err != nil {
			fmt.Printf("   ERROR: %v\n", err)
			return
		}
		fmt.Printf("   Result: %+v\n", result)

	case "snapshot":
		fmt.Println("\n4. Taking snapshot...")
		snapshot, err := page.Snapshot(ctx, browser.SnapshotOptions{})
		if err != nil {
			fmt.Printf("   ERROR: %v\n", err)
			return
		}
		fmt.Printf("Snapshot (%d chars):\n%s\n", len(snapshot), snapshot)

	case "screenshot":
		fmt.Println("\n4. Taking screenshot...")
		b64, err := page.Screenshot(ctx, browser.ScreenshotOptions{FullPage: false})
		if err != nil {
			fmt.Printf("   ERROR: %v\n", err)
			return
		}
		fmt.Printf("Screenshot data: %d chars (base64)\n", len(b64))
		if len(b64) > 100 {
			fmt.Printf("   data:image/png;base64,%s...\n", b64[:100])
		}

	default:
		fmt.Printf("Unknown action: %s (valid: navigate, snapshot, screenshot)\n", action)
	}

	fmt.Println("\n=== Done ===")
}
