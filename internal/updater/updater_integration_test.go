package updater

import (
	"context"
	"fmt"
	"os"
	"testing"
)

// TestBackgroundUpdateFlow verifies the full CDN-based update check, download,
// and checksum verification pipeline end-to-end. Run manually:
//
//	go test -v -run TestBackgroundUpdateFlow ./internal/updater/ -count=1
func TestBackgroundUpdateFlow(t *testing.T) {
	if os.Getenv("NEBO_INTEGRATION") == "" {
		t.Skip("set NEBO_INTEGRATION=1 to run")
	}

	ctx := context.Background()

	// 1. Check for update (pretend we're running v0.1.7)
	result, err := Check(ctx, "v0.1.7")
	if err != nil {
		t.Fatalf("Check failed: %v", err)
	}
	t.Logf("Available: %v | Current: %s | Latest: %s", result.Available, result.CurrentVersion, result.LatestVersion)

	if !result.Available {
		t.Skip("No update available — CDN version.json may not be ahead of v0.1.7")
	}

	// 2. Download binary
	t.Log("Downloading...")
	tmpPath, err := Download(ctx, result.LatestVersion, func(dl, total int64) {
		if total > 0 {
			fmt.Printf("\r  %d/%d (%d%%)", dl, total, dl*100/total)
		}
	})
	if err != nil {
		t.Fatalf("Download failed: %v", err)
	}
	defer os.Remove(tmpPath)
	fmt.Println() // newline after progress
	t.Logf("Downloaded to: %s", tmpPath)

	// 3. Verify checksum
	t.Log("Verifying checksum...")
	if err := VerifyChecksum(ctx, tmpPath, result.LatestVersion); err != nil {
		t.Fatalf("Checksum verification failed: %v", err)
	}
	t.Log("Checksum verified!")

	// 4. Verify install method detection
	method := DetectInstallMethod()
	t.Logf("Install method: %s", method)
}
