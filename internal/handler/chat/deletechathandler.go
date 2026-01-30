package chat

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Delete chat
func DeleteChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.DeleteChatRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Delete (cascade will remove messages)
		err := svcCtx.DB.DeleteChat(ctx, req.Id)
		if err != nil {
			logging.Errorf("Failed to delete chat: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Chat deleted",
		})
	}
}
