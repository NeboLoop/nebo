package setup

import (
	"net/http"
	"os"
	"path/filepath"

	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Update AI personality configuration
func UpdatePersonalityHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdatePersonalityRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get data directory path
		dataDir, err := defaults.DataDir()
		if err != nil {
			logging.Errorf("Failed to get data directory: %v", err)
			httputil.Error(w, err)
			return
		}

		// Ensure directory exists
		if err := os.MkdirAll(dataDir, 0755); err != nil {
			logging.Errorf("Failed to create data directory: %v", err)
			httputil.Error(w, err)
			return
		}

		soulPath := filepath.Join(dataDir, "SOUL.md")

		// Write content to file
		if err := os.WriteFile(soulPath, []byte(req.Content), 0644); err != nil {
			logging.Errorf("Failed to write SOUL.md: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.UpdatePersonalityResponse{
			Success: true,
		})
	}
}
