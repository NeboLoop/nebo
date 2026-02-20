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

	"github.com/neboloop/nebo/internal/agent/skills"
)

const (
	// DefaultSkillTTL is how many turns of inactivity before auto-matched skills expire.
	DefaultSkillTTL = 4
	// ManualSkillTTL is how many turns of inactivity before manually loaded skills expire.
	ManualSkillTTL = 6
	// MaxActiveSkills is the hard cap on concurrent active skills per session.
	MaxActiveSkills = 4
	// MaxSkillTokenBudget is the character budget for combined active skill content.
	MaxSkillTokenBudget = 16000
)

// invokedSkillState tracks a skill that was actually invoked (called by the model) in a session.
// Only invoked skills get their templates re-injected into subsequent system prompts.
type invokedSkillState struct {
	lastInvokedTurn int      // Turn when skill was last invoked/re-invoked
	content         string   // SKILL.md template (snapshot at invocation time)
	name            string   // Display name
	tools           []string // Tool restrictions from skill (empty = all tools)
	maxTurns        int      // Per-skill TTL override (0 = use default)
	manual          bool     // Loaded via explicit skill(action: "load")
}

// ttl returns the effective TTL for this invoked skill.
func (s *invokedSkillState) ttl() int {
	if s.maxTurns > 0 {
		return s.maxTurns
	}
	if s.manual {
		return ManualSkillTTL
	}
	return DefaultSkillTTL
}

// SkillDomainTool is the unified skill domain tool.
// All installed apps and standalone skills are exposed through this single tool.
// The LLM sees one catalog of capabilities and interacts through one interface.
type SkillDomainTool struct {
	mu            sync.RWMutex
	entries       map[string]*skillEntry
	invokedSkills map[string]map[string]*invokedSkillState // sessionKey -> slug -> invocation state
	sessionTurns  map[string]int                           // sessionKey -> current turn number
	dirty         bool                                     // schema/description cache invalidation
	skillsDir     string                                   // user skills directory for create/update/delete
	cachedSchema  json.RawMessage
	cachedDesc    string
}

