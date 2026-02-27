package memory

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"math"
	"sort"
	"strings"
	"time"
)

// DBContext holds context loaded from the database
type DBContext struct {
	AgentName         string
	PersonalityPrompt string
	VoiceStyle        string
	ResponseLength    string
	EmojiUsage        string
	Formality         string
	Proactivity       string
	AgentEmoji        string
	AgentCreature     string
	AgentVibe         string
	AgentRole         string
	AgentRules        string
	ToolNotes         string

	UserDisplayName  string
	UserLocation     string
	UserTimezone     string
	UserOccupation   string
	UserInterests    []string
	UserGoals        string
	UserContext      string
	UserCommStyle    string
	OnboardingNeeded bool

	TacitMemories        []DBMemoryItem
	PersonalityDirective string // Synthesized personality directive from style observations
}

// DBMemoryItem represents a memory item from the database (distinct from MemoryEntry for storage)
type DBMemoryItem struct {
	Namespace string
	Key       string
	Value     string
	Tags      []string

	accessCount int       // for decay scoring
	accessedAt  time.Time // for decay scoring
	confidence  float64   // for quality-weighted ranking (0.0 = no metadata, treated as 1.0)
}

// decayScore calculates a time-decayed relevance score.
// Formula: access_count * 0.7^(days_since_last_access / 30.0)
// NULL accessedAt falls back to raw access_count.
func decayScore(accessCount int, accessedAt *time.Time) float64 {
	if accessedAt == nil || accessedAt.IsZero() {
		return float64(accessCount)
	}
	days := time.Since(*accessedAt).Hours() / 24.0
	return float64(accessCount) * math.Pow(0.7, days/30.0)
}

