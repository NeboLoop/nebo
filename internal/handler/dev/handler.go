package dev

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/go-chi/chi/v5"

	"github.com/nebolabs/nebo/internal/apps"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// SideloadHandler adds a project directory to the dev workspace.
// This just saves the path — no build, no launch. The developer uses the
// Dev Assistant to scaffold/build, then triggers Build & Run separately.
func SideloadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.SideloadRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if req.Path == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "path is required")
			return
		}

		// Verify the path exists and is a directory
		info, err := os.Stat(req.Path)
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "path does not exist")
			return
		}
		if !info.IsDir() {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "path is not a directory")
			return
		}

		// If manifest exists, use that directory and its ID.
		// Otherwise use the directory name as the project ID.
		projectPath := req.Path
		appID := filepath.Base(req.Path)
		name := appID

		manifest, merr := apps.LoadManifest(req.Path)
		if merr == nil {
			appID = manifest.ID
			name = manifest.Name
		}

		// Persist in dev_sideloaded_apps table
		queries := db.New(svcCtx.DB.GetDB())
		_ = queries.InsertDevSideloadedApp(r.Context(), db.InsertDevSideloadedAppParams{
			AppID: appID,
			Path:  projectPath,
		})

		httputil.OkJSON(w, &types.SideloadResponse{
			AppID: appID,
			Name:  name,
			Path:  projectPath,
		})
	}
}

// UnsideloadHandler stops a sideloaded app and removes its symlink.
func UnsideloadHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "appId")
		if appID == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "appId is required")
			return
		}

		registry := getRegistry(svcCtx)
		if registry == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app registry not available (agent not connected)")
			return
		}

		if err := registry.Unsideload(appID); err != nil {
			httputil.ErrorWithCode(w, http.StatusBadRequest, err.Error())
			return
		}

		// Remove from dev_sideloaded_apps table
		queries := db.New(svcCtx.DB.GetDB())
		_ = queries.DeleteDevSideloadedApp(r.Context(), appID)

		httputil.OkJSON(w, &types.MessageResponse{Message: "app unsideloaded"})
	}
}

// ListDevAppsHandler returns all sideloaded dev apps with their running status.
func ListDevAppsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		queries := db.New(svcCtx.DB.GetDB())
		rows, err := queries.ListDevSideloadedApps(r.Context())
		if err != nil {
			httputil.InternalError(w, "failed to list dev apps")
			return
		}

		registry := getRegistry(svcCtx)
		items := make([]types.DevAppItem, 0, len(rows))
		for _, row := range rows {
			item := types.DevAppItem{
				AppID:    row.AppID,
				Path:     row.Path,
				LoadedAt: row.LoadedAt,
				Name:     filepath.Base(row.Path),
			}

			// Try to get name/version from manifest (may not exist yet for new projects)
			manifest, merr := apps.LoadManifest(row.Path)
			if merr == nil {
				item.Name = manifest.Name
				item.Version = manifest.Version
			}

			// Check running status
			if registry != nil {
				item.Running = registry.IsRunning(row.AppID)
			}

			items = append(items, item)
		}

		httputil.OkJSON(w, &types.ListDevAppsResponse{Apps: items})
	}
}

// OpenDevWindowHandler opens the developer window as a separate native window.
func OpenDevWindowHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		fn := svcCtx.OpenDevWindow()
		if fn == nil {
			httputil.ErrorWithCode(w, http.StatusNotImplemented, "dev window not available")
			return
		}
		fn()
		httputil.OkJSON(w, &types.OpenDevWindowResponse{Opened: true})
	}
}

// BrowseDirectoryHandler opens a native directory picker and returns the selected path.
func BrowseDirectoryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		fn := svcCtx.BrowseDirectory()
		if fn == nil {
			httputil.ErrorWithCode(w, http.StatusNotImplemented, "directory browser not available")
			return
		}

		path, err := fn()
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusInternalServerError, err.Error())
			return
		}

		// Empty path means user cancelled — return empty path, not an error
		httputil.OkJSON(w, &types.BrowseDirectoryResponse{Path: path})
	}
}

// getRegistry extracts the *apps.AppRegistry from the ServiceContext.
func getRegistry(svcCtx *svc.ServiceContext) *apps.AppRegistry {
	r := svcCtx.AppRegistry()
	if r == nil {
		return nil
	}
	reg, ok := r.(*apps.AppRegistry)
	if !ok {
		return nil
	}
	return reg
}

