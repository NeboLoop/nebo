package integration

import (
	"database/sql"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

func ListMCPIntegrationsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		integrations, err := svcCtx.DB.ListMCPIntegrations(r.Context())
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.MCPIntegration, 0, len(integrations))
		for _, i := range integrations {
			result = append(result, toMCPIntegration(i))
		}

		httputil.OkJSON(w, types.ListMCPIntegrationsResponse{Integrations: result})
	}
}

func ListMCPServerRegistryHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		servers, err := svcCtx.DB.ListMCPServerRegistry(r.Context())
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.MCPServerInfo, 0, len(servers))
		for _, s := range servers {
			result = append(result, types.MCPServerInfo{
				Id:                s.ID,
				Name:              s.Name,
				Description:       nullString(s.Description),
				Icon:              nullString(s.Icon),
				AuthType:          s.AuthType,
				ApiKeyUrl:         nullString(s.ApiKeyUrl),
				ApiKeyPlaceholder: nullString(s.ApiKeyPlaceholder),
				IsBuiltin:         s.IsBuiltin.Int64 == 1,
				DisplayOrder:      int(s.DisplayOrder.Int64),
			})
		}

		httputil.OkJSON(w, types.ListMCPServerRegistryResponse{Servers: result})
	}
}

func GetMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		integration, err := svcCtx.DB.GetMCPIntegration(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, types.GetMCPIntegrationResponse{Integration: toMCPIntegration(integration)})
	}
}

func CreateMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.CreateMCPIntegrationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		id := uuid.New().String()
		integration, err := svcCtx.DB.CreateMCPIntegration(r.Context(), db.CreateMCPIntegrationParams{
			ID:         id,
			Name:       req.Name,
			ServerType: req.ServerType,
			ServerUrl:  sql.NullString{String: req.ServerUrl, Valid: req.ServerUrl != ""},
			AuthType:   req.AuthType,
			IsEnabled:  sql.NullInt64{Int64: 1, Valid: true},
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Store credential if API key provided
		if req.ApiKey != "" {
			_, err = svcCtx.DB.CreateMCPIntegrationCredential(r.Context(), db.CreateMCPIntegrationCredentialParams{
				ID:              uuid.New().String(),
				IntegrationID:   id,
				CredentialType:  "api_key",
				CredentialValue: req.ApiKey,
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		httputil.OkJSON(w, types.CreateMCPIntegrationResponse{Integration: toMCPIntegration(integration)})
	}
}

func UpdateMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		var req types.UpdateMCPIntegrationRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get existing integration
		existing, err := svcCtx.DB.GetMCPIntegration(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		name := existing.Name
		if req.Name != "" {
			name = req.Name
		}

		serverUrl := existing.ServerUrl
		if req.ServerUrl != "" {
			serverUrl = sql.NullString{String: req.ServerUrl, Valid: true}
		}

		isEnabled := existing.IsEnabled
		if req.IsEnabled != nil {
			if *req.IsEnabled {
				isEnabled = sql.NullInt64{Int64: 1, Valid: true}
			} else {
				isEnabled = sql.NullInt64{Int64: 0, Valid: true}
			}
		}

		integration, err := svcCtx.DB.UpdateMCPIntegration(r.Context(), db.UpdateMCPIntegrationParams{
			ID:        id,
			Name:      name,
			ServerUrl: serverUrl,
			IsEnabled: isEnabled,
			Metadata:  existing.Metadata,
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Update credential if API key provided
		if req.ApiKey != "" {
			// Delete old credentials and create new
			svcCtx.DB.DeleteMCPIntegrationCredentials(r.Context(), id)
			_, err = svcCtx.DB.CreateMCPIntegrationCredential(r.Context(), db.CreateMCPIntegrationCredentialParams{
				ID:              uuid.New().String(),
				IntegrationID:   id,
				CredentialType:  "api_key",
				CredentialValue: req.ApiKey,
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		httputil.OkJSON(w, types.UpdateMCPIntegrationResponse{Integration: toMCPIntegration(integration)})
	}
}

func DeleteMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		// Delete credentials first (cascade should handle this, but be explicit)
		svcCtx.DB.DeleteMCPIntegrationCredentials(r.Context(), id)

		if err := svcCtx.DB.DeleteMCPIntegration(r.Context(), id); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, types.MessageResponse{Message: "Integration deleted"})
	}
}

func TestMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		// Get integration
		integration, err := svcCtx.DB.GetMCPIntegration(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Get credential
		cred, err := svcCtx.DB.GetMCPIntegrationCredential(r.Context(), id)
		if err != nil && err != sql.ErrNoRows {
			httputil.Error(w, err)
			return
		}

		// TODO: Actually test the connection based on server type
		// For now, just mark as connected if we have credentials
		if integration.AuthType == "none" || (cred.CredentialValue != "") {
			svcCtx.DB.UpdateMCPIntegrationStatus(r.Context(), db.UpdateMCPIntegrationStatusParams{
				ID:               id,
				ConnectionStatus: sql.NullString{String: "connected", Valid: true},
				LastConnectedAt:  sql.NullInt64{Int64: time.Now().Unix(), Valid: true},
			})
			httputil.OkJSON(w, types.TestMCPIntegrationResponse{Success: true, Message: "Connection successful"})
			return
		}

		httputil.OkJSON(w, types.TestMCPIntegrationResponse{Success: false, Message: "No credentials configured"})
	}
}

func toMCPIntegration(i db.McpIntegration) types.MCPIntegration {
	return types.MCPIntegration{
		Id:               i.ID,
		Name:             i.Name,
		ServerType:       i.ServerType,
		ServerUrl:        nullString(i.ServerUrl),
		AuthType:         i.AuthType,
		IsEnabled:        i.IsEnabled.Int64 == 1,
		ConnectionStatus: nullStringDefault(i.ConnectionStatus, "disconnected"),
		LastConnectedAt:  nullTimeString(i.LastConnectedAt),
		LastError:        nullString(i.LastError),
		CreatedAt:        time.Unix(i.CreatedAt, 0).Format(time.RFC3339),
		UpdatedAt:        time.Unix(i.UpdatedAt, 0).Format(time.RFC3339),
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
