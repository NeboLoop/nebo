package browser

import (
	"context"
	"testing"
	"time"
)

func TestBrowserManagerStart(t *testing.T) {
	mgr := GetManager()

	err := mgr.Start(Config{
		Enabled:  true,
		Headless: true,
	})
	if err != nil {
		t.Fatalf("Failed to start browser manager: %v", err)
	}
	defer mgr.Stop()

	t.Log("Browser manager started successfully")
}

func TestBrowserNavigate(t *testing.T) {
	mgr := GetManager()

	err := mgr.Start(Config{
		Enabled:  true,
		Headless: true,
	})
	if err != nil {
		t.Fatalf("Failed to start browser manager: %v", err)
	}
	defer mgr.Stop()

	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	session, err := mgr.GetSession(ctx, DefaultProfileName)
	if err != nil {
		t.Fatalf("Failed to get session: %v", err)
	}

	page, err := session.GetPage("")
	if err != nil {
		t.Fatalf("Failed to get page: %v", err)
	}

	result, err := page.Navigate(ctx, NavigateOptions{URL: "https://example.com"})
	if err != nil {
		t.Fatalf("Navigate failed: %v", err)
	}

	t.Logf("Navigated to example.com, title: %s", result.Title)

	// Get snapshot
	snapshot, err := page.Snapshot(ctx, SnapshotOptions{IncludeRefs: true})
	if err != nil {
		t.Fatalf("Snapshot failed: %v", err)
	}

	t.Logf("Snapshot (first 500 chars):\n%s", snapshot[:min(500, len(snapshot))])
}
