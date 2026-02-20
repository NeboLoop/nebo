package extensions

import (
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	bundled "github.com/neboloop/nebo/extensions"
	"github.com/neboloop/nebo/internal/agent/skills"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// isBundledSkill checks if a skill exists in the embedded bundle.
func isBundledSkill(name string) bool {
	path := filepath.Join("skills", name, "SKILL.md")
	_, err := fs.Stat(bundled.BundledSkills, path)
	return err == nil
}

var slugRe = regexp.MustCompile(`^[a-z0-9][a-z0-9-]*[a-z0-9]$`)

// CreateSkillHandler creates a new user skill from raw SKILL.md content.
func CreateSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.CreateSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}
		if strings.TrimSpace(req.Content) == "" {
			httputil.BadRequest(w, "content is required")
			return
		}

		// Validate the SKILL.md content
		skill, err := skills.ParseSkillMD([]byte(req.Content))
		if err != nil {
			httputil.BadRequest(w, "invalid SKILL.md: "+err.Error())
			return
		}
		if err := skill.Validate(); err != nil {
			httputil.BadRequest(w, err.Error())
			return
		}

		// Determine slug
		slug := req.Slug
		if slug == "" {
			slug = tools.Slugify(skill.Name)
		}
		if len(slug) < 2 || !slugRe.MatchString(slug) {
			httputil.BadRequest(w, "invalid slug: must be lowercase alphanumeric with hyphens, at least 2 chars")
			return
		}

		// Write to user skills dir
		skillDir := filepath.Join(svcCtx.NeboDir, "skills", slug)
		skillPath := filepath.Join(skillDir, "SKILL.md")

		if _, err := os.Stat(skillPath); err == nil {
			httputil.ErrorWithCode(w, http.StatusConflict, "skill already exists: "+slug)
			return
		}

		if err := os.MkdirAll(skillDir, 0755); err != nil {
			httputil.InternalError(w, "failed to create skill directory")
			return
		}
		if err := os.WriteFile(skillPath, []byte(req.Content), 0644); err != nil {
			httputil.InternalError(w, "failed to write skill file")
			return
		}

		enabled := svcCtx.SkillSettings.IsEnabled(skill.Name)
		httputil.OkJSON(w, &types.CreateSkillResponse{
			Skill: types.ExtensionSkill{
				Name:         skill.Name,
				Description:  skill.Description,
				Version:      skill.Version,
				Tags:         skill.Tags,
				Dependencies: skill.Dependencies,
				Tools:        skill.Tools,
				Priority:     skill.Priority,
				Enabled:      enabled,
				FilePath:     skillPath,
				Source:       "user",
				Editable:     true,
			},
		})
	}
}

// UpdateSkillHandler updates an existing user skill's SKILL.md content.
func UpdateSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdateSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}
		if strings.TrimSpace(req.Content) == "" {
			httputil.BadRequest(w, "content is required")
			return
		}

		// Validate new content
		skill, err := skills.ParseSkillMD([]byte(req.Content))
		if err != nil {
			httputil.BadRequest(w, "invalid SKILL.md: "+err.Error())
			return
		}
		if err := skill.Validate(); err != nil {
			httputil.BadRequest(w, err.Error())
			return
		}

		// Only allow editing user skills
		userSkillPath := filepath.Join(svcCtx.NeboDir, "skills", req.Name, "SKILL.md")
		if _, err := os.Stat(userSkillPath); os.IsNotExist(err) {
			// Check if it's a bundled skill
			if isBundledSkill(req.Name) {
				httputil.ErrorWithCode(w, http.StatusForbidden, "cannot edit bundled skills")
				return
			}
			httputil.NotFound(w, "skill not found: "+req.Name)
			return
		}

		if err := os.WriteFile(userSkillPath, []byte(req.Content), 0644); err != nil {
			httputil.InternalError(w, "failed to write skill file")
			return
		}

		enabled := svcCtx.SkillSettings.IsEnabled(skill.Name)
		httputil.OkJSON(w, &types.UpdateSkillResponse{
			Skill: types.ExtensionSkill{
				Name:         skill.Name,
				Description:  skill.Description,
				Version:      skill.Version,
				Tags:         skill.Tags,
				Dependencies: skill.Dependencies,
				Tools:        skill.Tools,
				Priority:     skill.Priority,
				Enabled:      enabled,
				FilePath:     userSkillPath,
				Source:       "user",
				Editable:     true,
			},
		})
	}
}

// DeleteSkillHandler removes a user skill from disk.
func DeleteSkillHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.DeleteSkillRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Only allow deleting user skills
		userSkillDir := filepath.Join(svcCtx.NeboDir, "skills", req.Name)
		if _, err := os.Stat(filepath.Join(userSkillDir, "SKILL.md")); os.IsNotExist(err) {
			if isBundledSkill(req.Name) {
				httputil.ErrorWithCode(w, http.StatusForbidden, "cannot delete bundled skills")
				return
			}
			httputil.NotFound(w, "skill not found: "+req.Name)
			return
		}

		if err := os.RemoveAll(userSkillDir); err != nil {
			httputil.InternalError(w, "failed to delete skill")
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{Message: fmt.Sprintf("skill %s deleted", req.Name)})
	}
}

// GetSkillContentHandler returns the raw SKILL.md file content.
func GetSkillContentHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetSkillContentRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Check user skills first (higher priority), then bundled (embedded)
		userPath := filepath.Join(svcCtx.NeboDir, "skills", req.Name, "SKILL.md")
		embeddedPath := filepath.Join("skills", req.Name, "SKILL.md")

		var content []byte
		var editable bool
		var err error

		if _, statErr := os.Stat(userPath); statErr == nil {
			content, err = os.ReadFile(userPath)
			editable = true
		} else {
			content, err = fs.ReadFile(bundled.BundledSkills, embeddedPath)
			editable = false
		}

		if err != nil || content == nil {
			httputil.NotFound(w, "skill not found: "+req.Name)
			return
		}

		httputil.OkJSON(w, &types.GetSkillContentResponse{
			Content:  string(content),
			Editable: editable,
		})
	}
}