// LoadContext loads agent and user context from the SQLite database
// Accepts a shared *sql.DB connection - does NOT close it
// If userID is empty, loads the first available user (fallback for CLI mode)
func LoadContext(db *sql.DB, userID string) (*DBContext, error) {
	if db == nil {
		return nil, fmt.Errorf("database connection is nil")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	result := &DBContext{}

	// Load agent profile (shared across all users)
	if err := loadAgentProfile(ctx, db, result); err != nil {
		// Continue even if agent profile fails - use defaults
		fmt.Printf("[memory] Warning: failed to load agent profile: %v\n", err)
	}

	// Load user profile for specific user
	if err := loadUserProfile(ctx, db, result, userID); err != nil {
		// Continue even if user profile fails
		fmt.Printf("[memory] Warning: failed to load user profile: %v\n", err)
	}

	// Load tacit memories so the agent retains learned knowledge across restarts
	if err := loadTacitMemories(ctx, db, result, userID); err != nil {
		fmt.Printf("[memory] Warning: failed to load tacit memories: %v\n", err)
	}

	// Load synthesized personality directive (if any)
	result.PersonalityDirective = GetDirective(ctx, db, userID)

	return result, nil
}

// loadAgentProfile loads the agent's personality settings
func loadAgentProfile(ctx context.Context, db *sql.DB, result *DBContext) error {
	// Get agent profile (created by migrations)
	var name, preset sql.NullString
	var customPersonality, voiceStyle, responseLength, emojiUsage, formality, proactivity sql.NullString
	var emoji, creature, vibe, role, agentRules, toolNotes sql.NullString

	err := db.QueryRowContext(ctx, `
		SELECT name, personality_preset, custom_personality, voice_style,
		       response_length, emoji_usage, formality, proactivity,
		       emoji, creature, vibe, role, agent_rules, tool_notes
		FROM agent_profile WHERE id = 1
	`).Scan(&name, &preset, &customPersonality, &voiceStyle,
		&responseLength, &emojiUsage, &formality, &proactivity,
		&emoji, &creature, &vibe, &role, &agentRules, &toolNotes)

	if err != nil && err != sql.ErrNoRows {
		return err
	}

	result.AgentName = stringOr(name, "Nebo")
	result.VoiceStyle = stringOr(voiceStyle, "neutral")
	result.ResponseLength = stringOr(responseLength, "adaptive")
	result.EmojiUsage = stringOr(emojiUsage, "moderate")
	result.Formality = stringOr(formality, "adaptive")
	result.Proactivity = stringOr(proactivity, "moderate")
	result.AgentEmoji = stringOr(emoji, "")
	result.AgentCreature = stringOr(creature, "")
	result.AgentVibe = stringOr(vibe, "")
	result.AgentRole = stringOr(role, "")
	result.AgentRules = stringOr(agentRules, "")
	result.ToolNotes = stringOr(toolNotes, "")

	// Get personality prompt from preset or custom
	if customPersonality.Valid && customPersonality.String != "" {
		result.PersonalityPrompt = customPersonality.String
	} else {
		presetID := stringOr(preset, "balanced")
		var systemPrompt string
		err = db.QueryRowContext(ctx, `
			SELECT system_prompt FROM personality_presets WHERE id = ?
		`, presetID).Scan(&systemPrompt)
		if err == nil {
			result.PersonalityPrompt = systemPrompt
		} else {
			// Fallback default
			result.PersonalityPrompt = "You are {name}, a helpful and friendly AI assistant."
		}
	}

	return nil
}

// loadUserProfile loads a user's profile by user_id
// If userID is empty, falls back to loading the first user (backwards compatibility)
func loadUserProfile(ctx context.Context, db *sql.DB, result *DBContext, userID string) error {
	var displayName, location, timezone, occupation, interests sql.NullString
	var goals, userContext, commStyle sql.NullString
	var onboardingCompleted sql.NullInt64

	var err error
	if userID != "" {
		// Load specific user's profile
		err = db.QueryRowContext(ctx, `
			SELECT display_name, location, timezone, occupation, interests,
			       goals, context, communication_style, onboarding_completed
			FROM user_profiles
			WHERE user_id = ?
		`, userID).Scan(&displayName, &location, &timezone, &occupation, &interests,
			&goals, &userContext, &commStyle, &onboardingCompleted)
	} else {
		// Backwards compatibility: load first user
		err = db.QueryRowContext(ctx, `
			SELECT display_name, location, timezone, occupation, interests,
			       goals, context, communication_style, onboarding_completed
			FROM user_profiles
			LIMIT 1
		`).Scan(&displayName, &location, &timezone, &occupation, &interests,
			&goals, &userContext, &commStyle, &onboardingCompleted)
	}

	if err == sql.ErrNoRows {
		// No user profile exists - needs onboarding
		result.OnboardingNeeded = true
		return nil
	}
	if err != nil {
		// Any other error (table doesn't exist, etc.) - assume fresh install needs onboarding
		result.OnboardingNeeded = true
		return err
	}

	result.UserDisplayName = stringOr(displayName, "")
	result.UserLocation = stringOr(location, "")
	result.UserTimezone = stringOr(timezone, "")
	result.UserOccupation = stringOr(occupation, "")
	result.UserGoals = stringOr(goals, "")
	result.UserContext = stringOr(userContext, "")
	result.UserCommStyle = stringOr(commStyle, "")
	result.OnboardingNeeded = !onboardingCompleted.Valid || onboardingCompleted.Int64 == 0

	// Parse interests JSON array
	if interests.Valid && interests.String != "" {
		json.Unmarshal([]byte(interests.String), &result.UserInterests)
	}

	return nil
}

// Memory budget constants for system prompt injection.
// Style/personality observations are capped to prevent them from crowding out
// actionable memories like preferences, artifacts, and project context.
const (
	maxTacitMemories = 50 // Total memories injected into system prompt
	maxStyleMemories = 10 // Cap for tacit/personality entries
)

// loadTacitMemories loads persistent memories from the tacit layer for a specific user.
// Uses a two-pass strategy to prevent style observations from crowding out useful memories:
//   1. Load up to maxStyleMemories from tacit/personality (capped)
//   2. Fill remaining slots from all other tacit/* namespaces (preferences, artifacts, etc.)
// If userID is empty, loads memories without user filtering (backwards compatibility).
func loadTacitMemories(ctx context.Context, db *sql.DB, result *DBContext, userID string) error {
	remaining := maxTacitMemories

	// Pass 1: Load capped personality/style memories
	styleCount, err := loadTacitSlice(ctx, db, result, userID, "tacit/personality", maxStyleMemories)
	if err != nil {
		return err
	}
	remaining -= styleCount

	// Pass 2: Fill the rest from non-personality tacit memories
	_, err = loadTacitNonPersonality(ctx, db, result, userID, remaining)
	return err
}

// loadTacitSlice loads memories from a specific namespace with a limit.
// Overfetches by 3x (min 30 rows) and re-ranks by time-decayed score so that
// recently-relevant memories surface above stale high-count entries.
func loadTacitSlice(ctx context.Context, db *sql.DB, result *DBContext, userID, namespace string, limit int) (int, error) {
	overfetch := limit * 3
	if overfetch < 30 {
		overfetch = 30
	}

	var rows *sql.Rows
	var err error

	// Filter out low-confidence facts from system prompt injection.
	// Threshold 0.80: inferred facts (0.6) need 2 reinforcements to cross.
	// Explicit facts (0.9) get in immediately. They can still be found via hybrid search.
	confidenceFilter := `AND (metadata IS NULL
		OR json_extract(metadata, '$.confidence') IS NULL
		OR json_extract(metadata, '$.confidence') >= 0.80)`

	if userID != "" {
		rows, err = db.QueryContext(ctx, `
			SELECT namespace, key, value, tags, access_count, accessed_at,
			       json_extract(metadata, '$.confidence') as confidence
			FROM memories
			WHERE namespace = ? AND user_id = ?
			`+confidenceFilter+`
			ORDER BY access_count DESC
			LIMIT ?
		`, namespace, userID, overfetch)
	} else {
		rows, err = db.QueryContext(ctx, `
			SELECT namespace, key, value, tags, access_count, accessed_at,
			       json_extract(metadata, '$.confidence') as confidence
			FROM memories
			WHERE namespace = ?
			`+confidenceFilter+`
			ORDER BY access_count DESC
			LIMIT ?
		`, namespace, overfetch)
	}
	if err != nil {
		return 0, err
	}
	defer rows.Close()

	var candidates []DBMemoryItem
	for rows.Next() {
		entry, scanErr := scanMemoryRow(rows)
		if scanErr != nil {
			continue
		}
		candidates = append(candidates, entry)
	}

	// Re-rank by confidence × decay score so high-confidence memories outrank
	// frequently-accessed but low-confidence ones.
	sort.Slice(candidates, func(i, j int) bool {
		si := candidates[i].confidence * decayScore(candidates[i].accessCount, &candidates[i].accessedAt)
		sj := candidates[j].confidence * decayScore(candidates[j].accessCount, &candidates[j].accessedAt)
		return si > sj
	})

	// Take top N
	if len(candidates) > limit {
		candidates = candidates[:limit]
	}

	result.TacitMemories = append(result.TacitMemories, candidates...)
	return len(candidates), nil
}

// loadTacitNonPersonality loads memories from all tacit/* namespaces EXCEPT tacit/personality.
// Overfetches by 3x (min 30 rows) and re-ranks by time-decayed score so that
// recently-relevant memories surface above stale high-count entries.
func loadTacitNonPersonality(ctx context.Context, db *sql.DB, result *DBContext, userID string, limit int) (int, error) {
	overfetch := limit * 3
	if overfetch < 30 {
		overfetch = 30
	}

	var rows *sql.Rows
	var err error

	// Filter out low-confidence facts from system prompt injection.
	// Threshold 0.80: inferred facts (0.6) need 2 reinforcements to cross.
	// Explicit facts (0.9) get in immediately. They can still be found via hybrid search.
	confidenceFilter := `AND (metadata IS NULL
		OR json_extract(metadata, '$.confidence') IS NULL
		OR json_extract(metadata, '$.confidence') >= 0.80)`

	if userID != "" {
		rows, err = db.QueryContext(ctx, `
			SELECT namespace, key, value, tags, access_count, accessed_at,
			       json_extract(metadata, '$.confidence') as confidence
			FROM memories
			WHERE (namespace = 'tacit' OR namespace LIKE 'tacit/%') AND namespace != 'tacit/personality' AND user_id = ?
			`+confidenceFilter+`
			ORDER BY access_count DESC
			LIMIT ?
		`, userID, overfetch)
	} else {
		rows, err = db.QueryContext(ctx, `
			SELECT namespace, key, value, tags, access_count, accessed_at,
			       json_extract(metadata, '$.confidence') as confidence
			FROM memories
			WHERE (namespace = 'tacit' OR namespace LIKE 'tacit/%') AND namespace != 'tacit/personality'
			`+confidenceFilter+`
			ORDER BY access_count DESC
			LIMIT ?
		`, overfetch)
	}
	if err != nil {
		return 0, err
	}
	defer rows.Close()

	var candidates []DBMemoryItem
	for rows.Next() {
		entry, scanErr := scanMemoryRow(rows)
		if scanErr != nil {
			continue
		}
		candidates = append(candidates, entry)
	}

	// Re-rank by confidence × decay score so high-confidence memories outrank
	// frequently-accessed but low-confidence ones.
	sort.Slice(candidates, func(i, j int) bool {
		si := candidates[i].confidence * decayScore(candidates[i].accessCount, &candidates[i].accessedAt)
		sj := candidates[j].confidence * decayScore(candidates[j].accessCount, &candidates[j].accessedAt)
		return si > sj
	})

	// Take top N
	if len(candidates) > limit {
		candidates = candidates[:limit]
	}

	result.TacitMemories = append(result.TacitMemories, candidates...)
	return len(candidates), nil
}

// scanMemoryRow scans a single memory row into a DBMemoryItem.
// Expects columns: namespace, key, value, tags, access_count, accessed_at, confidence.
func scanMemoryRow(rows *sql.Rows) (DBMemoryItem, error) {
	var namespace, key, value string
	var tagsJSON sql.NullString
	var accessCount sql.NullInt64
	var accessedAt sql.NullTime
	var confidence sql.NullFloat64

	if err := rows.Scan(&namespace, &key, &value, &tagsJSON, &accessCount, &accessedAt, &confidence); err != nil {
		return DBMemoryItem{}, err
	}

	entry := DBMemoryItem{
		Namespace:  namespace,
		Key:        key,
		Value:      value,
		confidence: 1.0, // Default: no metadata = full trust (legacy memories)
	}

	if accessCount.Valid {
		entry.accessCount = int(accessCount.Int64)
	}
	if accessedAt.Valid {
		entry.accessedAt = accessedAt.Time
	}
	if confidence.Valid {
		entry.confidence = confidence.Float64
	}

	if tagsJSON.Valid && tagsJSON.String != "" {
		json.Unmarshal([]byte(tagsJSON.String), &entry.Tags)
	}

	return entry, nil
}

// FormatForSystemPrompt formats the database context for injection into the system prompt
func (c *DBContext) FormatForSystemPrompt() string {
	var parts []string

	// Agent identity (this goes FIRST - most important)
	agentName := c.AgentName
	if agentName == "" {
		agentName = "Nebo"
	}

	if c.PersonalityPrompt != "" {
		// Replace {name} placeholder in soul documents with the actual agent name
		prompt := strings.ReplaceAll(c.PersonalityPrompt, "{name}", agentName)
		parts = append(parts, prompt)
	} else {
		identity := fmt.Sprintf("# Identity\n\nYou are %s, a personal AI assistant. You are NOT Claude, ChatGPT, or any other AI brand — always introduce yourself as %s.", agentName, agentName)
		parts = append(parts, identity)
	}

	// Agent character (creature, role, vibe, emoji — the "business card")
	if c.AgentCreature != "" || c.AgentRole != "" || c.AgentVibe != "" || c.AgentEmoji != "" {
		var charParts []string
		if c.AgentCreature != "" {
			charParts = append(charParts, "You are a "+c.AgentCreature+".")
		}
		if c.AgentRole != "" {
			charParts = append(charParts, "Your relationship to the user: "+c.AgentRole+".")
		}
		if c.AgentVibe != "" {
			charParts = append(charParts, "Your vibe: "+c.AgentVibe)
		}
		if c.AgentEmoji != "" {
			charParts = append(charParts, "Your signature emoji: "+c.AgentEmoji)
		}
		parts = append(parts, "## Character\n\n"+strings.Join(charParts, "\n"))
	}

	// Emergent personality directive (learned from interaction patterns)
	if c.PersonalityDirective != "" {
		parts = append(parts, "## Personality (Learned)\n\n"+c.PersonalityDirective)
	}

	// Agent style preferences
	if c.VoiceStyle != "" || c.Formality != "" || c.EmojiUsage != "" {
		style := fmt.Sprintf(`Communication style: %s voice, %s formality, %s emoji usage, %s response length`,
			c.VoiceStyle, c.Formality, c.EmojiUsage, c.ResponseLength)
		parts = append(parts, style)
	}

	// User context
	var userParts []string
	if c.UserDisplayName != "" {
		userParts = append(userParts, "Name: "+c.UserDisplayName)
	}
	if c.UserLocation != "" {
		userParts = append(userParts, "Location: "+c.UserLocation)
	}
	if c.UserTimezone != "" {
		userParts = append(userParts, "Timezone: "+c.UserTimezone)
	}
	if c.UserOccupation != "" {
		userParts = append(userParts, "Occupation: "+c.UserOccupation)
	}
	if len(c.UserInterests) > 0 {
		userParts = append(userParts, "Interests: "+strings.Join(c.UserInterests, ", "))
	}
	if c.UserGoals != "" {
		userParts = append(userParts, "Goals: "+c.UserGoals)
	}
	if c.UserContext != "" {
		userParts = append(userParts, "Context: "+c.UserContext)
	}
	if c.UserCommStyle != "" {
		userParts = append(userParts, "Communication preference: "+c.UserCommStyle)
	}

	if len(userParts) > 0 {
		parts = append(parts, "# User Information\n\n"+strings.Join(userParts, "\n"))
	}

	// Agent rules (user-defined behavioral guidelines — AGENTS.md equivalent)
	if c.AgentRules != "" {
		parts = append(parts, formatStructuredContent(c.AgentRules, "Rules"))
	}

	// Tool notes (environment-specific instructions — TOOLS.md equivalent)
	if c.ToolNotes != "" {
		parts = append(parts, formatStructuredContent(c.ToolNotes, "Tool Notes"))
	}

	// Inject tacit memories so the agent retains learned knowledge across restarts
	if len(c.TacitMemories) > 0 {
		var memLines []string
		for _, m := range c.TacitMemories {
			prefix := strings.TrimPrefix(m.Namespace, "tacit/")
			memLines = append(memLines, fmt.Sprintf("- %s/%s: %s", prefix, m.Key, m.Value))
		}
		parts = append(parts, "## What You Know\n\nThese are facts you've learned and stored. Reference them naturally — don't announce that you're \"recalling\" them:\n"+strings.Join(memLines, "\n"))
	}

	// Memory tool instructions
	parts = append(parts, "# Memory\n\nYou have a persistent memory system. Use it actively:\n- **Recall**: `agent(resource: memory, action: recall, key: \"...\")` — retrieve a specific memory\n- **Search**: `agent(resource: memory, action: search, query: \"...\")` — find relevant memories\n- **Store**: `agent(resource: memory, action: store, key: \"...\", value: \"...\", layer: \"tacit\")` — save facts\n\nWhen a user mentions preferences, personal details, or asks you to remember something, store it immediately. When context seems relevant to past conversations, search your memory proactively.")

	if len(parts) == 0 {
		return ""
	}

	return strings.Join(parts, "\n\n---\n\n")
}

// IsEmpty returns true if no meaningful context was loaded
func (c *DBContext) IsEmpty() bool {
	return c.PersonalityPrompt == "" && c.UserDisplayName == ""
}

// NeedsOnboarding returns true if the user hasn't completed onboarding
func (c *DBContext) NeedsOnboarding() bool {
	return c.OnboardingNeeded
}

// formatStructuredContent parses JSON structured rules/notes and renders as markdown.
// Falls back to raw text if the content is not valid structured JSON (backwards compat).
func formatStructuredContent(content string, heading string) string {
	var data struct {
		Version  int `json:"version"`
		Sections []struct {
			Name  string `json:"name"`
			Items []struct {
				Text    string `json:"text"`
				Enabled bool   `json:"enabled"`
			} `json:"items"`
		} `json:"sections"`
	}
	if err := json.Unmarshal([]byte(content), &data); err == nil && data.Version > 0 {
		var sb strings.Builder
		sb.WriteString("# " + heading + "\n\n")
		for _, s := range data.Sections {
			hasEnabled := false
			for _, item := range s.Items {
				if item.Enabled {
					hasEnabled = true
					break
				}
			}
			if !hasEnabled {
				continue
			}
			sb.WriteString("## " + s.Name + "\n")
			for _, item := range s.Items {
				if item.Enabled {
					sb.WriteString("- " + item.Text + "\n")
				}
			}
			sb.WriteString("\n")
		}
		return strings.TrimRight(sb.String(), "\n")
	}
	// Fallback: raw text (backwards compat with plain markdown)
	return "# " + heading + "\n\n" + content
}

// Helper function
func stringOr(ns sql.NullString, def string) string {
	if ns.Valid {
		return ns.String
	}
	return def
}
