package chat

import (
	"database/sql"
	"errors"
	"net/http"
	"strings"
	"time"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/markdown"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"

	"github.com/google/uuid"
)

// Send message (creates chat if needed)
func SendMessageHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.SendMessageRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		chatID := req.ChatId

		// If no chat ID, create a new chat
		if chatID == "" {
			chatID = uuid.New().String()
			// Generate title from first message (truncate to 50 chars)
			title := req.Content
			if len(title) > 50 {
				title = title[:47] + "..."
			}
			title = strings.TrimSpace(title)
			if title == "" {
				title = "New Chat"
			}

			_, err := svcCtx.DB.CreateChat(ctx, db.CreateChatParams{
				ID:    chatID,
				Title: title,
			})
			if err != nil {
				logging.Errorf("Failed to create chat: %v", err)
				httputil.Error(w, err)
				return
			}
		} else {
			// Verify chat exists
			_, err := svcCtx.DB.GetChat(ctx, chatID)
			if err != nil {
				if errors.Is(err, sql.ErrNoRows) {
					httputil.Error(w, errors.New("chat not found"))
					return
				}
				httputil.Error(w, err)
				return
			}
		}

		// Create message
		role := req.Role
		if role == "" {
			role = "user"
		}
		messageID := uuid.New().String()
		msg, err := svcCtx.DB.CreateChatMessage(ctx, db.CreateChatMessageParams{
			ID:      messageID,
			ChatID:  chatID,
			Role:    role,
			Content: req.Content,
		})
		if err != nil {
			logging.Errorf("Failed to create message: %v", err)
			httputil.Error(w, err)
			return
		}

		// Update chat timestamp
		_ = svcCtx.DB.UpdateChatTimestamp(ctx, chatID)

		httputil.OkJSON(w, &types.SendMessageResponse{
			ChatId: chatID,
			Message: types.ChatMessage{
				Id:          msg.ID,
				ChatId:      msg.ChatID,
				Role:        msg.Role,
				Content:     msg.Content,
				ContentHtml: markdown.Render(msg.Content),
				CreatedAt:   time.Unix(msg.CreatedAt, 0).Format(time.RFC3339),
			},
		})
	}
}
