package extensions

import (
	"net/http"
	"os"
	"path/filepath"
	"strings"

	bundled "github.com/neboloop/nebo/extensions"
	"github.com/neboloop/nebo/internal/agent/skills"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// List all extensions (tools, skills, plugins)
func ListExtensionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		resp := &types.ListExtensionsResponse{
			Tools:    []types.ExtensionTool{},
			Skills:   []types.ExtensionSkill{},
			Channels: []types.ExtensionChannel{},
		}

		// Get extensions directory - check for extensions/ in working directory
		extensionsDir := "extensions"

		// Built-in tools (hardcoded list since we can't access the registry directly)
		builtinTools := []types.ExtensionTool{
			{Name: "bash", Description: "Execute shell commands", RequiresApproval: true, IsPlugin: false},
			{Name: "read", Description: "Read file contents", RequiresApproval: false, IsPlugin: false},
			{Name: "write", Description: "Write/create files", RequiresApproval: true, IsPlugin: false},
			{Name: "edit", Description: "Find-and-replace edits", RequiresApproval: true, IsPlugin: false},
			{Name: "glob", Description: "Find files by pattern", RequiresApproval: false, IsPlugin: false},
			{Name: "grep", Description: "Search file contents", RequiresApproval: false, IsPlugin: false},
			{Name: "web", Description: "Fetch URLs", RequiresApproval: false, IsPlugin: false},
			{Name: "search", Description: "Web search", RequiresApproval: false, IsPlugin: false},
			{Name: "browser", Description: "Browser automation", RequiresApproval: true, IsPlugin: false},
			{Name: "screenshot", Description: "Desktop capture", RequiresApproval: false, IsPlugin: false},
			{Name: "vision", Description: "Image analysis", RequiresApproval: false, IsPlugin: false},
			{Name: "memory", Description: "Persistent facts storage", RequiresApproval: false, IsPlugin: false},
			{Name: "process", Description: "Process management", RequiresApproval: true, IsPlugin: false},
			{Name: "task", Description: "Spawn sub-agents", RequiresApproval: false, IsPlugin: false},
		}
		resp.Tools = append(resp.Tools, builtinTools...)

		// Load bundled skills from embedded filesystem
		skillsByName := make(map[string]types.ExtensionSkill)
		bundledLoader := skills.NewLoader("")
		if err := bundledLoader.LoadFromEmbedFS(bundled.BundledSkills, "skills"); err != nil {
			logging.Errorf("Failed to load bundled skills: %v", err)
		} else {
			for _, skill := range bundledLoader.List() {
				enabled := svcCtx.SkillSettings.IsEnabled(skill.Name)
				skillsByName[skill.Name] = types.ExtensionSkill{
					Name:         skill.Name,
					Description:  skill.Description,
					Version:      skill.Version,
					Tags:         skill.Tags,
					Dependencies: skill.Dependencies,
					Tools:        skill.Tools,
					Priority:     skill.Priority,
					Enabled:      enabled,
					FilePath:     skill.FilePath,
					Source:       "bundled",
					Editable:     false,
				}
			}
		}

		// Load user skills (override bundled on name collision)
		userSkillsDir := filepath.Join(svcCtx.NeboDir, "skills")
		userLoader := skills.NewLoader(userSkillsDir)
		if err := userLoader.LoadAll(); err == nil {
			for _, skill := range userLoader.List() {
				enabled := svcCtx.SkillSettings.IsEnabled(skill.Name)
				skillsByName[skill.Name] = types.ExtensionSkill{
					Name:         skill.Name,
					Description:  skill.Description,
					Version:      skill.Version,
					Tags:         skill.Tags,
					Dependencies: skill.Dependencies,
					Tools:        skill.Tools,
					Priority:     skill.Priority,
					Enabled:      enabled,
					FilePath:     skill.FilePath,
					Source:       "user",
					Editable:     true,
				}
			}
		}
		for _, s := range skillsByName {
			resp.Skills = append(resp.Skills, s)
		}

		// Load plugin tools from extensions/tools
		toolsDir := filepath.Join(extensionsDir, "tools")
		if entries, err := os.ReadDir(toolsDir); err == nil {
			for _, entry := range entries {
				if entry.IsDir() {
					// Each subdirectory is a tool plugin
					pluginPath := filepath.Join(toolsDir, entry.Name())
					// Look for executable file
					if subEntries, err := os.ReadDir(pluginPath); err == nil {
						for _, subEntry := range subEntries {
							if !subEntry.IsDir() && !strings.HasSuffix(subEntry.Name(), ".go") &&
								!strings.HasSuffix(subEntry.Name(), ".mod") &&
								!strings.HasSuffix(subEntry.Name(), ".sum") {
								execPath := filepath.Join(pluginPath, subEntry.Name())
								if info, err := os.Stat(execPath); err == nil && info.Mode()&0111 != 0 {
									resp.Tools = append(resp.Tools, types.ExtensionTool{
										Name:             entry.Name(),
										Description:      "Plugin: " + entry.Name(),
										RequiresApproval: true,
										IsPlugin:         true,
										Path:             execPath,
									})
									break
								}
							}
						}
					}
				}
			}
		}

		// Load channel plugins from extensions/plugins/channels
		channelsDir := filepath.Join(extensionsDir, "plugins", "channels")
		if entries, err := os.ReadDir(channelsDir); err == nil {
			for _, entry := range entries {
				if !entry.IsDir() {
					execPath := filepath.Join(channelsDir, entry.Name())
					if info, err := os.Stat(execPath); err == nil && info.Mode()&0111 != 0 {
						resp.Channels = append(resp.Channels, types.ExtensionChannel{
							Id:   entry.Name(),
							Path: execPath,
						})
					}
				}
			}
		}

		httputil.OkJSON(w, resp)
	}
}
