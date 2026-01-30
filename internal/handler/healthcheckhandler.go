package handler

import (
	"net/http"
	"time"

	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

const version = "1.0.0"

func HealthCheckHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		httputil.OkJSON(w, &types.HealthResponse{
			Status:    "healthy",
			Version:   version,
			Timestamp: time.Now().UTC().Format(time.RFC3339),
		})
	}
}
