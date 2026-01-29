package agent

import (
	"context"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ListAgentsLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// List connected agents
func NewListAgentsLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ListAgentsLogic {
	return &ListAgentsLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ListAgentsLogic) ListAgents(req *types.ListAgentsRequest) (resp *types.ListAgentsResponse, err error) {
	// Get agents from the hub
	hub := l.svcCtx.AgentHub
	if hub == nil {
		return &types.ListAgentsResponse{
			Agents: []types.AgentInfo{},
			Total:  0,
		}, nil
	}

	agents := hub.GetAllAgents()
	agentInfos := make([]types.AgentInfo, 0, len(agents))

	for _, agent := range agents {
		agentInfos = append(agentInfos, types.AgentInfo{
			AgentId:   agent.ID,
			Connected: true,
			CreatedAt: agent.CreatedAt.Format("2006-01-02T15:04:05Z"),
		})
	}

	return &types.ListAgentsResponse{
		Agents: agentInfos,
		Total:  len(agentInfos),
	}, nil
}
