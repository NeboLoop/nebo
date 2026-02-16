package setup

import (
	"net/http"

	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Mark initial setup as complete
func CompleteSetupHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Mark setup as complete by creating the .setup-complete file
		if err := defaults.MarkSetupComplete(); err != nil {
			logging.Errorf("Failed to mark setup as complete: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.CompleteSetupResponse{
			Success: true,
		})
	}
}
