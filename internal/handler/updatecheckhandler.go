package handler

import (
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
	"github.com/neboloop/nebo/internal/updater"
)

// UpdateCheckHandler returns the current version and whether an update is available.
func UpdateCheckHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		installMethod := updater.DetectInstallMethod()
		canAutoUpdate := installMethod == "direct"

		result, err := updater.Check(r.Context(), svcCtx.Version)
		if err != nil {
			// Non-fatal: return current version with available=false
			httputil.OkJSON(w, &types.UpdateCheckResponse{
				Available:      false,
				CurrentVersion: svcCtx.Version,
				InstallMethod:  installMethod,
				CanAutoUpdate:  canAutoUpdate,
			})
			return
		}
		httputil.OkJSON(w, &types.UpdateCheckResponse{
			Available:      result.Available,
			CurrentVersion: result.CurrentVersion,
			LatestVersion:  result.LatestVersion,
			ReleaseURL:     result.ReleaseURL,
			PublishedAt:    result.PublishedAt,
			InstallMethod:  installMethod,
			CanAutoUpdate:  canAutoUpdate,
		})
	}
}
