package provider

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/provider"
	"nebo/internal/svc"
	"nebo/internal/types"
)

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
