package provider

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/provider"
	"gobot/internal/svc"
)

// List all available models from YAML cache
func ListModelsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := provider.NewListModelsLogic(r.Context(), svcCtx)
		resp, err := l.ListModels()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
