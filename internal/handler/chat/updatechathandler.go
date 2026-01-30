package chat

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/chat"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Update chat title
func UpdateChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateChatRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := chat.NewUpdateChatLogic(r.Context(), svcCtx)
		resp, err := l.UpdateChat(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
