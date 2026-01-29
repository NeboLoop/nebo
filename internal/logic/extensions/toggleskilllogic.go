package extensions

import (
	"context"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ToggleSkillLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Toggle skill enabled/disabled
func NewToggleSkillLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ToggleSkillLogic {
	return &ToggleSkillLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ToggleSkillLogic) ToggleSkill(req *types.ToggleSkillRequest) (resp *types.ToggleSkillResponse, err error) {
	// Toggle the skill's enabled state in persistent storage
	enabled, err := l.svcCtx.SkillSettings.Toggle(req.Name)
	if err != nil {
		return nil, err
	}

	return &types.ToggleSkillResponse{
		Name:    req.Name,
		Enabled: enabled,
	}, nil
}
