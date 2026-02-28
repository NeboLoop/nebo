package agent

import (
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Get session messages — reads from chat_messages via session name → chat_id resolution.
func GetAgentSessionMessagesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.GetAgentSessionRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Resolve session ID to session name (sessionKey = chat_id)
		sess, err := svcCtx.DB.GetSession(ctx, req.Id)
		if err != nil {
			httputil.Error(w, err)
			return
		}
		chatID := sess.Name.String
		if chatID == "" {
			chatID = req.Id // fallback to raw ID
		}

		// Read from chat_messages using sessionKey as chat_id
		messages, err := svcCtx.DB.GetRecentChatMessagesWithTools(ctx, db.GetRecentChatMessagesWithToolsParams{
			ChatID: chatID,
			Limit:  100,
		})
		if err != nil {
			// Fallback: try GetChatMessages (no limit)
			messages2, err2 := svcCtx.DB.GetChatMessages(ctx, chatID)
			if err2 != nil {
				httputil.Error(w, err)
				return
			}
			result := make([]types.SessionMessage, 0, len(messages2))
			for _, m := range messages2 {
				result = append(result, types.SessionMessage{
					Id:        0,
					Role:      m.Role,
					Content:   m.Content,
					CreatedAt: time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
				})
			}
			httputil.OkJSON(w, &types.GetAgentSessionMessagesResponse{
				Messages: result,
				Total:    len(result),
			})
			return
		}

		result := make([]types.SessionMessage, 0, len(messages))
		for _, m := range messages {
			result = append(result, types.SessionMessage{
				Id:        0,
				Role:      m.Role,
				Content:   m.Content,
				CreatedAt: time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
			})
		}

		httputil.OkJSON(w, &types.GetAgentSessionMessagesResponse{
			Messages: result,
			Total:    len(result),
		})
	}
}
