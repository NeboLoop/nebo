package appui

import (
	"encoding/json"
	"net/http"

	"github.com/neboloop/nebo/internal/apps"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
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

// GetUIViewHandler returns the current UI view for an app.
// GET /apps/{id}/ui
func GetUIViewHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := httputil.PathVar(r, "id")
		p := svcCtx.AppUI()
		if p == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app system not ready")
			return
		}
		view, err := p.GetUIView(r.Context(), appID)
		if err != nil {
			httputil.Error(w, err)
			return
		}
		httputil.OkJSON(w, view)
	}
}

// SendUIEventHandler sends a user interaction event to a UI app.
// POST /apps/{id}/ui/event
func SendUIEventHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := httputil.PathVar(r, "id")
		p := svcCtx.AppUI()
		if p == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "app system not ready")
			return
		}

		var payload apps.UIEventPayload
		if err := json.NewDecoder(r.Body).Decode(&payload); err != nil {
			httputil.Error(w, err)
			return
		}

		resp, err := p.SendUIEvent(r.Context(), appID, &payload)
		if err != nil {
			httputil.Error(w, err)
			return
		}
		httputil.OkJSON(w, resp)
	}
}
