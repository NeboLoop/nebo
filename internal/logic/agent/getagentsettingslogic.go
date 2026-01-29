package agent

import (
	"context"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type GetAgentSettingsLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get agent settings
func NewGetAgentSettingsLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetAgentSettingsLogic {
	return &GetAgentSettingsLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetAgentSettingsLogic) GetAgentSettings() (resp *types.GetAgentSettingsResponse, err error) {
	settings := l.svcCtx.AgentSettings.Get()

	return &types.GetAgentSettingsResponse{
		Settings: types.AgentSettings{
			AutonomousMode:   settings.AutonomousMode,
			AutoApproveRead:  settings.AutoApproveRead,
			AutoApproveWrite: settings.AutoApproveWrite,
			AutoApproveBash:  settings.AutoApproveBash,
		},
	}, nil
}
