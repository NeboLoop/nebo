package skills

import (
	"os"
	"path/filepath"
	"testing"
)

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
author: nebo-community
dependencies:
  - calendar
  - gmail
tags:
  - productivity
  - meetings
tools:
  - skill
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

	if skill.Author != "nebo-community" {
		t.Errorf("Author = %q, want %q", skill.Author, "nebo-community")
	}

	if len(skill.Dependencies) != 2 {
		t.Errorf("len(Dependencies) = %d, want 2", len(skill.Dependencies))
	}

	if len(skill.Tags) != 2 {
		t.Errorf("len(Tags) = %d, want 2", len(skill.Tags))
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
tags:
  - test
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

func TestLoaderList(t *testing.T) {
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
priority: 10
---

First skill content.
`
	skill2MD := `---
name: skill-2
description: Second skill
priority: 5
---

Second skill content.
`
	skill3MD := `---
name: skill-3
description: Third skill
priority: 20
---

Third skill content.
`

	os.WriteFile(filepath.Join(dir, "skill-1", "SKILL.md"), []byte(skill1MD), 0644)
	os.WriteFile(filepath.Join(dir, "skill-2", "SKILL.md"), []byte(skill2MD), 0644)
	os.WriteFile(filepath.Join(dir, "skill-3", "SKILL.md"), []byte(skill3MD), 0644)

	loader := NewLoader(dir)
	loader.LoadAll()

	list := loader.List()
	if len(list) != 3 {
		t.Errorf("List() returned %d skills, want 3", len(list))
	}

	// Should be sorted by priority (highest first)
	if list[0].Name != "skill-3" {
		t.Errorf("First skill should be skill-3 (priority 20), got %s", list[0].Name)
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
