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

type GetPersonalityLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get AI personality configuration
func NewGetPersonalityLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetPersonalityLogic {
	return &GetPersonalityLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetPersonalityLogic) GetPersonality() (resp *types.GetPersonalityResponse, err error) {
	// Get data directory path
	dataDir, err := defaults.DataDir()
	if err != nil {
		l.Errorf("Failed to get data directory: %v", err)
		return nil, err
	}

	soulPath := filepath.Join(dataDir, "SOUL.md")

	// Try to read existing file
	content, err := os.ReadFile(soulPath)
	if err != nil {
		if os.IsNotExist(err) {
			// File doesn't exist, return default content
			defaultContent, defaultErr := defaults.GetDefault("SOUL.md")
			if defaultErr != nil {
				l.Errorf("Failed to get default SOUL.md: %v", defaultErr)
				return nil, defaultErr
			}
			return &types.GetPersonalityResponse{
				Content: string(defaultContent),
			}, nil
		}
		l.Errorf("Failed to read SOUL.md: %v", err)
		return nil, err
	}

	return &types.GetPersonalityResponse{
		Content: string(content),
	}, nil
}
