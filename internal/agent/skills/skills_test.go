package skills

import (
	"os"
	"path/filepath"
	"testing"
)

func TestSkillMatches(t *testing.T) {
	skill := &Skill{
		Name:        "test-skill",
		Description: "A test skill",
		Triggers:    []string{"review", "check code"},
		Enabled:     true,
	}

	tests := []struct {
		input   string
		matches bool
	}{
		{"please review my code", true},
		{"can you check code for bugs", true},
		{"REVIEW this file", true}, // Case insensitive
		{"hello world", false},
		{"run tests", false},
	}

	for _, tt := range tests {
		result := skill.Matches(tt.input)
		if result != tt.matches {
			t.Errorf("Matches(%q) = %v, want %v", tt.input, result, tt.matches)
		}
	}
}

func TestSkillMatchesDisabled(t *testing.T) {
	skill := &Skill{
		Name:     "disabled-skill",
		Triggers: []string{"hello"},
		Enabled:  false,
	}

	if skill.Matches("hello world") {
		t.Error("Disabled skill should not match")
	}
}

func TestSkillApplyToPrompt(t *testing.T) {
	skill := &Skill{
		Name:        "test-skill",
		Description: "Test",
		Template:    "When reviewing code, look for bugs.\n\n## Example\n\nUser: Review this\nAssistant: I'll check for issues.",
		Enabled:     true,
	}

	result := skill.ApplyToPrompt("Base prompt")

	if result == "Base prompt" {
		t.Error("ApplyToPrompt should modify the prompt")
	}

	if len(result) <= len("Base prompt") {
		t.Error("ApplyToPrompt should add content to prompt")
	}
}

func TestSkillValidate(t *testing.T) {
	tests := []struct {
		skill   Skill
		wantErr bool
	}{
		{Skill{Name: "test", Description: "Test"}, false},
		{Skill{Name: "", Description: "Test"}, true},  // Missing name
		{Skill{Name: "test", Description: ""}, true},  // Missing description
		{Skill{}, true},                               // Empty
	}

	for _, tt := range tests {
		err := tt.skill.Validate()
		if (err != nil) != tt.wantErr {
			t.Errorf("Validate() error = %v, wantErr %v", err, tt.wantErr)
		}
	}
}

func TestParseSkillMD(t *testing.T) {
	content := `---
name: test-skill
description: A test skill
version: "1.0.0"
triggers:
  - test
  - testing
tools:
  - bash
---

# Test Skill

This is the template content.

## Usage

Just test it!
`

	skill, err := ParseSkillMD([]byte(content))
	if err != nil {
		t.Fatalf("ParseSkillMD() error = %v", err)
	}

	if skill.Name != "test-skill" {
		t.Errorf("Name = %q, want %q", skill.Name, "test-skill")
	}

	if skill.Description != "A test skill" {
		t.Errorf("Description = %q, want %q", skill.Description, "A test skill")
	}

	if len(skill.Triggers) != 2 {
		t.Errorf("len(Triggers) = %d, want 2", len(skill.Triggers))
	}

	if skill.Template == "" {
		t.Error("Template should not be empty")
	}

	if skill.Template[:12] != "# Test Skill" {
		t.Errorf("Template should start with '# Test Skill', got %q", skill.Template[:20])
	}
}

func TestParseSkillMDNoFrontmatter(t *testing.T) {
	content := `# Just Markdown

No frontmatter here.
`
	_, err := ParseSkillMD([]byte(content))
	if err == nil {
		t.Error("ParseSkillMD() should error without frontmatter")
	}
}

func TestLoaderLoadAll(t *testing.T) {
	// Create temp directory with test skill in subdirectory
	dir := t.TempDir()

	skillDir := filepath.Join(dir, "test-skill")
	if err := os.Mkdir(skillDir, 0755); err != nil {
		t.Fatal(err)
	}

	skillMD := `---
name: test-skill
description: A test skill for testing
version: "1.0.0"
triggers:
  - test
  - testing
---

# Test Skill

This is a test template.
`
	err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(skillMD), 0644)
	if err != nil {
		t.Fatal(err)
	}

	loader := NewLoader(dir)
	if err := loader.LoadAll(); err != nil {
		t.Fatalf("LoadAll() error = %v", err)
	}

	if loader.Count() != 1 {
		t.Errorf("Count() = %d, want 1", loader.Count())
	}

	skill, ok := loader.Get("test-skill")
	if !ok {
		t.Fatal("Get() failed to find skill")
	}

	if skill.Name != "test-skill" {
		t.Errorf("skill.Name = %q, want %q", skill.Name, "test-skill")
	}
}

func TestLoaderFindMatching(t *testing.T) {
	dir := t.TempDir()

	// Create skill directories
	for _, name := range []string{"skill-1", "skill-2", "skill-3"} {
		if err := os.Mkdir(filepath.Join(dir, name), 0755); err != nil {
			t.Fatal(err)
		}
	}

	skill1MD := `---
name: skill-1
description: First skill
triggers:
  - hello
priority: 10
---

First skill content.
`
	skill2MD := `---
name: skill-2
description: Second skill
triggers:
  - world
priority: 5
---

Second skill content.
`
	skill3MD := `---
name: skill-3
description: Third skill
triggers:
  - hello
priority: 20
---

Third skill content.
`

	os.WriteFile(filepath.Join(dir, "skill-1", "SKILL.md"), []byte(skill1MD), 0644)
	os.WriteFile(filepath.Join(dir, "skill-2", "SKILL.md"), []byte(skill2MD), 0644)
	os.WriteFile(filepath.Join(dir, "skill-3", "SKILL.md"), []byte(skill3MD), 0644)

	loader := NewLoader(dir)
	loader.LoadAll()

	// Test matching "hello" - should return 2 skills, highest priority first
	matching := loader.FindMatching("hello there")
	if len(matching) != 2 {
		t.Errorf("FindMatching() returned %d skills, want 2", len(matching))
	}

	if matching[0].Name != "skill-3" {
		t.Errorf("First match should be skill-3 (priority 20), got %s", matching[0].Name)
	}

	// Test matching "world" - should return 1 skill
	matching = loader.FindMatching("hello world")
	if len(matching) != 3 {
		t.Errorf("FindMatching('hello world') returned %d skills, want 3", len(matching))
	}
}

func TestLoaderEmptyDir(t *testing.T) {
	loader := NewLoader("/nonexistent/path")
	err := loader.LoadAll()
	if err != nil {
		t.Errorf("LoadAll() should not error for nonexistent dir, got %v", err)
	}
	if loader.Count() != 0 {
		t.Errorf("Count() = %d, want 0 for empty/nonexistent dir", loader.Count())
	}
}
