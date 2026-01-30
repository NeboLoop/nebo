package chat

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/chat"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Delete chat
func DeleteChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.DeleteChatRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := chat.NewDeleteChatLogic(r.Context(), svcCtx)
		resp, err := l.DeleteChat(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
