package setup

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/defaults"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

func SetupStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Check if any admin user exists
		hasAdmin, err := svcCtx.DB.HasAdminUser(ctx)
		if err != nil {
			logging.Errorf("Failed to check for admin user: %v", err)
			httputil.Error(w, err)
			return
		}

		// Check if setup has been marked as complete
		setupComplete, err := defaults.IsSetupComplete()
		if err != nil {
			logging.Errorf("Failed to check setup complete status: %v", err)
			// Non-fatal error, assume setup is not complete
			setupComplete = false
		}

		httputil.OkJSON(w, &types.SetupStatusResponse{
			SetupRequired: hasAdmin == 0,
			HasAdmin:      hasAdmin == 1,
			SetupComplete: setupComplete,
		})
	}
}
