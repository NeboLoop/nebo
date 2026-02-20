package provider

import (
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/provider"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Update CLI provider active status
func UpdateCLIProviderHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateCLIProviderRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}
		if req.Active == nil {
			httputil.BadRequest(w, "active field is required")
			return
		}

		if err := provider.SetCLIProviderActive(req.CliId, *req.Active); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "CLI provider " + req.CliId + " updated",
		})
	}
}

// Update model settings (active, kind, preferred)
func UpdateModelHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateModelRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		update := provider.ModelUpdate{}

		// Only set fields that were provided (nil means not sent)
		if req.Active != nil {
			update.Active = req.Active
		}
		if req.Kind != nil {
			update.Kind = req.Kind
		}
		if req.Preferred != nil {
			update.Preferred = req.Preferred
		}

		if err := provider.UpdateModel(req.Provider, req.ModelId, update); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Model " + req.ModelId + " updated",
		})
	}
}
