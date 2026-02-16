package handler

import (
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

func HealthCheckHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		httputil.OkJSON(w, &types.HealthResponse{
			Status:    "healthy",
			Version:   svcCtx.Version,
			Timestamp: time.Now().UTC().Format(time.RFC3339),
		})
	}
}
