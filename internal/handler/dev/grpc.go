package dev

import (
	"encoding/json"
	"fmt"
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
)

// GrpcStreamHandler streams captured gRPC traffic for a dev app via SSE.
// Only works for sideloaded (dev) apps — production apps are never inspectable.
func GrpcStreamHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "appId")
		if appID == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "appId is required")
			return
		}

		registry := getRegistry(svcCtx)
		if registry == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable,
				"app registry not available (agent not connected)")
			return
		}

		if !registry.IsSideloaded(appID) {
			httputil.ErrorWithCode(w, http.StatusForbidden,
				"gRPC inspection is only available for sideloaded dev apps")
			return
		}

		ins := registry.Inspector()
		if ins == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "inspector not available")
			return
		}

		// Set SSE headers (same pattern as LogStreamHandler)
		w.Header().Set("Content-Type", "text/event-stream")
		w.Header().Set("Cache-Control", "no-cache")
		w.Header().Set("Connection", "keep-alive")
		w.Header().Set("X-Accel-Buffering", "no")

		flusher, ok := w.(http.Flusher)
		if !ok {
			httputil.ErrorWithCode(w, http.StatusInternalServerError, "streaming not supported")
			return
		}

		// Backfill: send recent events for this app
		recent := ins.Recent(appID, 200)
		for _, e := range recent {
			data, _ := json.Marshal(e)
			fmt.Fprintf(w, "data: %s\n\n", data)
		}
		flusher.Flush()

		// Subscribe to live events
		ch, unsub := ins.Subscribe()
		defer unsub()

		for {
			select {
			case <-r.Context().Done():
				return
			case e, ok := <-ch:
				if !ok {
					return
				}
				// Filter to the requested app
				if e.AppID != appID {
					continue
				}
				data, _ := json.Marshal(e)
				fmt.Fprintf(w, "data: %s\n\n", data)
				flusher.Flush()
			}
		}
	}
}
