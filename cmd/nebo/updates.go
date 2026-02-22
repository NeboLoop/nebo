package cli

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/neboloop/nebo/internal/realtime"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/updater"
)

// startBackgroundUpdater starts the periodic update checker. It broadcasts
// update_available to all connected browser clients. For "direct" installs it
// also auto-downloads the binary and broadcasts update_progress/update_ready
// (or update_error on failure). For homebrew/package_manager installs only the
// update_available notification is sent.
func startBackgroundUpdater(ctx context.Context, svcCtx *svc.ServiceContext) {
	broadcast := func(eventType string, data map[string]any) {
		h, ok := svcCtx.ClientHub().(*realtime.Hub)
		if !ok || h == nil {
			return
		}
		h.Broadcast(&realtime.Message{
			Type: eventType,
			Data: data,
		})
	}

	checker := updater.NewBackgroundChecker(svcCtx.Version, 6*time.Hour, func(result *updater.Result) {
		// Notify frontend that an update is available
		broadcast("update_available", map[string]any{
			"available":       result.Available,
			"current_version": result.CurrentVersion,
			"latest_version":  result.LatestVersion,
			"release_url":     result.ReleaseURL,
			"published_at":    result.PublishedAt,
		})

		// Only auto-download for direct installs
		installMethod := updater.DetectInstallMethod()
		if installMethod != "direct" {
			return
		}

		// Download with progress reporting
		tmpPath, err := updater.Download(ctx, result.LatestVersion, func(downloaded, total int64) {
			broadcast("update_progress", map[string]any{
				"downloaded": downloaded,
				"total":      total,
			})
		})
		if err != nil {
			fmt.Printf("[updater] download failed: %v\n", err)
			broadcast("update_error", map[string]any{
				"error": err.Error(),
			})
			return
		}

		// Verify checksum
		if err := updater.VerifyChecksum(ctx, tmpPath, result.LatestVersion); err != nil {
			os.Remove(tmpPath)
			fmt.Printf("[updater] checksum failed: %v\n", err)
			broadcast("update_error", map[string]any{
				"error": err.Error(),
			})
			return
		}

		// Stage the update
		if um := svcCtx.UpdateManager(); um != nil {
			um.SetPending(tmpPath, result.LatestVersion)
		}

		broadcast("update_ready", map[string]any{
			"version": result.LatestVersion,
		})
	})

	go checker.Run(ctx)
}
