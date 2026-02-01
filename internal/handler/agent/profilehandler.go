package agent

import (
	"database/sql"
	"net/http"
	"time"

	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/logging"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// GetAgentProfileHandler returns the agent's profile settings
func GetAgentProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		// Ensure agent profile exists (singleton)
		if err := svcCtx.DB.EnsureAgentProfile(ctx); err != nil {
			logging.Errorf("Failed to ensure agent profile: %v", err)
		}

		profile, err := svcCtx.DB.GetAgentProfile(ctx)
		if err != nil {
			if err == sql.ErrNoRows {
				// Return default profile
				httputil.OkJSON(w, &types.AgentProfileResponse{
					Name:              "Nebo",
					PersonalityPreset: "balanced",
					VoiceStyle:        "neutral",
					ResponseLength:    "adaptive",
					EmojiUsage:        "moderate",
					Formality:         "adaptive",
					Proactivity:       "moderate",
					CreatedAt:         time.Now().Format(time.RFC3339),
					UpdatedAt:         time.Now().Format(time.RFC3339),
				})
				return
			}
			logging.Errorf("Failed to get agent profile: %v", err)
			httputil.InternalError(w, "failed to get agent profile")
			return
		}

		httputil.OkJSON(w, dbAgentProfileToType(profile))
	}
}

// UpdateAgentProfileHandler updates the agent's profile settings
func UpdateAgentProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.UpdateAgentProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		// Ensure agent profile exists before update
		if err := svcCtx.DB.EnsureAgentProfile(ctx); err != nil {
			logging.Errorf("Failed to ensure agent profile: %v", err)
		}

		// Update the profile
		err := svcCtx.DB.UpdateAgentProfile(ctx, db.UpdateAgentProfileParams{
			Name:              toNullString(req.Name),
			PersonalityPreset: toNullString(req.PersonalityPreset),
			CustomPersonality: toNullString(req.CustomPersonality),
			VoiceStyle:        toNullString(req.VoiceStyle),
			ResponseLength:    toNullString(req.ResponseLength),
			EmojiUsage:        toNullString(req.EmojiUsage),
			Formality:         toNullString(req.Formality),
			Proactivity:       toNullString(req.Proactivity),
		})
		if err != nil {
			logging.Errorf("Failed to update agent profile: %v", err)
			httputil.InternalError(w, "failed to update agent profile")
			return
		}

		// Return updated profile
		profile, err := svcCtx.DB.GetAgentProfile(ctx)
		if err != nil {
			logging.Errorf("Failed to get updated agent profile: %v", err)
			httputil.InternalError(w, "failed to get updated profile")
			return
		}

		httputil.OkJSON(w, dbAgentProfileToType(profile))
	}
}

// ListPersonalityPresetsHandler returns all available personality presets
func ListPersonalityPresetsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		presets, err := svcCtx.DB.ListPersonalityPresets(ctx)
		if err != nil {
			logging.Errorf("Failed to list personality presets: %v", err)
			httputil.InternalError(w, "failed to list presets")
			return
		}

		response := types.ListPersonalityPresetsResponse{
			Presets: make([]types.PersonalityPreset, len(presets)),
		}

		for i, p := range presets {
			response.Presets[i] = types.PersonalityPreset{
				Id:           p.ID,
				Name:         p.Name,
				Description:  fromNullString(p.Description),
				SystemPrompt: p.SystemPrompt,
				Icon:         fromNullString(p.Icon),
				DisplayOrder: int(p.DisplayOrder.Int64),
			}
		}

		httputil.OkJSON(w, response)
	}
}

// Helper functions

func toNullString(s string) sql.NullString {
	if s == "" {
		return sql.NullString{}
	}
	return sql.NullString{String: s, Valid: true}
}

func fromNullString(ns sql.NullString) string {
	if ns.Valid {
		return ns.String
	}
	return ""
}

func dbAgentProfileToType(profile db.AgentProfile) *types.AgentProfileResponse {
	return &types.AgentProfileResponse{
		Name:              profile.Name,
		PersonalityPreset: fromNullString(profile.PersonalityPreset),
		CustomPersonality: fromNullString(profile.CustomPersonality),
		VoiceStyle:        fromNullString(profile.VoiceStyle),
		ResponseLength:    fromNullString(profile.ResponseLength),
		EmojiUsage:        fromNullString(profile.EmojiUsage),
		Formality:         fromNullString(profile.Formality),
		Proactivity:       fromNullString(profile.Proactivity),
		CreatedAt:         time.Unix(profile.CreatedAt, 0).Format(time.RFC3339),
		UpdatedAt:         time.Unix(profile.UpdatedAt, 0).Format(time.RFC3339),
	}
}
