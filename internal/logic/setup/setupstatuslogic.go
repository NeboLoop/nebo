package setup

import (
	"context"

	"gobot/internal/defaults"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type SetupStatusLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Check if setup is required (no admin exists)
func NewSetupStatusLogic(ctx context.Context, svcCtx *svc.ServiceContext) *SetupStatusLogic {
	return &SetupStatusLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *SetupStatusLogic) SetupStatus() (resp *types.SetupStatusResponse, err error) {
	// Check if any admin user exists
	hasAdmin, err := l.svcCtx.DB.HasAdminUser(l.ctx)
	if err != nil {
		l.Errorf("Failed to check for admin user: %v", err)
		return nil, err
	}

	// Check if setup has been marked as complete
	setupComplete, err := defaults.IsSetupComplete()
	if err != nil {
		l.Errorf("Failed to check setup complete status: %v", err)
		// Non-fatal error, assume setup is not complete
		setupComplete = false
	}

	return &types.SetupStatusResponse{
		SetupRequired: hasAdmin == 0,
		HasAdmin:      hasAdmin == 1,
		SetupComplete: setupComplete,
	}, nil
}
