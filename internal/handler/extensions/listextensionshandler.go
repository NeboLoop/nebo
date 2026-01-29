package extensions

import (
	"net/http"

	"github.com/zeromicro/go-zero/rest/httpx"
	"gobot/internal/logic/extensions"
	"gobot/internal/svc"
)

// List all extensions (tools, skills, plugins)
func ListExtensionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := extensions.NewListExtensionsLogic(r.Context(), svcCtx)
		resp, err := l.ListExtensions()
		if err != nil {
			httpx.ErrorCtx(r.Context(), w, err)
		} else {
			httpx.OkJsonCtx(r.Context(), w, resp)
		}
	}
}
