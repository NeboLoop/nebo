package chat

import (
	"database/sql"
	"net/http"
	"time"

	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/middleware"
	"nebo/internal/svc"
	"nebo/internal/types"

	"github.com/google/uuid"
)

// List chats - Single Bot Paradigm: returns only the companion chat
func ListChatsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.ListChatsRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Single Bot Paradigm: Return only the companion chat
		// There is only ONE conversation with THE agent
		userID := middleware.GetUserID(ctx)
		if userID == "" {
			userID = companionUserIDFallback
		}

		chat, err := svcCtx.DB.GetOrCreateCompanionChat(ctx, db.GetOrCreateCompanionChatParams{
			ID:     uuid.New().String(),
			UserID: sql.NullString{String: userID, Valid: true},
		})
		if err != nil {
			logging.Errorf("Failed to get companion chat: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.ListChatsResponse{
			Chats: []types.Chat{
				{
					Id:        chat.ID,
					Title:     chat.Title,
					CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
					UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
				},
			},
			Total: 1,
		})
	}
}
