package chat

import (
	"database/sql"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/middleware"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"

	"github.com/google/uuid"
)

// Create new chat - Single Bot Paradigm: returns the companion chat instead of creating new ones
func CreateChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.CreateChatRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Single Bot Paradigm: Always return the companion chat
		// We don't create new chats - there is only ONE conversation with THE agent
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

		httputil.OkJSON(w, &types.CreateChatResponse{
			Chat: types.Chat{
				Id:        chat.ID,
				Title:     chat.Title,
				CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
			},
		})
	}
}
