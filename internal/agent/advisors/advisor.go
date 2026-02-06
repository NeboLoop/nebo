// Package advisors provides a Markdown-based advisor definition and execution system.
// Advisors are internal "voices" that deliberate on tasks before the main agent decides.
// They do NOT speak to users, commit memory, or persist independently.
//
// Advisors use YAML frontmatter for metadata and the markdown body as the persona:
//
//	---
//	name: skeptic
//	role: critic
//	description: Challenges assumptions and identifies weaknesses
//	---
//
//	You are the Skeptic. Your role is to challenge ideas and find flaws...
package advisors

import (
	"bytes"
	"fmt"
	"strings"

	"gopkg.in/yaml.v3"
)

// AdvisorFileName is the expected filename for advisor definitions
const AdvisorFileName = "ADVISOR.md"

// Advisor represents a deliberation persona that provides internal critique.
// Advisors are short-lived, role-bound, and non-authoritative.
type Advisor struct {
	// Name is the unique identifier for the advisor (e.g., "skeptic", "pragmatist")
	Name string `yaml:"name"`

	// Role categorizes the advisor's perspective (e.g., "critic", "builder", "historian")
	Role string `yaml:"role"`

	// Description explains the advisor's purpose
	Description string `yaml:"description"`

	// Priority determines order when multiple advisors are invoked (higher = first)
	Priority int `yaml:"priority"`

	// Enabled allows disabling advisors without removing them
	Enabled bool `yaml:"enabled"`

	// MemoryAccess enables persistent memory recall for this advisor's deliberation.
	// When true, relevant memories are retrieved via hybrid search and injected into context.
	MemoryAccess bool `yaml:"memory_access"`

	// Persona is the markdown body - the system prompt that shapes this advisor's voice
	// This is NOT from YAML, it's parsed from the markdown body
	Persona string `yaml:"-"`

	// FilePath stores where this advisor was loaded from
	FilePath string `yaml:"-"`
}

// Response represents structured output from an advisor
type Response struct {
	AdvisorName string `json:"advisor_name"`
	Role        string `json:"role"`
	Critique    string `json:"critique"`    // The advisor's analysis/feedback
	Confidence  int    `json:"confidence"`  // 1-10 confidence in the critique
	Risks       string `json:"risks"`       // Identified risks (optional)
	Suggestion  string `json:"suggestion"`  // Recommended action (optional)
}

// Validate checks if the advisor definition is valid
func (a *Advisor) Validate() error {
	if a.Name == "" {
		return fmt.Errorf("advisor name is required")
	}
	if a.Description == "" {
		return fmt.Errorf("advisor %q: description is required", a.Name)
	}
	if a.Persona == "" {
		return fmt.Errorf("advisor %q: persona (markdown body) is required", a.Name)
	}
	return nil
}

// BuildSystemPrompt creates the full system prompt for this advisor's invocation
func (a *Advisor) BuildSystemPrompt(task string) string {
	return fmt.Sprintf(`%s

---

## Current Task

%s

---

## Response Format

Provide your analysis in a concise, structured format:

1. **Assessment**: Your main critique or observation (2-3 sentences)
2. **Confidence**: How confident are you in this assessment? (1-10)
3. **Risks**: What could go wrong? (optional, 1-2 sentences)
4. **Suggestion**: What action do you recommend? (optional, 1 sentence)

Be direct. No fluff. Focus on what matters.`, a.Persona, task)
}

// ParseAdvisorMD parses an ADVISOR.md file into an Advisor struct.
// The file format is YAML frontmatter (between --- markers) followed by markdown body.
func ParseAdvisorMD(data []byte) (*Advisor, error) {
	frontmatter, body, err := splitFrontmatter(data)
	if err != nil {
		return nil, err
	}

	var advisor Advisor
	if err := yaml.Unmarshal(frontmatter, &advisor); err != nil {
		return nil, fmt.Errorf("failed to parse frontmatter: %w", err)
	}

	// The markdown body IS the persona
	advisor.Persona = strings.TrimSpace(string(body))

	return &advisor, nil
}

// splitFrontmatter separates YAML frontmatter from markdown body.
// Frontmatter must be enclosed in --- markers at the start of the file.
func splitFrontmatter(data []byte) (frontmatter []byte, body []byte, err error) {
	// Must start with ---
	if !bytes.HasPrefix(data, []byte("---")) {
		return nil, nil, fmt.Errorf("ADVISOR.md must start with --- (YAML frontmatter)")
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
			return nil, nil, fmt.Errorf("ADVISOR.md missing closing --- for frontmatter")
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