// skillEntry represents a registered skill — either app-backed or standalone.
type skillEntry struct {
	slug        string   // URL-safe identifier: "calendar", "meeting-prep"
	name        string   // Display name: "Calendar", "Meeting Prep"
	description string   // One-liner for catalog
	skillMD     string   // Full SKILL.md content
	adapter     Tool     // gRPC adapter for app-backed skills; nil for standalone
	triggers    []string // Phrases that auto-activate this skill
	tools       []string // Tool restrictions (empty = all tools allowed)
	priority    int      // Higher = matched first when multiple skills trigger
	maxTurns    int      // Per-skill TTL override (0 = use default)
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
		entries:       make(map[string]*skillEntry),
		invokedSkills: make(map[string]map[string]*invokedSkillState),
		sessionTurns:  make(map[string]int),
		skillsDir:     skillsDir,
		dirty:         true,
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

	// Record invocation — the model called this skill, so track it for re-injection
	sessionKey := GetSessionKey(ctx)
	if sessionKey != "" {
		t.recordInvocation(sessionKey, in.Name, false)
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
// triggers are phrases that auto-activate this skill when matched in user messages.
// tools restricts which tools the model can use when this skill is active (empty = all).
// maxTurns overrides the default TTL for auto-expiry (0 = use default).
func (t *SkillDomainTool) Register(slug, name, description, skillMD string, adapter Tool, triggers, tools []string, priority, maxTurns int) {
	t.mu.Lock()
	defer t.mu.Unlock()

	t.entries[slug] = &skillEntry{
		slug:        slug,
		name:        name,
		description: description,
		skillMD:     skillMD,
		adapter:     adapter,
		triggers:    triggers,
		tools:       tools,
		priority:    priority,
		maxTurns:    maxTurns,
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
// Records the skill as invoked (manual=true) so its template is re-injected on subsequent turns.
func (t *SkillDomainTool) loadSkill(ctx context.Context, in SkillDomainInput) (*ToolResult, error) {
	if in.Name == "" {
		return &ToolResult{Content: "Name is required for load.", IsError: true}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		return &ToolResult{Content: "No session context available.", IsError: true}, nil
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

	// Record as manual invocation (stickier TTL)
	t.recordInvocation(sessionKey, in.Name, true)

	content := entry.skillMD
	if content == "" {
		content = fmt.Sprintf("# %s\n\n%s", entry.name, entry.description)
	}

	return &ToolResult{
		Content: fmt.Sprintf("Skill %q loaded for this conversation. Follow the instructions below:\n\n%s", entry.name, content),
	}, nil
}

// unloadSkill removes a skill from the invoked set for the current session.
func (t *SkillDomainTool) unloadSkill(ctx context.Context, in SkillDomainInput) (*ToolResult, error) {
	if in.Name == "" {
		return &ToolResult{Content: "Name is required for unload.", IsError: true}, nil
	}

	sessionKey := GetSessionKey(ctx)
	if sessionKey == "" {
		return &ToolResult{Content: "No session context available.", IsError: true}, nil
	}

	t.mu.Lock()
	if invoked, ok := t.invokedSkills[sessionKey]; ok {
		delete(invoked, in.Name)
		if len(invoked) == 0 {
			delete(t.invokedSkills, sessionKey)
		}
	}
	t.mu.Unlock()

	return &ToolResult{
		Content: fmt.Sprintf("Skill %q unloaded from this conversation.", in.Name),
	}, nil
}

// ActiveSkillContent returns the concatenated templates of invoked skills
// for the given session. Only skills the model actually called get re-injected.
// Sorted by most recently invoked and capped by MaxSkillTokenBudget.
func (t *SkillDomainTool) ActiveSkillContent(sessionKey string) string {
	t.mu.RLock()
	defer t.mu.RUnlock()

	invoked, ok := t.invokedSkills[sessionKey]
	if !ok || len(invoked) == 0 {
		return ""
	}

	// Sort by most recently invoked (higher turn first)
	type ranked struct {
		slug            string
		lastInvokedTurn int
	}
	var items []ranked
	for slug, state := range invoked {
		items = append(items, ranked{slug: slug, lastInvokedTurn: state.lastInvokedTurn})
	}
	sort.Slice(items, func(i, j int) bool {
		return items[i].lastInvokedTurn > items[j].lastInvokedTurn
	})

	var b strings.Builder
	b.WriteString("\n\n## Invoked Skills\n\n")
	b.WriteString("The following skills were invoked in this session. Continue to follow their guidelines:\n\n")

	budget := MaxSkillTokenBudget
	for _, r := range items {
		state := invoked[r.slug]
		content := state.content
		if content == "" {
			content = fmt.Sprintf("# %s\n\n(no template)", state.name)
		}
		if len(content) > budget {
			fmt.Printf("[skills] Skipping invoked skill %q (over budget: %d chars, %d remaining)\n", r.slug, len(content), budget)
			continue
		}
		b.WriteString(fmt.Sprintf("### Skill: %s\n\n", state.name))
		b.WriteString(content)
		b.WriteString("\n\n---\n\n")
		budget -= len(content)
	}

	return b.String()
}

// AutoMatchSkills ticks the session turn counter, expires stale invoked skills,
// and returns brief hints about trigger-matched skills for the system prompt.
// The model must call skill(name: "...") to actually invoke a skill.
func (t *SkillDomainTool) AutoMatchSkills(sessionKey, message string) string {
	if sessionKey == "" {
		return ""
	}

	t.mu.Lock()
	defer t.mu.Unlock()

	// Phase 1: Increment session turn counter
	t.sessionTurns[sessionKey]++
	currentTurn := t.sessionTurns[sessionKey]

	// Phase 2: Expire stale invoked skills
	if invoked := t.invokedSkills[sessionKey]; invoked != nil {
		for slug, state := range invoked {
			if currentTurn-state.lastInvokedTurn > state.ttl() {
				fmt.Printf("[skills] Expired invoked skill %q (inactive for %d turns, ttl=%d) session=%s\n",
					slug, currentTurn-state.lastInvokedTurn, state.ttl(), sessionKey)
				delete(invoked, slug)
			}
		}
		if len(invoked) == 0 {
			delete(t.invokedSkills, sessionKey)
		}
	}

	// Phase 3: Refresh invoked skills whose triggers re-match (reset TTL)
	if message != "" {
		msgLower := strings.ToLower(message)
		if invoked := t.invokedSkills[sessionKey]; invoked != nil {
			for slug, state := range invoked {
				entry, ok := t.entries[slug]
				if !ok {
					continue
				}
				for _, trigger := range entry.triggers {
					if strings.Contains(msgLower, strings.ToLower(trigger)) {
						state.lastInvokedTurn = currentTurn
						break
					}
				}
			}
		}
	}

	if message == "" {
		return ""
	}

	// Phase 4: Match triggers and return brief hints (NOT full templates)
	msgLower := strings.ToLower(message)

	type match struct {
		slug        string
		description string
		priority    int
	}
	var matches []match

	invoked := t.invokedSkills[sessionKey]
	for slug, entry := range t.entries {
		// Skip already-invoked skills (they're already in context via ActiveSkillContent)
		if invoked != nil && invoked[slug] != nil {
			continue
		}
		for _, trigger := range entry.triggers {
			if strings.Contains(msgLower, strings.ToLower(trigger)) {
				matches = append(matches, match{slug: slug, description: entry.description, priority: entry.priority})
				break
			}
		}
	}

	if len(matches) == 0 {
		return ""
	}

	sort.Slice(matches, func(i, j int) bool {
		return matches[i].priority > matches[j].priority
	})

	// Limit hints to top 3
	limit := min(3, len(matches))

	var b strings.Builder
	b.WriteString("\n\n## Skill Matches\n\n")
	b.WriteString("These skills may be relevant to the user's message. Use `skill(name: \"...\")` to activate one:\n")
	for _, m := range matches[:limit] {
		b.WriteString(fmt.Sprintf("- **%s** — %s\n", m.slug, m.description))
		fmt.Printf("[skills] Hint: skill %q matched for session %s (turn %d)\n", m.slug, sessionKey, currentTurn)
	}

	return b.String()
}

// ForceLoadSkill pre-loads a skill into the session without requiring a tool call.
// Used for system-driven skill activation (e.g., onboarding on first run).
// Returns true if the skill was found and loaded.
func (t *SkillDomainTool) ForceLoadSkill(sessionKey, skillName string) bool {
	if sessionKey == "" || skillName == "" {
		return false
	}

	t.mu.RLock()
	_, ok := t.entries[skillName]
	t.mu.RUnlock()

	if !ok {
		return false
	}

	// Record as manual invocation (stickier TTL) — this loads it into ActiveSkillContent
	t.recordInvocation(sessionKey, skillName, true)
	return true
}

// ActiveSkillTools returns the union of tool restrictions from all active skills
// for the given session. Returns nil if no active skills restrict tools (= all tools allowed).
func (t *SkillDomainTool) ActiveSkillTools(sessionKey string) []string {
	t.mu.RLock()
	defer t.mu.RUnlock()

	invoked, ok := t.invokedSkills[sessionKey]
	if !ok || len(invoked) == 0 {
		return nil
	}

	seen := make(map[string]bool)
	anyRestriction := false
	for _, state := range invoked {
		if len(state.tools) > 0 {
			anyRestriction = true
			for _, tool := range state.tools {
				seen[tool] = true
			}
		}
	}
	if !anyRestriction {
		return nil
	}

	result := make([]string, 0, len(seen))
	for tool := range seen {
		result = append(result, tool)
	}
	return result
}

// ClearSession removes all invoked skills and turn counter for a session (called on session clear/end).
func (t *SkillDomainTool) ClearSession(sessionKey string) {
	t.mu.Lock()
	defer t.mu.Unlock()
	delete(t.invokedSkills, sessionKey)
	delete(t.sessionTurns, sessionKey)
}

// recordInvocation tracks that a skill was invoked by the model.
// Must NOT be called with t.mu held — acquires its own lock.
func (t *SkillDomainTool) recordInvocation(sessionKey, slug string, manual bool) {
	t.mu.Lock()
	defer t.mu.Unlock()

	entry, ok := t.entries[slug]
	if !ok {
		return
	}

	if t.invokedSkills[sessionKey] == nil {
		t.invokedSkills[sessionKey] = make(map[string]*invokedSkillState)
	}

	currentTurn := t.sessionTurns[sessionKey]

	// Re-invocation — refresh the turn
	if state := t.invokedSkills[sessionKey][slug]; state != nil {
		state.lastInvokedTurn = currentTurn
		if manual {
			state.manual = true
		}
		fmt.Printf("[skills] Re-invoked skill %q (turn %d) session=%s\n", slug, currentTurn, sessionKey)
		return
	}

	content := entry.skillMD
	if content == "" {
		content = fmt.Sprintf("# %s\n\n%s", entry.name, entry.description)
	}

	t.invokedSkills[sessionKey][slug] = &invokedSkillState{
		lastInvokedTurn: currentTurn,
		content:         content,
		name:            entry.name,
		tools:           entry.tools,
		maxTurns:        entry.maxTurns,
		manual:          manual,
	}
	fmt.Printf("[skills] Recorded invocation: skill %q (turn %d, manual=%v) session=%s\n", slug, currentTurn, manual, sessionKey)
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

	b.WriteString("Skills with triggers auto-activate when the user's message matches. You can also manually load/unload:\n")
	b.WriteString("Use `skill(name: \"<name>\", action: \"load\")` to activate a skill for this conversation.\n")
	b.WriteString("Use `skill(name: \"<name>\", action: \"unload\")` to deactivate.")

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
	desc.WriteString("Unified interface for skills and apps. Call a skill by name to invoke it — invoked skills persist in context.\n")
	desc.WriteString("LIFECYCLE: Available in catalog. Call skill(name: ...) to invoke → content returned + tracked for this session. unload → removed from session.\n")
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
