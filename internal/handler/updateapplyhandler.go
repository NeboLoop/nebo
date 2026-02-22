package handler

import (
	"log"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
	"github.com/neboloop/nebo/internal/updater"
)

// UpdateApplyHandler triggers a pending auto-update. No client-supplied paths —
// the pending binary path is tracked server-side in UpdateManager.
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
		if pendingPath == "" {
			httputil.OkJSON(w, &types.UpdateApplyResponse{
				Status:  "no_update",
				Message: "no pending update",
			})
			return
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
