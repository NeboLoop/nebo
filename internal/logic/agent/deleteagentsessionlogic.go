package agent

import (
	"context"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type DeleteAgentSessionLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Delete agent session
func NewDeleteAgentSessionLogic(ctx context.Context, svcCtx *svc.ServiceContext) *DeleteAgentSessionLogic {
	return &DeleteAgentSessionLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *DeleteAgentSessionLogic) DeleteAgentSession(req *types.DeleteAgentSessionRequest) (resp *types.MessageResponse, err error) {
	err = l.svcCtx.DB.DeleteSession(l.ctx, req.Id)
	if err != nil {
		return nil, err
	}

	return &types.MessageResponse{
		Message: "Session deleted successfully",
	}, nil
}
