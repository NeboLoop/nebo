// Package neboloop provides HTTP handlers that proxy to NeboLoop API
// for account registration, login, and status checks.
package neboloop

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/google/uuid"
	"github.com/neboloop/nebo/internal/agenthub"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/local"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// NeboLoopRegisterHandler proxies registration to NeboLoop and stores the JWT
func NeboLoopRegisterHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !svcCtx.Config.IsNeboLoopEnabled() {
			httputil.Error(w, fmt.Errorf("NeboLoop integration is disabled"))
			return
		}

		var req types.NeboLoopRegisterRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Forward to NeboLoop
		apiURL := svcCtx.Config.NeboLoop.ApiURL + "/api/v1/owners/register"
		body, _ := json.Marshal(map[string]string{
			"email":        req.Email,
			"display_name": req.DisplayName,
			"password":     req.Password,
		})

		resp, err := http.Post(apiURL, "application/json", bytes.NewReader(body))
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to connect to NeboLoop: %w", err))
			return
		}
		defer resp.Body.Close()

		respBody, _ := io.ReadAll(resp.Body)

		if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(resp.StatusCode)
			w.Write(respBody)
			return
		}

		// Parse upstream response (uses snake_case from NeboLoop API)
		var upstream struct {
			ID          string `json:"id"`
			Email       string `json:"email"`
			DisplayName string `json:"display_name"`
			Token       string `json:"token"`
		}
		if err := json.Unmarshal(respBody, &upstream); err != nil {
			httputil.Error(w, fmt.Errorf("failed to parse NeboLoop response: %w", err))
			return
		}

		// Store the credentials in auth_profiles
		if err := storeNeboLoopProfile(r.Context(), svcCtx.DB, svcCtx.Config.NeboLoop.ApiURL, upstream.ID, upstream.Email, upstream.Token, "", false); err != nil {
			fmt.Printf("[NeboLoop] Warning: failed to store profile: %v\n", err)
		}

		// Activate comm — bot_id is generated locally on first startup,
		// so the agent will auto-connect using the JWT + existing bot_id.
		activateNeboLoopComm(svcCtx)

		httputil.OkJSON(w, types.NeboLoopRegisterResponse{
			ID:          upstream.ID,
			Email:       upstream.Email,
			DisplayName: upstream.DisplayName,
			Token:       upstream.Token,
		})
	}
}

// NeboLoopLoginHandler proxies login to NeboLoop and stores the JWT
func NeboLoopLoginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !svcCtx.Config.IsNeboLoopEnabled() {
			httputil.Error(w, fmt.Errorf("NeboLoop integration is disabled"))
			return
		}

		var req types.NeboLoopLoginRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Forward to NeboLoop
		apiURL := svcCtx.Config.NeboLoop.ApiURL + "/api/v1/owners/login"
		body, _ := json.Marshal(req)

		resp, err := http.Post(apiURL, "application/json", bytes.NewReader(body))
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to connect to NeboLoop: %w", err))
			return
		}
		defer resp.Body.Close()

		respBody, _ := io.ReadAll(resp.Body)

		if resp.StatusCode != http.StatusOK {
			w.Header().Set("Content-Type", "application/json")
			w.WriteHeader(resp.StatusCode)
			w.Write(respBody)
			return
		}

		// Parse upstream response
		var upstream struct {
			ID          string `json:"id"`
			Email       string `json:"email"`
			DisplayName string `json:"display_name"`
			Token       string `json:"token"`
		}
		if err := json.Unmarshal(respBody, &upstream); err != nil {
			httputil.Error(w, fmt.Errorf("failed to parse NeboLoop response: %w", err))
			return
		}

		// Store the credentials in auth_profiles
		if err := storeNeboLoopProfile(r.Context(), svcCtx.DB, svcCtx.Config.NeboLoop.ApiURL, upstream.ID, upstream.Email, upstream.Token, "", false); err != nil {
			fmt.Printf("[NeboLoop] Warning: failed to store profile: %v\n", err)
		}

		// Activate comm — bot_id is generated locally on first startup,
		// so the agent will auto-connect using the JWT + existing bot_id.
		activateNeboLoopComm(svcCtx)

		httputil.OkJSON(w, types.NeboLoopLoginResponse{
			ID:          upstream.ID,
			Email:       upstream.Email,
			DisplayName: upstream.DisplayName,
			Token:       upstream.Token,
		})
	}
}