// RelaunchDevAppHandler builds and launches (or re-launches) a dev app.
// Handles both first launch and subsequent restarts.
func RelaunchDevAppHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "appId")
		if appID == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "appId is required")
			return
		}

		registry := getRegistry(svcCtx)
		if registry == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app registry not available (agent not connected)")
			return
		}

		// Get the project path from DB
		queries := db.New(svcCtx.DB.GetDB())
		row, err := queries.GetDevSideloadedApp(context.Background(), appID)
		if err != nil {
			httputil.NotFound(w, "dev app not found")
			return
		}

		// Stop if currently running
		if registry.IsRunning(appID) {
			_ = registry.Unsideload(appID)
		}

		// Build and launch via Sideload (validates manifest, runs make build, finds binary, launches)
		manifest, err := registry.Sideload(r.Context(), row.Path)
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusBadRequest, err.Error())
			return
		}

		// Update DB with manifest ID if it changed (e.g. first build after scaffolding)
		if manifest.ID != appID {
			_ = queries.DeleteDevSideloadedApp(r.Context(), appID)
			_ = queries.InsertDevSideloadedApp(r.Context(), db.InsertDevSideloadedAppParams{
				AppID: manifest.ID,
				Path:  row.Path,
			})
		}

		httputil.OkJSON(w, &types.SideloadResponse{
			AppID:   manifest.ID,
			Name:    manifest.Name,
			Version: manifest.Version,
			Path:    row.Path,
		})
	}
}

// ProjectContextHandler returns full project context for the Dev Assistant system prompt.
// Includes file listing, manifest contents, build state, and recent logs.
func ProjectContextHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := chi.URLParam(r, "appId")
		if appID == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "appId is required")
			return
		}

		queries := db.New(svcCtx.DB.GetDB())
		row, err := queries.GetDevSideloadedApp(r.Context(), appID)
		if err != nil {
			httputil.NotFound(w, "dev project not found")
			return
		}

		projectPath := row.Path
		ctx := types.ProjectContext{
			Path: projectPath,
		}

		// File listing (top-level + one level deep)
		ctx.Files = listProjectFiles(projectPath)

		// Manifest
		if data, err := os.ReadFile(filepath.Join(projectPath, "manifest.json")); err == nil {
			ctx.ManifestRaw = string(data)
			var m map[string]any
			if json.Unmarshal(data, &m) == nil {
				if id, ok := m["id"].(string); ok {
					ctx.AppID = id
				}
				if name, ok := m["name"].(string); ok {
					ctx.Name = name
				}
				if ver, ok := m["version"].(string); ok {
					ctx.Version = ver
				}
			}
		}

		// Makefile
		if _, err := os.Stat(filepath.Join(projectPath, "Makefile")); err == nil {
			ctx.HasMakefile = true
		}

		// Binary
		if bp, err := apps.FindBinary(projectPath); err == nil {
			ctx.BinaryPath = bp
		}

		// Running status
		registry := getRegistry(svcCtx)
		if registry != nil {
			checkID := appID
			if ctx.AppID != "" {
				checkID = ctx.AppID
			}
			ctx.Running = registry.IsRunning(checkID)
		}

		// Recent logs (last 50 lines of stderr + stdout)
		ctx.RecentLogs = readRecentLogs(projectPath, 50)

		httputil.OkJSON(w, &ctx)
	}
}

// listProjectFiles returns a flat list of files in the project (top-level + one level deep).
func listProjectFiles(dir string) []string {
	var files []string
	entries, err := os.ReadDir(dir)
	if err != nil {
		return files
	}
	for _, e := range entries {
		name := e.Name()
		if strings.HasPrefix(name, ".") && name != ".gitignore" {
			continue
		}
		if e.IsDir() {
			files = append(files, name+"/")
			// One level deep
			subEntries, err := os.ReadDir(filepath.Join(dir, name))
			if err == nil {
				for _, se := range subEntries {
					if strings.HasPrefix(se.Name(), ".") {
						continue
					}
					subPath := name + "/" + se.Name()
					if se.IsDir() {
						subPath += "/"
					}
					files = append(files, subPath)
				}
			}
		} else {
			files = append(files, name)
		}
	}
	return files
}

// readRecentLogs reads the last N lines from the app's log files.
func readRecentLogs(projectPath string, maxLines int) string {
	var parts []string
	for _, logFile := range []string{"logs/stderr.log", "logs/stdout.log"} {
		data, err := os.ReadFile(filepath.Join(projectPath, logFile))
		if err != nil || len(data) == 0 {
			continue
		}
		lines := strings.Split(strings.TrimSpace(string(data)), "\n")
		start := 0
		if len(lines) > maxLines {
			start = len(lines) - maxLines
		}
		parts = append(parts, fmt.Sprintf("=== %s ===\n%s", logFile, strings.Join(lines[start:], "\n")))
	}
	if len(parts) == 0 {
		return ""
	}
	return strings.Join(parts, "\n\n")
}
