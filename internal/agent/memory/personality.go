package memory

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/session"
)

// PersonalityDirectiveKey is the memory key where the synthesized directive is stored
const PersonalityDirectiveKey = "directive"

// PersonalityDirectiveNamespace is the full namespace for personality directives
const PersonalityDirectiveNamespace = "tacit/personality"

// MinStyleObservations is the minimum number of style observations needed before synthesizing.
// Set to 5 to prevent premature personality directives from weak/noisy signals.
const MinStyleObservations = 5

// DecayThresholdDays is how many days a style with reinforced_count==1 survives before decay
const DecayThresholdDays = 14

// styleObservation is a style memory with its reinforcement weight
type styleObservation struct {
	Key             string
	Value           string
	ReinforcedCount float64
	FirstObserved   time.Time
	LastReinforced  time.Time
}

// SynthesizeDirective reads all style/* memories, applies decay, and generates
// a one-paragraph personality directive using the given provider.
// The directive is stored as tacit/personality/directive.
func SynthesizeDirective(ctx context.Context, db *sql.DB, provider ai.Provider, userID string) (string, error) {
	if db == nil || provider == nil {
		return "", fmt.Errorf("db and provider are required")
	}

	observations, err := loadStyleObservations(ctx, db, userID)
	if err != nil {
		return "", fmt.Errorf("failed to load style observations: %w", err)
	}

	if len(observations) < MinStyleObservations {
		return "", nil // Not enough data yet — skip silently
	}

	// Apply decay: remove weak observations that haven't been reinforced recently
	observations = applyDecay(observations)
	if len(observations) == 0 {
		return "", nil
	}

	// Sort by reinforcement count (strongest signals first)
	sort.Slice(observations, func(i, j int) bool {
		return observations[i].ReinforcedCount > observations[j].ReinforcedCount
	})

	// Cap at top 15 observations to keep the synthesis prompt compact
	if len(observations) > 15 {
		observations = observations[:15]
	}

	// Build the synthesis prompt
	var obsLines []string
	for _, obs := range observations {
		obsLines = append(obsLines, fmt.Sprintf("- %s: %s (observed %d times)", obs.Key, obs.Value, int(obs.ReinforcedCount)))
	}

	prompt := fmt.Sprintf(`You are synthesizing a personality directive for an AI assistant based on observed user interaction patterns.

Below are style observations extracted from real conversations, each with a reinforcement count showing how often this pattern was observed:

%s

Distill these observations into a single cohesive paragraph (3-5 sentences) that describes how this assistant should communicate and behave. Write in second person ("You tend to...", "Keep responses..."). Focus on the strongest signals. Don't list traits — weave them into natural prose.

Output ONLY the paragraph, no preamble or formatting.`, strings.Join(obsLines, "\n"))

	events, err := provider.Stream(ctx, &ai.ChatRequest{
		Messages: []session.Message{
			{Role: "user", Content: prompt},
		},
	})
	if err != nil {
		return "", fmt.Errorf("failed to stream synthesis: %w", err)
	}

	var result strings.Builder
	for event := range events {
		if event.Type == ai.EventTypeText {
			result.WriteString(event.Text)
		}
		if event.Type == ai.EventTypeError {
			return "", event.Error
		}
	}

	directive := strings.TrimSpace(result.String())
	if directive == "" {
		return "", nil
	}

	// Store the directive as a tacit memory
	_, err = db.ExecContext(ctx, `
		INSERT INTO memories (namespace, key, value, tags, metadata, user_id, created_at, updated_at)
		VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
		ON CONFLICT(namespace, key, user_id) DO UPDATE SET
			value = excluded.value,
			metadata = excluded.metadata,
			updated_at = CURRENT_TIMESTAMP
	`, PersonalityDirectiveNamespace, PersonalityDirectiveKey, directive,
		`["personality","directive"]`,
		fmt.Sprintf(`{"synthesized_at":"%s","observation_count":%d}`, time.Now().Format(time.RFC3339), len(observations)),
		userID,
	)
	if err != nil {
		return "", fmt.Errorf("failed to store directive: %w", err)
	}

	fmt.Printf("[personality] Synthesized directive from %d observations for user %s\n", len(observations), userID)
	return directive, nil
}

// loadStyleObservations loads all style/* memories from tacit/personality namespace
func loadStyleObservations(ctx context.Context, db *sql.DB, userID string) ([]styleObservation, error) {
	rows, err := db.QueryContext(ctx, `
		SELECT key, value, metadata, created_at
		FROM memories
		WHERE namespace = 'tacit/personality' AND key LIKE 'style/%' AND user_id = ?
	`, userID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var observations []styleObservation
	for rows.Next() {
		var key, value string
		var metadataStr sql.NullString
		var createdAt time.Time

		if err := rows.Scan(&key, &value, &metadataStr, &createdAt); err != nil {
			continue
		}

		obs := styleObservation{
			Key:             key,
			Value:           value,
			ReinforcedCount: 1,
			FirstObserved:   createdAt,
			LastReinforced:  createdAt,
		}

		if metadataStr.Valid && metadataStr.String != "" {
			var meta map[string]interface{}
			if json.Unmarshal([]byte(metadataStr.String), &meta) == nil {
				if count, ok := meta["reinforced_count"].(float64); ok {
					obs.ReinforcedCount = count
				}
				if ts, ok := meta["first_observed"].(string); ok {
					if t, err := time.Parse(time.RFC3339, ts); err == nil {
						obs.FirstObserved = t
					}
				}
				if ts, ok := meta["last_reinforced"].(string); ok {
					if t, err := time.Parse(time.RFC3339, ts); err == nil {
						obs.LastReinforced = t
					}
				}
			}
		}

		observations = append(observations, obs)
	}

	return observations, nil
}

// applyDecay removes weak style observations that haven't been reinforced recently.
// A style with reinforced_count==1 is removed after DecayThresholdDays.
// Higher reinforcement counts get proportionally longer lifespans.
func applyDecay(observations []styleObservation) []styleObservation {
	now := time.Now()
	var kept []styleObservation

	for _, obs := range observations {
		// Lifespan scales with reinforcement: count * threshold days
		maxAge := time.Duration(obs.ReinforcedCount) * time.Duration(DecayThresholdDays) * 24 * time.Hour
		if now.Sub(obs.LastReinforced) < maxAge {
			kept = append(kept, obs)
		}
	}

	return kept
}

// GetDirective loads the current personality directive from the database.
// Returns empty string if none exists yet.
func GetDirective(ctx context.Context, db *sql.DB, userID string) string {
	var value string
	err := db.QueryRowContext(ctx, `
		SELECT value FROM memories
		WHERE namespace = ? AND key = ? AND user_id = ?
	`, PersonalityDirectiveNamespace, PersonalityDirectiveKey, userID).Scan(&value)
	if err != nil {
		return ""
	}
	return value
}