// NeboLoopAccountStatusHandler returns the current NeboLoop connection status
func NeboLoopAccountStatusHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !svcCtx.Config.IsNeboLoopEnabled() {
			httputil.OkJSON(w, types.NeboLoopAccountStatusResponse{Connected: false})
			return
		}

		ctx := r.Context()
		profiles, err := svcCtx.DB.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
		if err != nil || len(profiles) == 0 {
			httputil.OkJSON(w, types.NeboLoopAccountStatusResponse{Connected: false})
			return
		}

		profile := profiles[0]

		var metadata map[string]string
		if profile.Metadata.Valid {
			json.Unmarshal([]byte(profile.Metadata.String), &metadata)
		}

		httputil.OkJSON(w, types.NeboLoopAccountStatusResponse{
			Connected:     true,
			JanusProvider: metadata["janus_provider"] == "true",
			ProfileID:     profile.ID,
			OwnerID:       metadata["owner_id"],
			Email:         metadata["email"],
			DisplayName:   profile.Name,
		})
	}
}

// NeboLoopDisconnectHandler removes the NeboLoop profile
func NeboLoopDisconnectHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		profiles, err := svcCtx.DB.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to list profiles: %w", err))
			return
		}

		for _, p := range profiles {
			if err := svcCtx.DB.ToggleAuthProfile(ctx, db.ToggleAuthProfileParams{
				ID:       p.ID,
				IsActive: sql.NullInt64{Int64: 0, Valid: true},
			}); err != nil {
				fmt.Printf("[NeboLoop] Warning: failed to deactivate profile %s: %v\n", p.ID, err)
			}
		}

		// Clear token from plugin settings so the comms connection stops.
		// bot_id is preserved — it's immutable and generated locally.
		if svcCtx.PluginStore != nil {
			plugin, err := svcCtx.PluginStore.GetPlugin(ctx, "neboloop")
			if err == nil {
				clearSettings := map[string]string{
					"token": "",
				}
				if err := svcCtx.PluginStore.UpdateSettings(ctx, plugin.ID, clearSettings, nil); err != nil {
					fmt.Printf("[NeboLoop] Warning: failed to clear token: %v\n", err)
				}
			}
		}

		httputil.OkJSON(w, types.NeboLoopDisconnectResponse{Disconnected: true})
	}
}

// NeboLoopOpenHandler opens a NeboLoop page in the user's system browser.
// Accepts an optional ?path= query param (e.g. /app/settings/billing).
// Only allows paths under the NeboLoop frontend domain for safety.
func NeboLoopOpenHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		base := neboLoopFrontendURL(svcCtx.Config.NeboLoop.ApiURL)
		path := r.URL.Query().Get("path")
		targetURL := base + path
		fmt.Printf("[NeboLoop] Opening URL: %s\n", targetURL)
		openBrowser(targetURL)
		httputil.OkJSON(w, map[string]bool{"opened": true})
	}
}

// NeboLoopJanusUsageHandler returns the current Janus token usage from in-memory rate-limit data.
func NeboLoopJanusUsageHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		rl := svcCtx.JanusUsage.Load()
		if rl == nil {
			httputil.OkJSON(w, types.NeboLoopJanusUsageResponse{})
			return
		}
		httputil.OkJSON(w, types.NeboLoopJanusUsageResponse{
			Session: buildWindowUsage(rl.SessionLimitTokens, rl.SessionRemainingTokens, rl.SessionResetAt),
			Weekly:  buildWindowUsage(rl.WeeklyLimitTokens, rl.WeeklyRemainingTokens, rl.WeeklyResetAt),
		})
	}
}

