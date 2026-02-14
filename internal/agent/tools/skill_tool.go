package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"

	"github.com/nebolabs/nebo/internal/agent/skills"
)

// SkillDomainTool is the unified skill domain tool.
// All installed apps and standalone skills are exposed through this single tool.
// The LLM sees one catalog of capabilities and interacts through one interface.
type SkillDomainTool struct {
	mu           sync.RWMutex
	entries      map[string]*skillEntry
	activeSkills map[string]map[string]bool // sessionKey -> set of active skill slugs
	dirty        bool                       // schema/description cache invalidation
	skillsDir    string                     // user skills directory for create/update/delete
	cachedSchema json.RawMessage
	cachedDesc   string
}

// skillEntry represents a registered skill — either app-backed or standalone.
type skillEntry struct {
	slug        string // URL-safe identifier: "calendar", "meeting-prep"
	name        string // Display name: "Calendar", "Meeting Prep"
	description string // One-liner for catalog
	skillMD     string // Full SKILL.md content
	adapter     Tool   // gRPC adapter for app-backed skills; nil for standalone
}

// SkillDomainInput is the input schema for the skill tool.
type SkillDomainInput struct {
	Name     string `json:"name,omitempty"`     // Skill slug
	Resource string `json:"resource,omitempty"` // Passed through to adapter
	Action   string `json:"action,omitempty"`   // "catalog", "help", "create", "update", "delete", or passed through
	Content  string `json:"content,omitempty"`  // SKILL.md content for create/update
}

// NewSkillDomainTool creates a new unified skill domain tool.
// skillsDir is the user skills directory where create/update/delete write files.
// The fsnotify watcher on this directory handles re-registration automatically.
func NewSkillDomainTool(skillsDir string) *SkillDomainTool {
	return &SkillDomainTool{
		entries:      make(map[string]*skillEntry),
		activeSkills: make(map[string]map[string]bool),
		skillsDir:    skillsDir,
		dirty:        true,
	}
}

// --- Tool interface ---

func (t *SkillDomainTool) Name() string { return "skill" }

func (t *SkillDomainTool) Description() string {
	t.mu.RLock()
	if !t.dirty && t.cachedDesc != "" {
		desc := t.cachedDesc
		t.mu.RUnlock()
		return desc
	}
	t.mu.RUnlock()

	t.mu.Lock()
	defer t.mu.Unlock()
	t.rebuildCache()
	return t.cachedDesc
}

func (t *SkillDomainTool) Schema() json.RawMessage {
	t.mu.RLock()
	if !t.dirty && t.cachedSchema != nil {
		schema := t.cachedSchema
		t.mu.RUnlock()
		return schema
	}
	t.mu.RUnlock()

	t.mu.Lock()
	defer t.mu.Unlock()
	t.rebuildCache()
	return t.cachedSchema
}

func (t *SkillDomainTool) RequiresApproval() bool { return false }

func (t *SkillDomainTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var in SkillDomainInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid input: %v", err), IsError: true}, nil
	}

	// Skill lifecycle and session actions
	switch in.Action {
	case "catalog":
		return t.catalog()
	case "create":
		return t.createSkill(in)
	case "update":
		return t.updateSkill(in)
	case "delete":
		return t.deleteSkill(in)
	case "load":
		return t.loadSkill(ctx, in)
	case "unload":
		return t.unloadSkill(ctx, in)
	}

	// No name → return catalog
	if in.Name == "" {
		return t.catalog()
	}

	t.mu.RLock()
	entry, ok := t.entries[in.Name]
	t.mu.RUnlock()

	if !ok {
		return &ToolResult{
			Content: fmt.Sprintf("Skill %q not found. Use skill(action: \"catalog\") to see available skills.", in.Name),
			IsError: true,
		}, nil
	}

	// Empty action or "help" → return SKILL.md (progressive disclosure)
	if in.Action == "" || in.Action == "help" {
		if entry.skillMD != "" {
			return &ToolResult{Content: entry.skillMD}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("# %s\n\n%s\n\nNo detailed documentation available.", entry.name, entry.description)}, nil
	}

	// Standalone skill (no adapter) → return orchestration guidance
	if entry.adapter == nil {
		content := entry.skillMD
		if content == "" {
			content = fmt.Sprintf("# %s\n\n%s", entry.name, entry.description)
		}
		return &ToolResult{
			Content: fmt.Sprintf("This is an orchestration skill. Follow the guidance below, calling other skills as directed.\n\n%s", content),
		}, nil
	}

	// App-backed skill → forward to gRPC adapter
	return entry.adapter.Execute(ctx, input)
}

