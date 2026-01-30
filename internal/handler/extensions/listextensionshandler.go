package extensions

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/extensions"
	"gobot/internal/svc"
)

// List all extensions (tools, skills, plugins)
func ListExtensionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := extensions.NewListExtensionsLogic(r.Context(), svcCtx)
		resp, err := l.ListExtensions()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
