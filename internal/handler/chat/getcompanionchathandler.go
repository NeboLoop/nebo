package chat

import (
	"net/http"

	"gobot/internal/httputil"
	"gobot/internal/logic/chat"
	"gobot/internal/svc"
)

// Get companion chat (auto-creates if needed)
func GetCompanionChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		l := chat.NewGetCompanionChatLogic(r.Context(), svcCtx)
		resp, err := l.GetCompanionChat()
		if err != nil {
			httputil.Error(w, err)
		} else {
			httputil.OkJSON(w, resp)
		}
	}
}
