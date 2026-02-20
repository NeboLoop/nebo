package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"

	"github.com/neboloop/nebo/extensions"
	agentcfg "github.com/neboloop/nebo/internal/agent/config"
	"github.com/neboloop/nebo/internal/agent/skills"
)

// skillsCmd creates the skills management command
func SkillsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "skills",
		Short: "Manage skill definitions",
		Long: `Skills are SKILL.md files that define AI capabilities.
They use YAML frontmatter for metadata and markdown body for instructions.

Skills are loaded from the Nebo data directory's skills/ folder or the extensions/skills/ directory.
Each skill should be in its own subdirectory with a SKILL.md file.`,
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List all loaded skills",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			listSkills(cfg)
		},
	})

	cmd.AddCommand(&cobra.Command{
		Use:   "show [name]",
		Short: "Show details of a skill",
		Args:  cobra.ExactArgs(1),
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			showSkill(cfg, args[0])
		},
	})

	return cmd
}

// listSkills lists all loaded skills
func listSkills(cfg *agentcfg.Config) {
	loader := createSkillLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading skills: %v\n", err)
		os.Exit(1)
	}

	skillList := loader.List()
	if len(skillList) == 0 {
		fmt.Println("No skills loaded.")
		fmt.Printf("\nSkills directory: %s\n", filepath.Join(cfg.DataDir, "skills"))
		fmt.Println("Create subdirectories with SKILL.md files to define skills.")
		return
	}

	fmt.Println("Loaded skills:")
	for _, s := range skillList {
		status := "\033[32m✓\033[0m"
		if !s.Enabled {
			status = "\033[31m✗\033[0m"
		}
		fmt.Printf("  %s %s (priority: %d)\n", status, s.Name, s.Priority)
		fmt.Printf("      %s\n", s.Description)
		if len(s.Tags) > 0 {
			fmt.Printf("      Tags: %s\n", strings.Join(s.Tags, ", "))
		}
		if len(s.Dependencies) > 0 {
			fmt.Printf("      Dependencies: %s\n", strings.Join(s.Dependencies, ", "))
		}
	}
}

// showSkill shows details of a specific skill
func showSkill(cfg *agentcfg.Config, name string) {
	loader := createSkillLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading skills: %v\n", err)
		os.Exit(1)
	}

	skill, ok := loader.Get(name)
	if !ok {
		fmt.Fprintf(os.Stderr, "Skill not found: %s\n", name)
		os.Exit(1)
	}

	fmt.Printf("Skill: %s\n", skill.Name)
	fmt.Printf("Version: %s\n", skill.Version)
	fmt.Printf("Description: %s\n", skill.Description)
	fmt.Printf("Priority: %d\n", skill.Priority)
	fmt.Printf("Enabled: %v\n", skill.Enabled)
	fmt.Printf("File: %s\n", skill.FilePath)
	if skill.Author != "" {
		fmt.Printf("Author: %s\n", skill.Author)
	}
	fmt.Println()

	if len(skill.Tags) > 0 {
		fmt.Println("Tags:")
		for _, t := range skill.Tags {
			fmt.Printf("  - %s\n", t)
		}
		fmt.Println()
	}

	if len(skill.Dependencies) > 0 {
		fmt.Println("Dependencies:")
		for _, d := range skill.Dependencies {
			fmt.Printf("  - %s\n", d)
		}
		fmt.Println()
	}

	if len(skill.Tools) > 0 {
		fmt.Println("Required tools:")
		for _, t := range skill.Tools {
			fmt.Printf("  - %s\n", t)
		}
		fmt.Println()
	}

	if skill.Template != "" {
		fmt.Println("Content (markdown body):")
		fmt.Println(skill.Template)
	}
}

func createSkillLoader(cfg *agentcfg.Config) *skills.Loader {
	loader := skills.NewLoader(filepath.Join(cfg.DataDir, "skills"))
	// Load bundled skills from embedded binary
	if err := loader.LoadFromEmbedFS(extensions.BundledSkills, "skills"); err != nil {
		fmt.Fprintf(os.Stderr, "Warning: failed to load bundled skills: %v\n", err)
	}
	return loader
}
