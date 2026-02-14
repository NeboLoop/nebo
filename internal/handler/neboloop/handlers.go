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
	"net/url"

	"github.com/google/uuid"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	neboloopapi "github.com/nebolabs/nebo/internal/neboloop"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
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
			ID              string `json:"id"`
			Email           string `json:"email"`
			DisplayName     string `json:"display_name"`
			Token           string `json:"token"`
			BotID           string `json:"bot_id"`
			ConnectionToken string `json:"connection_token"`
		}
		if err := json.Unmarshal(respBody, &upstream); err != nil {
			httputil.Error(w, fmt.Errorf("failed to parse NeboLoop response: %w", err))
			return
		}

		// Store the credentials in auth_profiles
		if err := storeNeboLoopProfile(r.Context(), svcCtx.DB, svcCtx.Config.NeboLoop.ApiURL, upstream.ID, upstream.Email, upstream.Token); err != nil {
			fmt.Printf("[NeboLoop] Warning: failed to store profile: %v\n", err)
		}

		// Auto-connect the bot (exchange token → MQTT creds → store in plugin settings)
		if upstream.ConnectionToken != "" {
			if err := autoConnectBot(r.Context(), svcCtx, svcCtx.Config.NeboLoop.ApiURL, upstream.BotID, upstream.ConnectionToken); err != nil {
				fmt.Printf("[NeboLoop] Warning: auto-connect failed: %v\n", err)
			}
		}

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
			ID              string `json:"id"`
			Email           string `json:"email"`
			DisplayName     string `json:"display_name"`
			Token           string `json:"token"`
			BotID           string `json:"bot_id"`
			ConnectionToken string `json:"connection_token"`
		}
		if err := json.Unmarshal(respBody, &upstream); err != nil {
			httputil.Error(w, fmt.Errorf("failed to parse NeboLoop response: %w", err))
			return
		}

		// Store the credentials in auth_profiles
		if err := storeNeboLoopProfile(r.Context(), svcCtx.DB, svcCtx.Config.NeboLoop.ApiURL, upstream.ID, upstream.Email, upstream.Token); err != nil {
			fmt.Printf("[NeboLoop] Warning: failed to store profile: %v\n", err)
		}

		// Auto-connect the bot (exchange token → MQTT creds → store in plugin settings)
		if upstream.ConnectionToken != "" {
			if err := autoConnectBot(r.Context(), svcCtx, svcCtx.Config.NeboLoop.ApiURL, upstream.BotID, upstream.ConnectionToken); err != nil {
				fmt.Printf("[NeboLoop] Warning: auto-connect failed: %v\n", err)
			}
		}

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
		profiles, err := svcCtx.DB.ListActiveAuthProfilesByProvider(ctx, "neboloop")
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
			Connected:   true,
			OwnerID:     metadata["owner_id"],
			Email:       metadata["email"],
			DisplayName: profile.Name,
		})
	}
}

// NeboLoopDisconnectHandler removes the NeboLoop profile
func NeboLoopDisconnectHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		profiles, err := svcCtx.DB.ListActiveAuthProfilesByProvider(ctx, "neboloop")
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

		// Also clear bot MQTT credentials so the connection actually stops
		if svcCtx.PluginStore != nil {
			plugin, err := svcCtx.PluginStore.GetPlugin(ctx, "neboloop")
			if err == nil {
				clearSettings := map[string]string{
					"bot_id":        "",
					"mqtt_username": "",
					"mqtt_password": "",
				}
				if err := svcCtx.PluginStore.UpdateSettings(ctx, plugin.ID, clearSettings, nil); err != nil {
					fmt.Printf("[NeboLoop] Warning: failed to clear MQTT settings: %v\n", err)
				}
			}
		}

		httputil.OkJSON(w, types.NeboLoopDisconnectResponse{Disconnected: true})
	}
}

// autoConnectBot exchanges a connection_token for MQTT creds and stores them
// in the neboloop plugin settings (triggers comm plugin auto-reconnect).
func autoConnectBot(ctx context.Context, svcCtx *svc.ServiceContext, apiURL, botID, connectionToken string) error {
	if connectionToken == "" || svcCtx.PluginStore == nil {
		return nil
	}

	// Exchange connection_token → MQTT credentials
	creds, err := neboloopapi.ExchangeToken(ctx, apiURL, connectionToken)
	if err != nil {
		return fmt.Errorf("token exchange: %w", err)
	}

	// Derive MQTT broker from API URL (same host, port 1883)
	parsed, err := url.Parse(apiURL)
	if err != nil {
		return fmt.Errorf("parse api url: %w", err)
	}
	broker := "tcp://" + parsed.Hostname() + ":1883"

	// Store in neboloop plugin settings (triggers OnSettingsChanged → MQTT connect)
	plugin, err := svcCtx.PluginStore.GetPlugin(ctx, "neboloop")
	if err != nil {
		return fmt.Errorf("neboloop plugin not found: %w", err)
	}

	newSettings := map[string]string{
		"api_server":    apiURL,
		"bot_id":        botID,
		"broker":        broker,
		"mqtt_username": creds.MQTTUsername,
		"mqtt_password": creds.MQTTPassword,
	}
	secrets := map[string]bool{
		"mqtt_password": true,
	}

	return svcCtx.PluginStore.UpdateSettings(ctx, plugin.ID, newSettings, secrets)
}

// storeNeboLoopProfile saves NeboLoop credentials to auth_profiles
func storeNeboLoopProfile(ctx context.Context, store *db.Store, apiURL, ownerID, email, token string) error {
	metadata := map[string]string{
		"owner_id": ownerID,
		"email":    email,
	}
	metadataJSON, _ := json.Marshal(metadata)

	// Deactivate existing NeboLoop profiles first
	profiles, _ := store.ListActiveAuthProfilesByProvider(ctx, "neboloop")
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
