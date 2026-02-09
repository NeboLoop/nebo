package dev

import (
	"bufio"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"time"

	"github.com/go-chi/chi/v5"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
)

// LogStreamHandler streams app logs via Server-Sent Events (SSE).
// Query params: stream (stdout|stderr, default stdout), lines (initial lines, default 100)
func LogStreamHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "appId")
		if appID == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "appId is required")
			return
		}

		stream := r.URL.Query().Get("stream")
		if stream == "" {
			stream = "stdout"
		}
		if stream != "stdout" && stream != "stderr" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "stream must be stdout or stderr")
			return
		}

		// Look up the app's path from the dev_sideloaded_apps table
		queries := db.New(svcCtx.DB.GetDB())
		row, err := queries.GetDevSideloadedApp(r.Context(), appID)
		if err != nil {
			httputil.NotFound(w, "dev app not found")
			return
		}

		logPath := filepath.Join(row.Path, "logs", stream+".log")

		// Check if log file exists
		if _, err := os.Stat(logPath); os.IsNotExist(err) {
			httputil.NotFound(w, fmt.Sprintf("log file not found: %s", stream+".log"))
			return
		}

		// Set SSE headers
		w.Header().Set("Content-Type", "text/event-stream")
		w.Header().Set("Cache-Control", "no-cache")
		w.Header().Set("Connection", "keep-alive")
		w.Header().Set("X-Accel-Buffering", "no")

		flusher, ok := w.(http.Flusher)
		if !ok {
			httputil.ErrorWithCode(w, http.StatusInternalServerError, "streaming not supported")
			return
		}

		// Open the log file
		f, err := os.Open(logPath)
		if err != nil {
			httputil.InternalError(w, "failed to open log file")
			return
		}
		defer f.Close()

		// Seek to near the end for initial content (last ~32KB)
		stat, _ := f.Stat()
		if stat.Size() > 32*1024 {
			f.Seek(-32*1024, io.SeekEnd)
			// Skip partial first line
			scanner := bufio.NewScanner(f)
			scanner.Scan() // discard partial line
		}

		// Read and send existing content
		scanner := bufio.NewScanner(f)
		scanner.Buffer(make([]byte, 64*1024), 64*1024) // 64KB line buffer
		for scanner.Scan() {
			fmt.Fprintf(w, "data: %s\n\n", scanner.Text())
		}
		flusher.Flush()

		// Tail: poll for new content
		ticker := time.NewTicker(200 * time.Millisecond)
		defer ticker.Stop()

		for {
			select {
			case <-r.Context().Done():
				return
			case <-ticker.C:
				// Read any new lines
				hasNew := false
				for scanner.Scan() {
					fmt.Fprintf(w, "data: %s\n\n", scanner.Text())
					hasNew = true
				}
				if hasNew {
					flusher.Flush()
				}
			}
		}
	}
}
