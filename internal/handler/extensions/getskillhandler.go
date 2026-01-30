package extensions

import (
	"fmt"
	"net/http"
	"path/filepath"

	"nebo/agent/skills"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Get single skill details
func GetSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Load skills from extensions/skills directory
		extensionsDir := "extensions"
		skillsDir := filepath.Join(extensionsDir, "skills")
		skillLoader := skills.NewLoader(skillsDir)
		if err := skillLoader.LoadAll(); err != nil {
			logging.Errorf("Failed to load skills: %v", err)
			httputil.Error(w, fmt.Errorf("failed to load skills: %w", err))
			return
		}

		// Find the requested skill
		skill, found := skillLoader.Get(req.Name)
		if !found {
			httputil.Error(w, fmt.Errorf("skill not found: %s", req.Name))
			return
		}

		// Check enabled state from persistent settings
		enabled := svcCtx.SkillSettings.IsEnabled(skill.Name)

		httputil.OkJSON(w, &types.GetSkillResponse{
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
		})
	}
}
