package chat

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/chat"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get chat with messages
func GetChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetChatRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := chat.NewGetChatLogic(r.Context(), svcCtx)
		resp, err := l.GetChat(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
