package chat

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/chat"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Search chat messages
func SearchChatMessagesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.SearchChatMessagesRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		l := chat.NewSearchChatMessagesLogic(r.Context(), svcCtx)
		resp, err := l.SearchChatMessages(&req)
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
