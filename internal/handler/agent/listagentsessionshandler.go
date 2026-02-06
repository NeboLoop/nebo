package agent

import (
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
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