// --- DomainTool interface ---

func (t *SkillDomainTool) Domain() string { return "skill" }

func (t *SkillDomainTool) Resources() []string {
	t.mu.RLock()
	defer t.mu.RUnlock()
	slugs := make([]string, 0, len(t.entries))
	for slug := range t.entries {
		slugs = append(slugs, slug)
	}
	sort.Strings(slugs)
	return slugs
}

func (t *SkillDomainTool) ActionsFor(resource string) []string {
	return []string{"catalog", "help", "create", "update", "delete", "load", "unload"}
}

// --- Registration ---

// Register adds or updates a skill entry.
// adapter is nil for standalone skills (SKILL.md only).
func (t *SkillDomainTool) Register(slug, name, description, skillMD string, adapter Tool) {
	t.mu.Lock()
	defer t.mu.Unlock()

	t.entries[slug] = &skillEntry{
		slug:        slug,
		name:        name,
		description: description,
		skillMD:     skillMD,
		adapter:     adapter,
	}
	t.dirty = true
}

// Unregister removes a skill entry.
func (t *SkillDomainTool) Unregister(slug string) {
	t.mu.Lock()
	defer t.mu.Unlock()

	delete(t.entries, slug)
	t.dirty = true
}

// UnregisterStandalone removes all standalone skills (no gRPC adapter).
// App-backed skills are preserved.
func (t *SkillDomainTool) UnregisterStandalone() {
	t.mu.Lock()
	defer t.mu.Unlock()

	for slug, entry := range t.entries {
		if entry.adapter == nil {
			delete(t.entries, slug)
		}
	}
	t.dirty = true
}

// Slugs returns all registered skill slugs sorted alphabetically.
func (t *SkillDomainTool) Slugs() []string {
	return t.Resources()
}

// Count returns the number of registered skills.
func (t *SkillDomainTool) Count() int {
	t.mu.RLock()
	defer t.mu.RUnlock()
	return len(t.entries)
}

// --- Session-scoped load/unload ---

// loadSkill activates a skill for the current session.
// The skill's template is injected into the system prompt on subsequent runs.
func (t *SkillDomainTool) loadSkill(ctx context.Context, in SkillDomainInput) (*ToolResult, error) {
	if in.Name == "" {
		return &ToolResult{Content: "Name is required for load.", IsError: true}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		return &ToolResult{Content: "No session context available.", IsError: true}, nil
	}

	t.mu.Lock()
	entry, ok := t.entries[in.Name]
	if !ok {
		t.mu.Unlock()
		return &ToolResult{
			Content: fmt.Sprintf("Skill %q not found. Use skill(action: \"catalog\") to see available skills.", in.Name),
			IsError: true,
		}, nil
	}

	if t.activeSkills[sessionKey] == nil {
		t.activeSkills[sessionKey] = make(map[string]bool)
	}
	t.activeSkills[sessionKey][in.Name] = true
	content := entry.skillMD
	name := entry.name
	t.mu.Unlock()

	if content == "" {
		content = fmt.Sprintf("# %s\n\n%s", name, entry.description)
	}

	return &ToolResult{
		Content: fmt.Sprintf("Skill %q loaded for this conversation. Follow the instructions below:\n\n%s", name, content),
	}, nil
}

// unloadSkill deactivates a skill for the current session.
func (t *SkillDomainTool) unloadSkill(ctx context.Context, in SkillDomainInput) (*ToolResult, error) {
	if in.Name == "" {
		return &ToolResult{Content: "Name is required for unload.", IsError: true}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		return &ToolResult{Content: "No session context available.", IsError: true}, nil
	}

	t.mu.Lock()
	if active, ok := t.activeSkills[sessionKey]; ok {
		delete(active, in.Name)
		if len(active) == 0 {
			delete(t.activeSkills, sessionKey)
		}
	}
	t.mu.Unlock()

	return &ToolResult{
		Content: fmt.Sprintf("Skill %q unloaded from this conversation.", in.Name),
	}, nil
}

// ActiveSkillContent returns the concatenated templates of all skills
// loaded for the given session. Used by the runner to inject into the system prompt.
func (t *SkillDomainTool) ActiveSkillContent(sessionKey string) string {
	t.mu.RLock()
	defer t.mu.RUnlock()

	active, ok := t.activeSkills[sessionKey]
	if !ok || len(active) == 0 {
		return ""
	}

	var b strings.Builder
	b.WriteString("\n\n## Active Skills\n\n")
	b.WriteString("The following skills are loaded for this conversation. Follow their instructions.\n\n")

	for slug := range active {
		if entry, ok := t.entries[slug]; ok {
			content := entry.skillMD
			if content == "" {
				content = fmt.Sprintf("# %s\n\n%s", entry.name, entry.description)
			}
			b.WriteString(content)
			b.WriteString("\n\n---\n\n")
		}
	}

	return b.String()
}

