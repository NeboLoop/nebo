// Package skills provides a Markdown-based skill definition and loading system.
// Skills are the unified abstraction for AI capabilities — whether backed by
// a .napp binary or standalone orchestration guidance.
//
// Skills use YAML frontmatter for metadata and the markdown body as content:
//
//	---
//	name: my-skill
//	description: Does something useful
//	version: "1.0.0"
//	---
//
//	# My Skill
//
//	Instructions for the agent...
package skills

import (
	"bytes"
	"fmt"

	"gopkg.in/yaml.v3"
)

// Skill represents a skill definition parsed from a SKILL.md file.
type Skill struct {
	// Name is the unique identifier for the skill
	Name string `yaml:"name"`

	// Description explains what the skill does (one-liner for catalog)
	Description string `yaml:"description"`

	// Version for tracking skill updates
	Version string `yaml:"version"`

	// Author of the skill (for future ecosystem/marketplace)
	Author string `yaml:"author"`

	// Dependencies lists required skill names that must be installed
	Dependencies []string `yaml:"dependencies"`

	// Tags for categorization and discovery
	Tags []string `yaml:"tags"`

	// Platform lists supported platforms (macos, linux, windows).
	// Empty means all platforms (cross-platform).
	Platform []string `yaml:"platform"`

	// Triggers are phrases that auto-activate this skill when matched in user messages
	Triggers []string `yaml:"triggers"`

	// Tools lists required tool names for this skill
	Tools []string `yaml:"tools"`

	// Priority determines precedence (higher = first)
	Priority int `yaml:"priority"`

	// MaxTurns is how many turns of inactivity before auto-expiring.
	// 0 means use the system default.
	MaxTurns int `yaml:"max_turns"`

	// Metadata holds additional data
	Metadata map[string]any `yaml:"metadata"`

	// Template is the markdown body — the actual skill instructions
	// This is NOT from YAML, it's parsed from the markdown body
	Template string `yaml:"-"`

	// Enabled allows disabling skills without removing them
	Enabled bool `yaml:"-"`

	// FilePath stores where this skill was loaded from
	FilePath string `yaml:"-"`
}

// Validate checks if the skill definition is valid
func (s *Skill) Validate() error {
	if s.Name == "" {
		return fmt.Errorf("skill name is required")
	}
	if s.Description == "" {
		return fmt.Errorf("skill %q: description is required", s.Name)
	}
	return nil
}

// ParseSkillMD parses a SKILL.md file into a Skill struct.
// The file format is YAML frontmatter (between --- markers) followed by markdown body.
func ParseSkillMD(data []byte) (*Skill, error) {
	frontmatter, body, err := splitFrontmatter(data)
	if err != nil {
		return nil, err
	}

	var skill Skill
	if err := yaml.Unmarshal(frontmatter, &skill); err != nil {
		return nil, fmt.Errorf("failed to parse frontmatter: %w", err)
	}

	// The markdown body IS the template
	skill.Template = string(bytes.TrimSpace(body))

	return &skill, nil
}

// splitFrontmatter separates YAML frontmatter from markdown body.
// Frontmatter must be enclosed in --- markers at the start of the file.
func splitFrontmatter(data []byte) (frontmatter []byte, body []byte, err error) {
	// Must start with ---
	if !bytes.HasPrefix(data, []byte("---")) {
		return nil, nil, fmt.Errorf("SKILL.md must start with --- (YAML frontmatter)")
	}

	// Find the closing ---
	rest := data[3:] // Skip opening ---

	// Skip any whitespace/newline after opening ---
	rest = bytes.TrimLeft(rest, " \t")
	if len(rest) > 0 && rest[0] == '\n' {
		rest = rest[1:]
	} else if len(rest) > 1 && rest[0] == '\r' && rest[1] == '\n' {
		rest = rest[2:]
	}

	// Find closing ---
	closingIdx := bytes.Index(rest, []byte("\n---"))
	if closingIdx == -1 {
		// Try with \r\n
		closingIdx = bytes.Index(rest, []byte("\r\n---"))
		if closingIdx == -1 {
			return nil, nil, fmt.Errorf("SKILL.md missing closing --- for frontmatter")
		}
	}

	frontmatter = rest[:closingIdx]

	// Body starts after the closing ---
	body = rest[closingIdx+4:] // +4 for \n---

	// Skip any whitespace/newline after closing ---
	body = bytes.TrimLeft(body, " \t")
	if len(body) > 0 && body[0] == '\n' {
		body = body[1:]
	} else if len(body) > 1 && body[0] == '\r' && body[1] == '\n' {
		body = body[2:]
	}

	return frontmatter, body, nil
}
