package handler

import (
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/updater"
)

// UpdateCheckHandler returns the current version and whether an update is available.
func UpdateCheckHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		result, err := updater.Check(r.Context(), svcCtx.Version)
		if err != nil {
			// Non-fatal: return current version with available=false
			httputil.OkJSON(w, &updater.Result{
				Available:      false,
				CurrentVersion: svcCtx.Version,
			})
			return
		}
		httputil.OkJSON(w, result)
	}
}
