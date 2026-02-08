// Package skills provides a Markdown-based skill definition and loading system.
// Skills are declarative definitions that modify agent behavior without requiring
// compiled code - just SKILL.md files that can be hot-reloaded.
//
// Skills use YAML frontmatter for metadata and the markdown body as the template:
//
//	---
//	name: my-skill
//	description: Does something useful
//	triggers:
//	  - keyword
//	---
//
//	# My Skill
//
//	Instructions for the agent...
package skills

import (
	"bytes"
	"fmt"
	"strings"

	"gopkg.in/yaml.v3"
)

// Skill represents a declarative skill definition that modifies agent behavior.
// Skills can add context to prompts, require specific tools, and provide examples.
type Skill struct {
	// Name is the unique identifier for the skill
	Name string `yaml:"name"`

	// Description explains what the skill does
	Description string `yaml:"description"`

	// Version for tracking skill updates
	Version string `yaml:"version"`

	// Triggers are keywords/phrases that activate this skill
	Triggers []string `yaml:"triggers"`

	// Tools lists required tool names for this skill
	Tools []string `yaml:"tools"`

	// Priority determines precedence when multiple skills match (higher = first)
	Priority int `yaml:"priority"`

	// Metadata holds additional data (emoji, requires, install)
	Metadata map[string]any `yaml:"metadata"`

	// Template is the markdown body - the actual skill instructions
	// This is NOT from YAML, it's parsed from the markdown body
	Template string `yaml:"-"`

	// Enabled allows disabling skills without removing them
	Enabled bool `yaml:"-"`

	// FilePath stores where this skill was loaded from
	FilePath string `yaml:"-"`
}

// Matches checks if the given input text triggers this skill
func (s *Skill) Matches(input string) bool {
	if !s.Enabled {
		return false
	}

	inputLower := strings.ToLower(input)
	for _, trigger := range s.Triggers {
		if strings.Contains(inputLower, strings.ToLower(trigger)) {
			return true
		}
	}
	return false
}

// ApplyToPrompt modifies the system prompt with skill-specific content
func (s *Skill) ApplyToPrompt(systemPrompt string) string {
	if s.Template == "" {
		return systemPrompt
	}

	var sb strings.Builder
	sb.WriteString(systemPrompt)

	// Add skill template (the markdown body)
	sb.WriteString("\n\n## Skill: ")
	sb.WriteString(s.Name)
	sb.WriteString("\n\n")
	sb.WriteString(s.Template)

	return sb.String()
}

// RequiredTools returns the list of tools this skill needs
func (s *Skill) RequiredTools() []string {
	return s.Tools
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
	skill.Template = strings.TrimSpace(string(body))

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
