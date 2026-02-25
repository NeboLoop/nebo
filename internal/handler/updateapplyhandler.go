package handler

import (
	"context"
	"log"
	"net/http"
	"os"
	"time"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
	"github.com/neboloop/nebo/internal/updater"
)

// UpdateApplyHandler triggers an auto-update. If a binary is already staged,
// it applies immediately. Otherwise it checks for an available update,
// downloads, verifies, stages, and applies — so "Update Now" always works
// regardless of whether the background downloader has finished.
func UpdateApplyHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		um := svcCtx.UpdateManager()
		if um == nil {
			httputil.OkJSON(w, &types.UpdateApplyResponse{
				Status:  "no_update",
				Message: "no update manager available",
			})
			return
		}

		pendingPath := um.PendingPath()

		// No staged binary — try to download one now.
		// Use a detached context so the download survives if the browser
		// disconnects during the 10-30s fetch (tab close, network blip).
		if pendingPath == "" {
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
			defer cancel()

			result, err := updater.Check(ctx, svcCtx.Version)
			if err != nil || result == nil || !result.Available {
				httputil.OkJSON(w, &types.UpdateApplyResponse{
					Status:  "no_update",
					Message: "no update available",
				})
				return
			}

			tmpPath, err := updater.Download(ctx, result.LatestVersion, nil)
			if err != nil {
				log.Printf("[updater] on-demand download failed: %v", err)
				httputil.OkJSON(w, &types.UpdateApplyResponse{
					Status:  "error",
					Message: "download failed",
				})
				return
			}

			if err := updater.VerifyChecksum(ctx, tmpPath, result.LatestVersion); err != nil {
				os.Remove(tmpPath)
				log.Printf("[updater] on-demand checksum failed: %v", err)
				httputil.OkJSON(w, &types.UpdateApplyResponse{
					Status:  "error",
					Message: "verification failed",
				})
				return
			}

			um.SetPending(tmpPath, result.LatestVersion)
			pendingPath = tmpPath
		}

		// Respond before restarting so the frontend gets the response
		httputil.OkJSON(w, &types.UpdateApplyResponse{
			Status:  "restarting",
			Message: "applying update and restarting",
		})

		// Give the response time to flush, then apply
		go func() {
			time.Sleep(500 * time.Millisecond)
			if err := updater.Apply(pendingPath); err != nil {
				log.Printf("[updater] apply failed: %v", err)
			}
		}()
	}
}
