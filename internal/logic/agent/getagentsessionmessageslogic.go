package agent

import (
	"context"
	"time"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type GetAgentSessionMessagesLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get session messages
func NewGetAgentSessionMessagesLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetAgentSessionMessagesLogic {
	return &GetAgentSessionMessagesLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetAgentSessionMessagesLogic) GetAgentSessionMessages(req *types.GetAgentSessionRequest) (resp *types.GetAgentSessionMessagesResponse, err error) {
	messages, err := l.svcCtx.DB.GetSessionMessages(l.ctx, req.Id)
	if err != nil {
		return nil, err
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

	return &types.GetAgentSessionMessagesResponse{
		Messages: result,
	}, nil
}
