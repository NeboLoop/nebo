package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
)

// SkillDomainTool is the unified skill domain tool.
// All installed apps and standalone skills are exposed through this single tool.
// The LLM sees one catalog of capabilities and interacts through one interface.
type SkillDomainTool struct {
	mu      sync.RWMutex
	entries map[string]*skillEntry
	dirty   bool   // schema/description cache invalidation
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
	Action   string `json:"action,omitempty"`   // "catalog", "help", or passed through
}

// NewSkillDomainTool creates a new unified skill domain tool.
func NewSkillDomainTool() *SkillDomainTool {
	return &SkillDomainTool{
		entries: make(map[string]*skillEntry),
		dirty:   true,
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

	// No name → return catalog
	if in.Name == "" || in.Action == "catalog" {
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
	return []string{"catalog", "help"}
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

// --- Internals ---

// catalog returns a formatted listing of all registered skills.
func (t *SkillDomainTool) catalog() (*ToolResult, error) {
	t.mu.RLock()
	defer t.mu.RUnlock()

	if len(t.entries) == 0 {
		return &ToolResult{Content: "No skills installed. Visit the App Store to install apps, or create standalone skills in the skills/ directory."}, nil
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

	b.WriteString("Use `skill(name: \"<name>\")` to load detailed instructions for any skill.")

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
	desc.WriteString("Unified interface for all installed skills and apps. ")
	desc.WriteString("Use skill(action: \"catalog\") to browse, skill(name: \"x\") to load instructions.\n\n")
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
			"description": "Skill name (see list above)",
		},
		"action": map[string]any{
			"type":        "string",
			"description": "Action: catalog (list skills), help (load instructions), or skill-specific action",
		},
		"resource": map[string]any{
			"type":        "string",
			"description": "Resource type (skill-specific, e.g. events, email, contacts)",
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
