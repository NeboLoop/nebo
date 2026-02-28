package voice

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"

	voicepkg "github.com/neboloop/nebo/internal/voice"
)

// ModelsStatusHandler returns the download status of required voice models.
//
//	GET /api/v1/voice/models/status → { "ready": bool, "models": [...] }
func ModelsStatusHandler() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]any{
			"ready":  voicepkg.VoiceModelsReady(),
			"models": voicepkg.ModelStatus(),
		})
	}
}

// ModelsDownloadHandler downloads missing voice models, streaming progress via SSE.
//
//	POST /api/v1/voice/models/download → SSE stream of DownloadProgress events
func ModelsDownloadHandler() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Already ready — nothing to download
		if voicepkg.VoiceModelsReady() {
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]any{"ready": true})
			return
		}

		// Set up SSE
		flusher, ok := w.(http.Flusher)
		if !ok {
			http.Error(w, "streaming not supported", http.StatusInternalServerError)
			return
		}

		w.Header().Set("Content-Type", "text/event-stream")
		w.Header().Set("Cache-Control", "no-cache")
		w.Header().Set("Connection", "keep-alive")

		ctx, cancel := context.WithCancel(r.Context())
		defer cancel()

		// Stream progress events
		err := voicepkg.DownloadModels(ctx, func(p voicepkg.DownloadProgress) {
			data, _ := json.Marshal(p)
			fmt.Fprintf(w, "data: %s\n\n", data)
			flusher.Flush()
		})

		if err != nil {
			errData, _ := json.Marshal(map[string]string{"error": err.Error()})
			fmt.Fprintf(w, "data: %s\n\n", errData)
			flusher.Flush()
			return
		}

		// Final done event
		doneData, _ := json.Marshal(map[string]any{"ready": true})
		fmt.Fprintf(w, "data: %s\n\n", doneData)
		flusher.Flush()
	}
}
