package files

import (
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/neboloop/nebo/internal/defaults"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// filesDir returns the agent files directory under the Nebo data dir.
func filesDir() (string, error) {
	dataDir, err := defaults.DataDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(dataDir, "files")
	os.MkdirAll(dir, 0755)
	return dir, nil
}

// ServeFileHandler serves files from <data_dir>/files/ with path traversal protection.
// Route: GET /api/v1/files/*
func ServeFileHandler(_ *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Get the wildcard path after /api/v1/files/
		requestedPath := chi.URLParam(r, "*")
		if requestedPath == "" {
			http.Error(w, "file path required", http.StatusBadRequest)
			return
		}

		// Clean and validate — reject any traversal attempts
		cleaned := filepath.Clean(requestedPath)
		if strings.Contains(cleaned, "..") {
			http.Error(w, "invalid path", http.StatusBadRequest)
			return
		}

		baseDir, err := filesDir()
		if err != nil {
			http.Error(w, "files directory unavailable", http.StatusInternalServerError)
			return
		}

		fullPath := filepath.Join(baseDir, cleaned)

		// Double-check the resolved path is still within the base directory
		if !strings.HasPrefix(fullPath, baseDir) {
			http.Error(w, "invalid path", http.StatusBadRequest)
			return
		}

		// Check file exists
		info, err := os.Stat(fullPath)
		if err != nil || info.IsDir() {
			http.Error(w, "file not found", http.StatusNotFound)
			return
		}

		// Set appropriate content type and cache headers
		ext := strings.ToLower(filepath.Ext(fullPath))
		switch ext {
		case ".png":
			w.Header().Set("Content-Type", "image/png")
		case ".jpg", ".jpeg":
			w.Header().Set("Content-Type", "image/jpeg")
		case ".gif":
			w.Header().Set("Content-Type", "image/gif")
		case ".webp":
			w.Header().Set("Content-Type", "image/webp")
		case ".svg":
			w.Header().Set("Content-Type", "image/svg+xml")
		case ".pdf":
			w.Header().Set("Content-Type", "application/pdf")
		default:
			w.Header().Set("Content-Type", "application/octet-stream")
		}

		// Cache for 1 hour — files are content-addressed by timestamp
		w.Header().Set("Cache-Control", "public, max-age=3600")

		// Serve the file
		http.ServeFile(w, r, fullPath)
	}
}

// BrowseFilesHandler opens a native file picker and returns the selected paths.
// Route: POST /api/v1/files/browse
func BrowseFilesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		fn := svcCtx.BrowseFiles()
		if fn == nil {
			httputil.ErrorWithCode(w, http.StatusNotImplemented, "file browser not available")
			return
		}

		paths, err := fn()
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusInternalServerError, err.Error())
			return
		}

		if paths == nil {
			paths = []string{}
		}

		httputil.OkJSON(w, &types.BrowseFilesResponse{Paths: paths})
	}
}

// FilesBaseURL returns the base URL for serving agent files.
func FilesBaseURL(port int) string {
	return fmt.Sprintf("http://localhost:%d/api/v1/files", port)
}
