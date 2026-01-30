package memory

import (
	"os"
	"path/filepath"
	"strings"
)

// LoadedFiles contains the contents of personality and memory files
type LoadedFiles struct {
	Agents    string // AGENTS.md - Agent behavior instructions
	Memory    string // MEMORY.md - Long-term facts and preferences
	Soul      string // SOUL.md - Personality and identity
	Heartbeat string // HEARTBEAT.md - Proactive tasks to check
}

// LoadMemoryFiles loads personality and memory files from workspace or home directory
// It tries workspace first, then falls back to ~/.nebo/
func LoadMemoryFiles(workspaceDir string) LoadedFiles {
	result := LoadedFiles{}

	// Files to load with their field setters
	files := []struct {
		name   string
		setter func(string)
	}{
		{"AGENTS.md", func(s string) { result.Agents = s }},
		{"MEMORY.md", func(s string) { result.Memory = s }},
		{"SOUL.md", func(s string) { result.Soul = s }},
		{"HEARTBEAT.md", func(s string) { result.Heartbeat = s }},
	}

	// Build paths to check (workspace first, then home)
	var basePaths []string
	if workspaceDir != "" {
		basePaths = append(basePaths, workspaceDir)
	}
	if homeDir, err := os.UserHomeDir(); err == nil {
		basePaths = append(basePaths, filepath.Join(homeDir, ".nebo"))
	}

	// Load each file
	for _, file := range files {
		for _, base := range basePaths {
			path := filepath.Join(base, file.name)
			if content, err := os.ReadFile(path); err == nil {
				file.setter(strings.TrimSpace(string(content)))
				break
			}
		}
	}

	return result
}

// FormatForSystemPrompt formats the loaded files for injection into the system prompt
func (f LoadedFiles) FormatForSystemPrompt() string {
	var parts []string

	// Soul/personality comes first - defines who the agent is
	if f.Soul != "" {
		parts = append(parts, "# Personality (SOUL.md)\n\n"+f.Soul)
	}

	if f.Agents != "" {
		parts = append(parts, "# Agent Instructions (AGENTS.md)\n\n"+f.Agents)
	}

	if f.Memory != "" {
		parts = append(parts, "# User Memory (MEMORY.md)\n\n"+f.Memory)
	}

	// Note: HEARTBEAT.md is not included here - it's used by the heartbeat daemon

	if len(parts) == 0 {
		return ""
	}

	return strings.Join(parts, "\n\n---\n\n")
}

// IsEmpty returns true if no memory files were loaded
func (f LoadedFiles) IsEmpty() bool {
	return f.Agents == "" && f.Memory == "" && f.Soul == ""
}

// HasHeartbeat returns true if HEARTBEAT.md was loaded
func (f LoadedFiles) HasHeartbeat() bool {
	return f.Heartbeat != ""
}
