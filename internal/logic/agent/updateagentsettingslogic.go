package agent

import (
	"context"

	"gobot/internal/agenthub"
	"gobot/internal/local"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type UpdateAgentSettingsLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Update agent settings
func NewUpdateAgentSettingsLogic(ctx context.Context, svcCtx *svc.ServiceContext) *UpdateAgentSettingsLogic {
	return &UpdateAgentSettingsLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *UpdateAgentSettingsLogic) UpdateAgentSettings(req *types.UpdateAgentSettingsRequest) (resp *types.GetAgentSettingsResponse, err error) {
	// Update local settings store
	settings := local.AgentSettings{
		AutonomousMode:   req.AutonomousMode,
		AutoApproveRead:  req.AutoApproveRead,
		AutoApproveWrite: req.AutoApproveWrite,
		AutoApproveBash:  req.AutoApproveBash,
	}

	if err := l.svcCtx.AgentSettings.Update(settings); err != nil {
		return nil, err
	}

	// Broadcast settings to all connected agents
	frame := &agenthub.Frame{
		Type:   "event",
		Method: "settings_updated",
		Payload: map[string]any{
			"autonomousMode":   settings.AutonomousMode,
			"autoApproveRead":  settings.AutoApproveRead,
			"autoApproveWrite": settings.AutoApproveWrite,
			"autoApproveBash":  settings.AutoApproveBash,
		},
	}

	// Broadcast to all connected agents
	l.svcCtx.AgentHub.Broadcast(frame)

	logx.Infof("Agent settings updated and broadcast: autonomous=%v", settings.AutonomousMode)

	return &types.GetAgentSettingsResponse{
		Settings: types.AgentSettings{
			AutonomousMode:   settings.AutonomousMode,
			AutoApproveRead:  settings.AutoApproveRead,
			AutoApproveWrite: settings.AutoApproveWrite,
			AutoApproveBash:  settings.AutoApproveBash,
		},
	}, nil
}
