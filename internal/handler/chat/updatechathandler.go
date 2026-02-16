package chat

import (
	"database/sql"
	"errors"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Update chat title
func UpdateChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.UpdateChatRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get chat
		chat, err := svcCtx.DB.GetChat(ctx, req.Id)
		if err != nil {
			if errors.Is(err, sql.ErrNoRows) {
				httputil.Error(w, errors.New("chat not found"))
				return
			}
			httputil.Error(w, err)
			return
		}

		// Update title
		err = svcCtx.DB.UpdateChatTitle(ctx, db.UpdateChatTitleParams{
			Title: req.Title,
			ID:    req.Id,
		})
		if err != nil {
			logging.Errorf("Failed to update chat: %v", err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.Chat{
			Id:        chat.ID,
			Title:     req.Title,
			CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
			UpdatedAt: time.Now().Format(time.RFC3339),
		})
	}
}
