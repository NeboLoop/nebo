package agent

import (
	"context"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ListAgentSessionsLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// List agent sessions
func NewListAgentSessionsLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ListAgentSessionsLogic {
	return &ListAgentSessionsLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ListAgentSessionsLogic) ListAgentSessions() (resp *types.ListAgentSessionsResponse, err error) {
	sessions, err := l.svcCtx.DB.ListSessions(l.ctx, db.ListSessionsParams{
		Limit:  100,
		Offset: 0,
	})
	if err != nil {
		return nil, err
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

	return &types.ListAgentSessionsResponse{
		Sessions: result,
		Total:    len(result),
	}, nil
}
