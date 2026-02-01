package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"

	agentcfg "nebo/agent/config"
	"nebo/agent/skills"
)

// skillsCmd creates the skills management command
func SkillsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "skills",
		Short: "Manage skill definitions",
		Long: `Skills are SKILL.md files that modify agent behavior without code changes.
They use YAML frontmatter for metadata and markdown body for instructions.

Skills are loaded from ~/.nebo/skills/ or the extensions/skills/ directory.
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

	cmd.AddCommand(&cobra.Command{
		Use:   "test [name] [input]",
		Short: "Test if a skill matches input",
		Args:  cobra.MinimumNArgs(2),
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			testSkill(cfg, args[0], strings.Join(args[1:], " "))
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
		fmt.Printf("\nSkills directory: %s\n", skillsDir(cfg))
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
		if len(s.Triggers) > 0 {
			fmt.Printf("      Triggers: %s\n", strings.Join(s.Triggers, ", "))
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
	fmt.Println()

	if len(skill.Triggers) > 0 {
		fmt.Println("Triggers:")
		for _, t := range skill.Triggers {
			fmt.Printf("  - %s\n", t)
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
		fmt.Println("Template (markdown body):")
		fmt.Println(skill.Template)
	}
}

// testSkill tests if a skill matches the given input
func testSkill(cfg *agentcfg.Config, name, input string) {
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

	if skill.Matches(input) {
		fmt.Printf("\033[32m✓ Skill '%s' matches input\033[0m\n", name)
		fmt.Println("\nPrompt would be modified with:")
		modified := skill.ApplyToPrompt("")
		fmt.Println(modified)
	} else {
		fmt.Printf("\033[31m✗ Skill '%s' does not match input\033[0m\n", name)
		fmt.Printf("\nTriggers: %s\n", strings.Join(skill.Triggers, ", "))
	}
}

func skillsDir(cfg *agentcfg.Config) string {
	userDir := filepath.Join(cfg.DataDir, "skills")
	if _, err := os.Stat(userDir); err == nil {
		return userDir
	}
	return "extensions/skills"
}

func createSkillLoader(cfg *agentcfg.Config) *skills.Loader {
	return skills.NewLoader(skillsDir(cfg))
}

func truncateString(s string, max int) string {
	s = strings.ReplaceAll(s, "\n", " ")
	if len(s) <= max {
		return s
	}
	return s[:max] + "..."
}
