package extensions

import (
	"context"
	"fmt"
	"path/filepath"

	"gobot/agent/skills"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type GetSkillLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get single skill details
func NewGetSkillLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetSkillLogic {
	return &GetSkillLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetSkillLogic) GetSkill(req *types.GetSkillRequest) (resp *types.GetSkillResponse, err error) {
	// Load skills from extensions/skills directory
	extensionsDir := "extensions"
	skillsDir := filepath.Join(extensionsDir, "skills")
	skillLoader := skills.NewLoader(skillsDir)
	if err := skillLoader.LoadAll(); err != nil {
		l.Errorf("Failed to load skills: %v", err)
		return nil, fmt.Errorf("failed to load skills: %w", err)
	}

	// Find the requested skill
	skill, found := skillLoader.Get(req.Name)
	if !found {
		return nil, fmt.Errorf("skill not found: %s", req.Name)
	}

	// Check enabled state from persistent settings
	enabled := l.svcCtx.SkillSettings.IsEnabled(skill.Name)

	return &types.GetSkillResponse{
		Skill: types.ExtensionSkill{
			Name:        skill.Name,
			Description: skill.Description,
			Version:     skill.Version,
			Triggers:    skill.Triggers,
			Tools:       skill.Tools,
			Priority:    skill.Priority,
			Enabled:     enabled,
			FilePath:    skill.FilePath,
		},
	}, nil
}
