package setup

import (
	"context"

	"gobot/internal/defaults"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type CompleteSetupLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Mark initial setup as complete
func NewCompleteSetupLogic(ctx context.Context, svcCtx *svc.ServiceContext) *CompleteSetupLogic {
	return &CompleteSetupLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *CompleteSetupLogic) CompleteSetup() (resp *types.CompleteSetupResponse, err error) {
	// Mark setup as complete by creating the .setup-complete file
	if err := defaults.MarkSetupComplete(); err != nil {
		l.Errorf("Failed to mark setup as complete: %v", err)
		return nil, err
	}

	return &types.CompleteSetupResponse{
		Success: true,
	}, nil
}