// ClearSession removes all active skills for a session (called on session clear/end).
func (t *SkillDomainTool) ClearSession(sessionKey string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	delete(t.activeSkills, sessionKey)
}

// --- Skill lifecycle ---

// createSkill writes a new SKILL.md to the user skills directory.
func (t *SkillDomainTool) createSkill(in SkillDomainInput) (*ToolResult, error) {
	if t.skillsDir == "" {
		return &ToolResult{Content: "Skill creation not available (no skills directory configured).", IsError: true}, nil
	}
	if in.Content == "" {
		return &ToolResult{Content: "Content is required. Provide valid SKILL.md content with YAML frontmatter.", IsError: true}, nil
	}

	parsed, err := skills.ParseSkillMD([]byte(in.Content))
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid SKILL.md content: %v", err), IsError: true}, nil
	}
	if err := parsed.Validate(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Validation failed: %v", err), IsError: true}, nil
	}

	slug := Slugify(parsed.Name)
	if slug == "" {
		return &ToolResult{Content: "Could not derive a valid slug from the skill name.", IsError: true}, nil
	}

	skillDir := filepath.Join(t.skillsDir, slug)
	if _, err := os.Stat(filepath.Join(skillDir, "SKILL.md")); err == nil {
		return &ToolResult{Content: fmt.Sprintf("Skill %q already exists. Use action: \"update\" to modify it.", slug), IsError: true}, nil
	}

	if err := os.MkdirAll(skillDir, 0755); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to create skill directory: %v", err), IsError: true}, nil
	}
	if err := os.WriteFile(filepath.Join(skillDir, "SKILL.md"), []byte(in.Content), 0644); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to write SKILL.md: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Skill %q created and available in catalog. Use skill(name: %q, action: \"load\") to activate it for this conversation.", parsed.Name, slug)}, nil
}

// updateSkill overwrites an existing user SKILL.md.
func (t *SkillDomainTool) updateSkill(in SkillDomainInput) (*ToolResult, error) {
	if t.skillsDir == "" {
		return &ToolResult{Content: "Skill update not available (no skills directory configured).", IsError: true}, nil
	}
	if in.Name == "" {
		return &ToolResult{Content: "Name is required for update.", IsError: true}, nil
	}
	if in.Content == "" {
		return &ToolResult{Content: "Content is required. Provide the full updated SKILL.md content.", IsError: true}, nil
	}

	parsed, err := skills.ParseSkillMD([]byte(in.Content))
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Invalid SKILL.md content: %v", err), IsError: true}, nil
	}
	if err := parsed.Validate(); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Validation failed: %v", err), IsError: true}, nil
	}

	slug := Slugify(in.Name)
	skillPath := filepath.Join(t.skillsDir, slug, "SKILL.md")
	if _, err := os.Stat(skillPath); os.IsNotExist(err) {
		return &ToolResult{Content: fmt.Sprintf("Skill %q not found in user skills. Only user-created skills can be updated.", slug), IsError: true}, nil
	}

	if err := os.WriteFile(skillPath, []byte(in.Content), 0644); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to write SKILL.md: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Skill %q updated. If it's loaded in this session, unload and reload it to pick up changes.", parsed.Name)}, nil
}

// deleteSkill removes a user skill directory.
func (t *SkillDomainTool) deleteSkill(in SkillDomainInput) (*ToolResult, error) {
	if t.skillsDir == "" {
		return &ToolResult{Content: "Skill deletion not available (no skills directory configured).", IsError: true}, nil
	}
	if in.Name == "" {
		return &ToolResult{Content: "Name is required for delete.", IsError: true}, nil
	}

	slug := Slugify(in.Name)
	skillDir := filepath.Join(t.skillsDir, slug)
	if _, err := os.Stat(filepath.Join(skillDir, "SKILL.md")); os.IsNotExist(err) {
		return &ToolResult{Content: fmt.Sprintf("Skill %q not found in user skills. Only user-created skills can be deleted.", slug), IsError: true}, nil
	}

	if err := os.RemoveAll(skillDir); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to delete skill: %v", err), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Skill %q deleted successfully. It will be unloaded automatically.", in.Name)}, nil
}

// --- Internals ---

