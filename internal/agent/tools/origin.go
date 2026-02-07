package tools

import "context"

// Origin identifies the source of a request flowing through the agent.
// Used by Policy to enforce per-origin tool restrictions.
type Origin string

const (
	OriginUser   Origin = "user"   // Direct user interaction (web UI, CLI)
	OriginComm   Origin = "comm"   // Inter-agent communication (NeboLoop, loopback)
	OriginPlugin Origin = "plugin" // External plugin binary
	OriginSkill  Origin = "skill"  // Matched skill template
	OriginSystem Origin = "system" // Internal system tasks (heartbeat, cron, recovery)
)

// contextKey is an unexported type for context keys to avoid collisions.
type contextKey int

const originKey contextKey = iota

// WithOrigin returns a new context carrying the given origin.
func WithOrigin(ctx context.Context, origin Origin) context.Context {
	return context.WithValue(ctx, originKey, origin)
}

// GetOrigin extracts the origin from a context.
// Returns OriginUser if no origin is set (safe default for direct calls).
func GetOrigin(ctx context.Context) Origin {
	if origin, ok := ctx.Value(originKey).(Origin); ok {
		return origin
	}
	return OriginUser
}
