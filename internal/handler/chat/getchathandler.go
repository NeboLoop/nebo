package chat

import (
	"database/sql"
	"errors"
	"net/http"
	"time"

	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Get chat with messages
func GetChatHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.GetChatRequest
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
			logging.Errorf("Failed to get chat: %v", err)
			httputil.Error(w, err)
			return
		}

		// Get messages
		messages, err := svcCtx.DB.GetChatMessages(ctx, req.Id)
		if err != nil {
			logging.Errorf("Failed to get messages: %v", err)
			httputil.Error(w, err)
			return
		}

		msgList := make([]types.ChatMessage, len(messages))
		for i, m := range messages {
			metadata := ""
			if m.Metadata.Valid {
				metadata = m.Metadata.String
			}
			msgList[i] = types.ChatMessage{
				Id:        m.ID,
				ChatId:    m.ChatID,
				Role:      m.Role,
				Content:   m.Content,
				Metadata:  metadata,
				CreatedAt: time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
			}
		}

		httputil.OkJSON(w, &types.GetChatResponse{
			Chat: types.Chat{
				Id:        chat.ID,
				Title:     chat.Title,
				CreatedAt: time.Unix(chat.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt: time.Unix(chat.UpdatedAt, 0).Format(time.RFC3339),
			},
			Messages: msgList,
		})
	}
}
