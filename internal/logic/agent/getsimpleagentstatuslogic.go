package agent

import (
	"context"
	"time"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type GetSimpleAgentStatusLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get simple agent status (single agent model)
func NewGetSimpleAgentStatusLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetSimpleAgentStatusLogic {
	return &GetSimpleAgentStatusLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetSimpleAgentStatusLogic) GetSimpleAgentStatus() (resp *types.SimpleAgentStatusResponse, err error) {
	hub := l.svcCtx.AgentHub
	if hub == nil {
		return &types.SimpleAgentStatusResponse{
			Connected: false,
		}, nil
	}

	// Get any connected agent (single agent model)
	agent := hub.GetAnyAgent()
	if agent == nil {
		return &types.SimpleAgentStatusResponse{
			Connected: false,
		}, nil
	}

	uptime := int64(time.Since(agent.CreatedAt).Seconds())
	return &types.SimpleAgentStatusResponse{
		Connected: true,
		AgentId:   agent.ID,
		Uptime:    uptime,
	}, nil
}
