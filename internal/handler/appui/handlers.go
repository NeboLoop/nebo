package appui

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/go-chi/chi/v5"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
	"github.com/neboloop/nebo/internal/webview"
)

// ListUIAppsHandler returns all apps that provide UI.
// GET /apps/ui
func ListUIAppsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		p := svcCtx.AppUI()
		if p == nil {
			httputil.OkJSON(w, map[string]any{"apps": []any{}})
			return
		}
		httputil.OkJSON(w, map[string]any{"apps": p.ListUIApps()})
	}
}

// AppAPIProxyHandler proxies HTTP requests from the browser to a UI app.
// ANY /apps/{id}/api/*
func AppAPIProxyHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "id")
		p := svcCtx.AppUI()
		if p == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app system not ready")
			return
		}

		// Read request body (limit 10MB)
		body, err := io.ReadAll(io.LimitReader(r.Body, 10<<20))
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Extract path after /apps/{id}/api/
		apiPath := chi.URLParam(r, "*")
		if !strings.HasPrefix(apiPath, "/") {
			apiPath = "/" + apiPath
		}

		// Copy headers
		headers := make(map[string]string, len(r.Header))
		for k := range r.Header {
			headers[k] = r.Header.Get(k)
		}

		resp, err := p.HandleRequest(r.Context(), appID, &svc.AppHTTPRequest{
			Method:  r.Method,
			Path:    apiPath,
			Query:   r.URL.RawQuery,
			Headers: headers,
			Body:    body,
		})
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusBadGateway, err.Error())
			return
		}

		// Write response headers
		for k, v := range resp.Headers {
			w.Header().Set(k, v)
		}
		w.WriteHeader(resp.StatusCode)
		w.Write(resp.Body)
	}
}

// AppStaticHandler serves static frontend files from an app's ui/ directory.
// GET /apps/{id}/ui/*
func AppStaticHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "id")
		p := svcCtx.AppUI()
		if p == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app system not ready")
			return
		}

		// Build base path: {appsDir}/{appID}/ui/
		baseDir := filepath.Join(p.AppsDir(), appID, "ui")

		// Get the wildcard path after /apps/{id}/ui/
		requestedPath := chi.URLParam(r, "*")
		if requestedPath == "" {
			requestedPath = "index.html"
		}

		// Clean and validate — reject any traversal attempts
		cleaned := filepath.Clean(requestedPath)
		if strings.Contains(cleaned, "..") {
			http.Error(w, "invalid path", http.StatusBadRequest)
			return
		}

		fullPath := filepath.Join(baseDir, cleaned)

		// Double-check the resolved path is still within the base directory
		if !strings.HasPrefix(fullPath, baseDir) {
			http.Error(w, "invalid path", http.StatusBadRequest)
			return
		}

		// Check file exists — SPA fallback: serve index.html for unknown paths
		info, err := os.Stat(fullPath)
		if err != nil || info.IsDir() {
			indexPath := filepath.Join(baseDir, "index.html")
			if _, indexErr := os.Stat(indexPath); indexErr == nil {
				http.ServeFile(w, r, indexPath)
				return
			}
			http.Error(w, "not found", http.StatusNotFound)
			return
		}

		http.ServeFile(w, r, fullPath)
	}
}

// OpenAppUIHandler opens an app's config UI in a new window.
// Desktop mode: creates a native Wails window. Headless: returns the URL.
// POST /apps/{id}/ui/open
func OpenAppUIHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "id")
		p := svcCtx.AppUI()
		if p == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app system not ready")
			return
		}

		// Build the URL for the app's UI
		uiURL := fmt.Sprintf("/api/v1/apps/%s/ui/", appID)

		// Try native window (desktop mode)
		mgr := webview.GetManager()
		if mgr != nil && mgr.IsAvailable() {
			// Find app name for window title
			title := appID
			for _, app := range p.ListUIApps() {
				if app.ID == appID {
					title = app.Name
					break
				}
			}

			absURL := fmt.Sprintf("http://localhost:%d%s", svcCtx.Config.Port, uiURL)
			_, err := mgr.CreateWindow(absURL, title, "")
			if err != nil {
				httputil.ErrorWithCode(w, http.StatusInternalServerError, err.Error())
				return
			}
			httputil.OkJSON(w, types.OpenAppUIResponse{Opened: true})
			return
		}

		// Headless fallback — frontend will window.open()
		httputil.OkJSON(w, types.OpenAppUIResponse{Opened: false, URL: uiURL})
	}
}