func buildWindowUsage(limit, remaining int64, resetAt time.Time) types.NeboLoopJanusWindowUsage {
	used := limit - remaining
	pct := 0
	if limit > 0 {
		pct = int(float64(used) / float64(limit) * 100)
	}
	resetStr := ""
	if !resetAt.IsZero() {
		resetStr = resetAt.Format(time.RFC3339)
	}
	return types.NeboLoopJanusWindowUsage{
		LimitTokens:     limit,
		RemainingTokens: remaining,
		UsedTokens:      used,
		PercentUsed:     pct,
		ResetAt:         resetStr,
	}
}

// storeNeboLoopProfile saves NeboLoop credentials to auth_profiles.
// refreshToken is stored in metadata for token renewal.
// Carries forward janus_provider from existing profile on re-auth.
func storeNeboLoopProfile(ctx context.Context, store *db.Store, apiURL, ownerID, email, token, refreshToken string, janusProvider bool) error {
	// Carry forward janus_provider from existing profile if not explicitly set.
	// This prevents re-authentication (settings page, token refresh) from
	// losing the Janus provider flag that was set during onboarding.
	if !janusProvider {
		profiles, _ := store.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
		for _, p := range profiles {
			if p.Metadata.Valid {
				var existingMeta map[string]string
				if err := json.Unmarshal([]byte(p.Metadata.String), &existingMeta); err == nil {
					if existingMeta["janus_provider"] == "true" {
						janusProvider = true
						break
					}
				}
			}
		}
	}

	metadata := map[string]string{
		"owner_id":      ownerID,
		"email":         email,
		"refresh_token": refreshToken,
	}
	if janusProvider {
		metadata["janus_provider"] = "true"
	}
	metadataJSON, _ := json.Marshal(metadata)

	// Deactivate existing NeboLoop profiles first
	profiles, _ := store.ListAllActiveAuthProfilesByProvider(ctx, "neboloop")
	for _, p := range profiles {
		store.ToggleAuthProfile(ctx, db.ToggleAuthProfileParams{
			ID:       p.ID,
			IsActive: sql.NullInt64{Int64: 0, Valid: true},
		})
	}

	// Create new profile
	_, err := store.CreateAuthProfile(ctx, db.CreateAuthProfileParams{
		ID:       uuid.New().String(),
		Name:     email,
		Provider: "neboloop",
		ApiKey:   token,
		BaseUrl:  sql.NullString{String: apiURL, Valid: true},
		AuthType: sql.NullString{String: "oauth", Valid: true},
		IsActive: sql.NullInt64{Int64: 1, Valid: true},
		Metadata: sql.NullString{String: string(metadataJSON), Valid: true},
	})
	return err
}

// activateNeboLoopComm persists comm settings and notifies the agent to activate
// the NeboLoop comm plugin. This is called after a successful NeboLoop registration
// or login during onboarding so the agent starts the MQTT install listener
// and receives the Janus gateway app before the user sends their first message.
func activateNeboLoopComm(svcCtx *svc.ServiceContext) {
	// Persist comm settings so the agent re-activates on restart
	if store := local.GetAgentSettings(); store != nil {
		s := store.Get()
		s.CommEnabled = true
		s.CommPlugin = "neboloop"
		if err := store.Update(s); err != nil {
			fmt.Printf("[NeboLoop] Warning: failed to persist comm settings: %v\n", err)
		}
	}

	// Notify the agent to activate the NeboLoop comm plugin now
	if svcCtx.AgentHub != nil {
		svcCtx.AgentHub.Broadcast(&agenthub.Frame{
			Type:   "event",
			Method: "settings_updated",
			Payload: map[string]any{
				"commEnabled": true,
				"commPlugin":  "neboloop",
			},
		})
		fmt.Println("[NeboLoop] Broadcast settings_updated to agent for comm activation")
	}
}