// catalog returns a formatted listing of all registered skills.
func (t *SkillDomainTool) catalog() (*ToolResult, error) {
	t.mu.RLock()
	defer t.mu.RUnlock()

	if len(t.entries) == 0 {
		return &ToolResult{Content: "No skills installed. Use skill(action: \"create\", content: \"...\") to create one, or visit the App Store."}, nil
	}

	// Sort entries for stable output
	slugs := make([]string, 0, len(t.entries))
	for slug := range t.entries {
		slugs = append(slugs, slug)
	}
	sort.Strings(slugs)

	var b strings.Builder
	b.WriteString("# Available Skills\n\n")

	var appBacked, standalone []string
	for _, slug := range slugs {
		if t.entries[slug].adapter != nil {
			appBacked = append(appBacked, slug)
		} else {
			standalone = append(standalone, slug)
		}
	}

	if len(appBacked) > 0 {
		b.WriteString("## App Skills\n")
		for _, slug := range appBacked {
			e := t.entries[slug]
			b.WriteString(fmt.Sprintf("- **%s** — %s\n", slug, e.description))
		}
		b.WriteString("\n")
	}

	if len(standalone) > 0 {
		b.WriteString("## Orchestration Skills\n")
		for _, slug := range standalone {
			e := t.entries[slug]
			b.WriteString(fmt.Sprintf("- **%s** — %s\n", slug, e.description))
		}
		b.WriteString("\n")
	}

	b.WriteString("Use `skill(name: \"<name>\", action: \"load\")` to activate a skill for this conversation.\n")
	b.WriteString("Use `skill(name: \"<name>\", action: \"unload\")` when done. Skills are NOT auto-triggered.")

	return &ToolResult{Content: b.String()}, nil
}

// rebuildCache regenerates the cached description and schema.
// Must be called with t.mu held for writing.
func (t *SkillDomainTool) rebuildCache() {
	if !t.dirty {
		return
	}

	// Build description with catalog one-liners
	var desc strings.Builder
	desc.WriteString("Unified interface for skills and apps. Skills are NOT active by default — they must be explicitly loaded per conversation.\n")
	desc.WriteString("LIFECYCLE: create → available in catalog. load → active in THIS session (injected into system prompt). unload → removed from session. Skills do NOT auto-trigger.\n")
	desc.WriteString("Actions: catalog (browse), help (read instructions), load (activate for session), unload (deactivate), create/update/delete (manage on disk).\n\n")
	desc.WriteString("Available skills:\n")

	slugs := make([]string, 0, len(t.entries))
	for slug := range t.entries {
		slugs = append(slugs, slug)
	}
	sort.Strings(slugs)

	for _, slug := range slugs {
		e := t.entries[slug]
		desc.WriteString(fmt.Sprintf("- %s — %s\n", slug, e.description))
	}

	t.cachedDesc = desc.String()

	// Build dynamic schema with name enum
	nameEnum := make([]any, len(slugs))
	for i, s := range slugs {
		nameEnum[i] = s
	}

	properties := map[string]any{
		"name": map[string]any{
			"type":        "string",
			"description": "Skill name/slug (see list above)",
		},
		"action": map[string]any{
			"type":        "string",
			"description": "Action: catalog (list), help (show instructions), load (activate for session), unload (deactivate), create (new skill), update (modify), delete (remove), or skill-specific",
		},
		"resource": map[string]any{
			"type":        "string",
			"description": "Resource type (skill-specific, e.g. events, email, contacts)",
		},
		"content": map[string]any{
			"type":        "string",
			"description": "Full SKILL.md content with YAML frontmatter for create/update actions",
		},
	}

	// Add name enum if there are entries
	if len(nameEnum) > 0 {
		properties["name"].(map[string]any)["enum"] = nameEnum
	}

	schema := map[string]any{
		"type":                 "object",
		"properties":          properties,
		"additionalProperties": true,
	}

	data, _ := json.MarshalIndent(schema, "", "  ")
	t.cachedSchema = json.RawMessage(data)

	t.dirty = false
}

// slugRe matches valid slug characters.
var slugRe = regexp.MustCompile(`[^a-z0-9-]`)

// Slugify converts a name to a URL-safe slug.
func Slugify(name string) string {
	s := strings.ToLower(strings.TrimSpace(name))
	s = strings.ReplaceAll(s, " ", "-")
	s = strings.ReplaceAll(s, "_", "-")
	s = slugRe.ReplaceAllString(s, "")
	// Collapse multiple hyphens
	for strings.Contains(s, "--") {
		s = strings.ReplaceAll(s, "--", "-")
	}
	return strings.Trim(s, "-")
}
