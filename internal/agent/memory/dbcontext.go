package memory

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
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

	UserDisplayName  string
	UserLocation     string
	UserTimezone     string
	UserOccupation   string
	UserInterests    []string
	UserGoals        string
	UserContext      string
	UserCommStyle    string
	OnboardingNeeded bool

	TacitMemories []DBMemoryItem
}

// DBMemoryItem represents a memory item from the database (distinct from MemoryEntry for storage)
type DBMemoryItem struct {
	Namespace string
	Key       string
	Value     string
	Tags      []string
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

	// Load tacit memories for specific user
	if err := loadTacitMemories(ctx, db, result, userID); err != nil {
		// Continue even if memories fail
		fmt.Printf("[memory] Warning: failed to load memories: %v\n", err)
	}

	return result, nil
}

// loadAgentProfile loads the agent's personality settings
func loadAgentProfile(ctx context.Context, db *sql.DB, result *DBContext) error {
	// Get agent profile (created by migrations)
	var name, preset sql.NullString
	var customPersonality, voiceStyle, responseLength, emojiUsage, formality, proactivity sql.NullString

	err := db.QueryRowContext(ctx, `
		SELECT name, personality_preset, custom_personality, voice_style,
		       response_length, emoji_usage, formality, proactivity
		FROM agent_profile WHERE id = 1
	`).Scan(&name, &preset, &customPersonality, &voiceStyle,
		&responseLength, &emojiUsage, &formality, &proactivity)

	if err != nil && err != sql.ErrNoRows {
		return err
	}

	result.AgentName = stringOr(name, "Nebo")
	result.VoiceStyle = stringOr(voiceStyle, "neutral")
	result.ResponseLength = stringOr(responseLength, "adaptive")
	result.EmojiUsage = stringOr(emojiUsage, "moderate")
	result.Formality = stringOr(formality, "adaptive")
	result.Proactivity = stringOr(proactivity, "moderate")

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
			result.PersonalityPrompt = "You are Nebo, a helpful and friendly AI assistant."
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

// loadTacitMemories loads persistent memories from the tacit layer for a specific user
// If userID is empty, loads memories without user filtering (backwards compatibility)
func loadTacitMemories(ctx context.Context, db *sql.DB, result *DBContext, userID string) error {
	var rows *sql.Rows
	var err error

	if userID != "" {
		// Load user-specific memories
		rows, err = db.QueryContext(ctx, `
			SELECT namespace, key, value, tags
			FROM memories
			WHERE namespace LIKE 'tacit.%' AND user_id = ?
			ORDER BY access_count DESC
			LIMIT 50
		`, userID)
	} else {
		// Backwards compatibility: load all tacit memories (or first user's)
		rows, err = db.QueryContext(ctx, `
			SELECT namespace, key, value, tags
			FROM memories
			WHERE namespace LIKE 'tacit.%'
			ORDER BY access_count DESC
			LIMIT 50
		`)
	}
	if err != nil {
		return err
	}
	defer rows.Close()

	for rows.Next() {
		var namespace, key, value string
		var tagsJSON sql.NullString

		if err := rows.Scan(&namespace, &key, &value, &tagsJSON); err != nil {
			continue
		}

		entry := DBMemoryItem{
			Namespace: namespace,
			Key:       key,
			Value:     value,
		}

		if tagsJSON.Valid && tagsJSON.String != "" {
			json.Unmarshal([]byte(tagsJSON.String), &entry.Tags)
		}

		result.TacitMemories = append(result.TacitMemories, entry)
	}

	return nil
}

// FormatForSystemPrompt formats the database context for injection into the system prompt
func (c *DBContext) FormatForSystemPrompt() string {
	var parts []string

	// Agent identity (this goes FIRST - most important)
	agentName := c.AgentName
	if agentName == "" {
		agentName = "Nebo"
	}

	identity := fmt.Sprintf("# Identity\n\nYou are %s, a personal AI assistant. You are NOT Claude, ChatGPT, or any other AI brand — always introduce yourself as %s.", agentName, agentName)
	if c.PersonalityPrompt != "" {
		// Personality augments identity on new lines, not inline
		identity += "\n\n" + c.PersonalityPrompt
	}
	parts = append(parts, identity)

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

	// Memories
	if len(c.TacitMemories) > 0 {
		var memParts []string
		for _, m := range c.TacitMemories {
			memParts = append(memParts, fmt.Sprintf("- %s: %s", m.Key, m.Value))
		}
		parts = append(parts, "# Remembered Facts\n\nThese facts were loaded from your persistent memory database. Your memory system is ACTIVE and WORKING.\n\n"+strings.Join(memParts, "\n"))
	} else {
		parts = append(parts, "# Remembered Facts\n\nNo facts stored yet. Your memory system is ACTIVE — use agent(resource: memory, action: store, ...) to save important facts about the user and their preferences.")
	}

	if len(parts) == 0 {
		return ""
	}

	return strings.Join(parts, "\n\n---\n\n")
}

// IsEmpty returns true if no meaningful context was loaded
func (c *DBContext) IsEmpty() bool {
	return c.PersonalityPrompt == "" && c.UserDisplayName == "" && len(c.TacitMemories) == 0
}

// NeedsOnboarding returns true if the user hasn't completed onboarding
func (c *DBContext) NeedsOnboarding() bool {
	return c.OnboardingNeeded
}

// Helper function
func stringOr(ns sql.NullString, def string) string {
	if ns.Valid {
		return ns.String
	}
	return def
}
