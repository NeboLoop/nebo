package extensions

import (
	"fmt"
	"net/http"
	"path/filepath"

	"github.com/neboloop/nebo/internal/agent/skills"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Get single skill details
func GetSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Search user skills first, then bundled
		var skill *skills.Skill
		var source string

		userLoader := skills.NewLoader(filepath.Join(svcCtx.NeboDir, "skills"))
		if err := userLoader.LoadAll(); err == nil {
			if s, ok := userLoader.Get(req.Name); ok {
				skill = s
				source = "user"
			}
		}

		if skill == nil {
			bundledLoader := skills.NewLoader(filepath.Join("extensions", "skills"))
			if err := bundledLoader.LoadAll(); err != nil {
				logging.Errorf("Failed to load skills: %v", err)
				httputil.Error(w, fmt.Errorf("failed to load skills: %w", err))
				return
			}
			if s, ok := bundledLoader.Get(req.Name); ok {
				skill = s
				source = "bundled"
			}
		}

		if skill == nil {
			httputil.NotFound(w, "skill not found: "+req.Name)
			return
		}

		enabled := svcCtx.SkillSettings.IsEnabled(skill.Name)

		httputil.OkJSON(w, &types.GetSkillResponse{
			Skill: types.ExtensionSkill{
				Name:         skill.Name,
				Description:  skill.Description,
				Version:      skill.Version,
				Tags:         skill.Tags,
				Dependencies: skill.Dependencies,
				Tools:        skill.Tools,
				Priority:     skill.Priority,
				Enabled:      enabled,
				FilePath:     skill.FilePath,
				Source:       source,
				Editable:     source == "user",
			},
		})
	}
}
