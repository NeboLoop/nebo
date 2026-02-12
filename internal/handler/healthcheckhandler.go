package handler

import (
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

const version = "0.1.0"

func HealthCheckHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		httputil.OkJSON(w, &types.HealthResponse{
			Status:    "healthy",
			Version:   version,
			Timestamp: time.Now().UTC().Format(time.RFC3339),
		})
	}
}
