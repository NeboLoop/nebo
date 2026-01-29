package setup

import (
	"context"
	"os"
	"path/filepath"

	"gobot/internal/defaults"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type UpdatePersonalityLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Update AI personality configuration
func NewUpdatePersonalityLogic(ctx context.Context, svcCtx *svc.ServiceContext) *UpdatePersonalityLogic {
	return &UpdatePersonalityLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *UpdatePersonalityLogic) UpdatePersonality(req *types.UpdatePersonalityRequest) (resp *types.UpdatePersonalityResponse, err error) {
	// Get data directory path
	dataDir, err := defaults.DataDir()
	if err != nil {
		l.Errorf("Failed to get data directory: %v", err)
		return nil, err
	}

	// Ensure directory exists
	if err := os.MkdirAll(dataDir, 0755); err != nil {
		l.Errorf("Failed to create data directory: %v", err)
		return nil, err
	}

	soulPath := filepath.Join(dataDir, "SOUL.md")

	// Write content to file
	if err := os.WriteFile(soulPath, []byte(req.Content), 0644); err != nil {
		l.Errorf("Failed to write SOUL.md: %v", err)
		return nil, err
	}

	return &types.UpdatePersonalityResponse{
		Success: true,
	}, nil
}
