package agent

import (
	"net/http"
	"time"

	"gobot/internal/db"
	"gobot/internal/httputil"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// List agent sessions
func ListAgentSessionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		sessions, err := svcCtx.DB.ListSessions(ctx, db.ListSessionsParams{
			Limit:  100,
			Offset: 0,
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.AgentSession, 0, len(sessions))
		for _, s := range sessions {
			result = append(result, types.AgentSession{
				Id:           s.ID,
				Name:         s.Name.String,
				Summary:      s.Summary.String,
				MessageCount: int(s.MessageCount.Int64),
				CreatedAt:    time.Unix(s.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt:    time.Unix(s.UpdatedAt, 0).Format(time.RFC3339),
			})
		}

		httputil.OkJSON(w, &types.ListAgentSessionsResponse{
			Sessions: result,
			Total:    len(result),
		})
	}
}
