package agent

import (
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Get session messages
func GetAgentSessionMessagesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.GetAgentSessionRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		messages, err := svcCtx.DB.GetSessionMessages(ctx, req.Id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.SessionMessage, 0, len(messages))
		for _, m := range messages {
			result = append(result, types.SessionMessage{
				Id:        int(m.ID),
				Role:      m.Role,
				Content:   m.Content.String,
				CreatedAt: time.Unix(m.CreatedAt, 0).Format(time.RFC3339),
			})
		}

		httputil.OkJSON(w, &types.GetAgentSessionMessagesResponse{
			Messages: result,
			Total:    len(result),
		})
	}
}
