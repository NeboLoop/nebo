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

// Get AI personality configuration
func GetPersonalityHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Get data directory path
		dataDir, err := defaults.DataDir()
		if err != nil {
			logging.Errorf("Failed to get data directory: %v", err)
			httputil.Error(w, err)
			return
		}

		soulPath := filepath.Join(dataDir, "SOUL.md")

		// Try to read existing file
		content, err := os.ReadFile(soulPath)
		if err != nil {
			if os.IsNotExist(err) {
				// File doesn't exist, return default content
				defaultContent, defaultErr := defaults.GetDefault("SOUL.md")
				if defaultErr != nil {
					logging.Errorf("Failed to get default SOUL.md: %v", defaultErr)
					httputil.Error(w, defaultErr)
					return
				}
				httputil.OkJSON(w, &types.GetPersonalityResponse{
					Content: string(defaultContent),
				})
				return
			}
			logging.Errorf("Failed to read SOUL.md: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.GetPersonalityResponse{
			Content: string(content),
		})
	}
}
