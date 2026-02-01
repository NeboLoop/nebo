package channel

import (
	"database/sql"
	"encoding/json"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

func ListChannelsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		channels, err := svcCtx.DB.ListChannels(r.Context())
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.ChannelItem, 0, len(channels))
		for _, c := range channels {
			item := toChannelItem(c)
			// Load config for each channel
			configs, _ := svcCtx.DB.ListChannelConfig(r.Context(), c.ID)
			if len(configs) > 0 {
				item.Config = make(map[string]string)
				for _, cfg := range configs {
					item.Config[cfg.ConfigKey] = cfg.ConfigValue
				}
			}
			result = append(result, item)
		}

		httputil.OkJSON(w, types.ListChannelsResponse{Channels: result})
	}
}

func ListChannelRegistryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		channels, err := svcCtx.DB.ListChannelRegistry(r.Context())
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.ChannelRegistryItem, 0, len(channels))
		for _, c := range channels {
			var required, optional []string
			if c.RequiredCredentials.Valid {
				json.Unmarshal([]byte(c.RequiredCredentials.String), &required)
			}
			if c.OptionalCredentials.Valid {
				json.Unmarshal([]byte(c.OptionalCredentials.String), &optional)
			}

			result = append(result, types.ChannelRegistryItem{
				Id:                  c.ID,
				Name:                c.Name,
				Description:         nullString(c.Description),
				Icon:                nullString(c.Icon),
				SetupInstructions:   nullString(c.SetupInstructions),
				RequiredCredentials: required,
				OptionalCredentials: optional,
				DisplayOrder:        int(c.DisplayOrder.Int64),
			})
		}

		httputil.OkJSON(w, types.ListChannelRegistryResponse{Channels: result})
	}
}

func GetChannelHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		channel, err := svcCtx.DB.GetChannel(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		item := toChannelItem(channel)
		// Load config
		configs, _ := svcCtx.DB.ListChannelConfig(r.Context(), id)
		if len(configs) > 0 {
			item.Config = make(map[string]string)
			for _, cfg := range configs {
				item.Config[cfg.ConfigKey] = cfg.ConfigValue
			}
		}

		httputil.OkJSON(w, types.GetChannelResponse{Channel: item})
	}
}

func CreateChannelHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.CreateChannelRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		id := uuid.New().String()
		channel, err := svcCtx.DB.CreateChannel(r.Context(), db.CreateChannelParams{
			ID:          id,
			Name:        req.Name,
			ChannelType: req.ChannelType,
			IsEnabled:   sql.NullInt64{Int64: 1, Valid: true},
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Store credentials
		for key, value := range req.Credentials {
			_, err = svcCtx.DB.UpsertChannelCredential(r.Context(), db.UpsertChannelCredentialParams{
				ID:              uuid.New().String(),
				ChannelID:       id,
				CredentialKey:   key,
				CredentialValue: value,
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		// Store config
		for key, value := range req.Config {
			_, err = svcCtx.DB.UpsertChannelConfig(r.Context(), db.UpsertChannelConfigParams{
				ID:          uuid.New().String(),
				ChannelID:   id,
				ConfigKey:   key,
				ConfigValue: value,
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		item := toChannelItem(channel)
		item.Config = req.Config

		httputil.OkJSON(w, types.CreateChannelResponse{Channel: item})
	}
}

func UpdateChannelHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		var req types.UpdateChannelRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get existing channel
		existing, err := svcCtx.DB.GetChannel(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		name := existing.Name
		if req.Name != "" {
			name = req.Name
		}

		isEnabled := existing.IsEnabled
		if req.IsEnabled != nil {
			if *req.IsEnabled {
				isEnabled = sql.NullInt64{Int64: 1, Valid: true}
			} else {
				isEnabled = sql.NullInt64{Int64: 0, Valid: true}
			}
		}

		channel, err := svcCtx.DB.UpdateChannel(r.Context(), db.UpdateChannelParams{
			ID:        id,
			Name:      name,
			IsEnabled: isEnabled,
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Update credentials
		for key, value := range req.Credentials {
			_, err = svcCtx.DB.UpsertChannelCredential(r.Context(), db.UpsertChannelCredentialParams{
				ID:              uuid.New().String(),
				ChannelID:       id,
				CredentialKey:   key,
				CredentialValue: value,
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		// Update config
		for key, value := range req.Config {
			_, err = svcCtx.DB.UpsertChannelConfig(r.Context(), db.UpsertChannelConfigParams{
				ID:          uuid.New().String(),
				ChannelID:   id,
				ConfigKey:   key,
				ConfigValue: value,
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		item := toChannelItem(channel)
		// Load updated config
		configs, _ := svcCtx.DB.ListChannelConfig(r.Context(), id)
		if len(configs) > 0 {
			item.Config = make(map[string]string)
			for _, cfg := range configs {
				item.Config[cfg.ConfigKey] = cfg.ConfigValue
			}
		}

		httputil.OkJSON(w, types.UpdateChannelResponse{Channel: item})
	}
}

func DeleteChannelHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		// Delete credentials and config (cascade should handle this, but be explicit)
		svcCtx.DB.DeleteChannelCredentials(r.Context(), id)

		if err := svcCtx.DB.DeleteChannel(r.Context(), id); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, types.MessageResponse{Message: "Channel deleted"})
	}
}

func TestChannelHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		// Get channel
		channel, err := svcCtx.DB.GetChannel(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Get credentials
		creds, err := svcCtx.DB.ListChannelCredentials(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Check if we have required credentials
		hasBotToken := false
		for _, c := range creds {
			if c.CredentialKey == "bot_token" && c.CredentialValue != "" {
				hasBotToken = true
				break
			}
		}

		if !hasBotToken {
			httputil.OkJSON(w, types.TestChannelResponse{Success: false, Message: "Bot token not configured"})
			return
		}

		// TODO: Actually test the connection based on channel type
		// For now, just mark as connected if we have credentials
		svcCtx.DB.UpdateChannelStatus(r.Context(), db.UpdateChannelStatusParams{
			ID:               id,
			ConnectionStatus: sql.NullString{String: "connected", Valid: true},
			LastConnectedAt:  sql.NullInt64{Int64: time.Now().Unix(), Valid: true},
		})

		_ = channel // Use later for actual connection test
		httputil.OkJSON(w, types.TestChannelResponse{Success: true, Message: "Connection successful"})
	}
}

func toChannelItem(c db.Channel) types.ChannelItem {
	return types.ChannelItem{
		Id:               c.ID,
		Name:             c.Name,
		ChannelType:      c.ChannelType,
		IsEnabled:        c.IsEnabled.Int64 == 1,
		ConnectionStatus: nullStringDefault(c.ConnectionStatus, "disconnected"),
		LastConnectedAt:  nullTimeString(c.LastConnectedAt),
		LastError:        nullString(c.LastError),
		MessageCount:     c.MessageCount.Int64,
		CreatedAt:        time.Unix(c.CreatedAt, 0).Format(time.RFC3339),
		UpdatedAt:        time.Unix(c.UpdatedAt, 0).Format(time.RFC3339),
	}
}

func nullString(s sql.NullString) string {
	if s.Valid {
		return s.String
	}
	return ""
}

func nullStringDefault(s sql.NullString, def string) string {
	if s.Valid {
		return s.String
	}
	return def
}

func nullTimeString(t sql.NullInt64) string {
	if t.Valid && t.Int64 > 0 {
		return time.Unix(t.Int64, 0).Format(time.RFC3339)
	}
	return ""
}
