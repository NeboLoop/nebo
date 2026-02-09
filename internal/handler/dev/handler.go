package dev

import (
	"context"
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/nebolabs/nebo/internal/apps"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// SideloadHandler validates a developer's project directory and creates a symlink in appsDir.
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

		registry := getRegistry(svcCtx)
		if registry == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app registry not available (agent not connected)")
			return
		}

		manifest, err := registry.Sideload(r.Context(), req.Path)
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusBadRequest, err.Error())
			return
		}

		// Persist in dev_sideloaded_apps table
		queries := db.New(svcCtx.DB.GetDB())
		_ = queries.InsertDevSideloadedApp(r.Context(), db.InsertDevSideloadedAppParams{
			AppID: manifest.ID,
			Path:  req.Path,
		})

		httputil.OkJSON(w, &types.SideloadResponse{
			AppID:   manifest.ID,
			Name:    manifest.Name,
			Version: manifest.Version,
			Path:    req.Path,
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
			}

			// Try to get name/version from manifest
			manifest, merr := apps.LoadManifest(row.Path)
			if merr == nil {
				item.Name = manifest.Name
				item.Version = manifest.Version
			} else {
				item.Name = row.AppID
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

// RelaunchDevAppHandler stops and re-launches a sideloaded app.
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

		// Get the original path from DB
		queries := db.New(svcCtx.DB.GetDB())
		row, err := queries.GetDevSideloadedApp(context.Background(), appID)
		if err != nil {
			httputil.NotFound(w, "dev app not found")
			return
		}

		// Unsideload then re-sideload for a clean restart
		_ = registry.Unsideload(appID)
		manifest, err := registry.Sideload(r.Context(), row.Path)
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusBadRequest, err.Error())
			return
		}

		httputil.OkJSON(w, &types.SideloadResponse{
			AppID:   manifest.ID,
			Name:    manifest.Name,
			Version: manifest.Version,
			Path:    row.Path,
		})
	}
}
