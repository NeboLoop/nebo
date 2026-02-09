package integration

import (
	"database/sql"
	"fmt"
	"net/http"
	"net/url"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/nebolabs/nebo/internal/agenthub"
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

		if req.ServerUrl == "" {
			httputil.BadRequest(w, "serverUrl is required")
			return
		}

		// Derive serverType and name from the URL hostname if not provided
		if req.ServerType == "" || req.Name == "" {
			if parsed, err := url.Parse(req.ServerUrl); err == nil && parsed.Hostname() != "" {
				if req.ServerType == "" {
					req.ServerType = parsed.Hostname()
				}
				if req.Name == "" {
					req.Name = parsed.Hostname()
				}
			}
		}
		if req.ServerType == "" {
			req.ServerType = "custom"
		}
		if req.Name == "" {
			req.Name = "MCP Server"
		}

		id := uuid.New().String()
		integration, err := svcCtx.DB.CreateMCPIntegration(r.Context(), db.CreateMCPIntegrationParams{
			ID:         id,
			Name:       req.Name,
			ServerType: req.ServerType,
			ServerUrl:  sql.NullString{String: req.ServerUrl, Valid: true},
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

		notifyIntegrationsChanged(svcCtx)
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

		notifyIntegrationsChanged(svcCtx)
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

		notifyIntegrationsChanged(svcCtx)
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
		_, err := svcCtx.DB.GetMCPIntegration(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.MCPClient == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "MCP client not available")
			return
		}

		// Actually test the connection by listing tools from the server
		mcpTools, err := svcCtx.MCPClient.ListTools(r.Context(), id)
		if err != nil {
			svcCtx.DB.UpdateMCPIntegrationConnectionStatus(r.Context(), db.UpdateMCPIntegrationConnectionStatusParams{
				ConnectionStatus: sql.NullString{String: "error", Valid: true},
				Column2:          "error",
				LastError:        sql.NullString{String: err.Error(), Valid: true},
				ID:               id,
			})
			httputil.OkJSON(w, types.TestMCPIntegrationResponse{Success: false, Message: err.Error()})
			return
		}

		// Connection succeeded — update status and tool count
		svcCtx.DB.UpdateMCPIntegrationConnectionStatus(r.Context(), db.UpdateMCPIntegrationConnectionStatusParams{
			ConnectionStatus: sql.NullString{String: "connected", Valid: true},
			Column2:          "connected",
			LastError:        sql.NullString{Valid: false},
			ID:               id,
		})
		svcCtx.DB.UpdateMCPIntegrationToolCount(r.Context(), db.UpdateMCPIntegrationToolCountParams{
			ToolCount: sql.NullInt64{Int64: int64(len(mcpTools)), Valid: true},
			ID:        id,
		})

		// Notify agent to re-sync MCP bridge
		if svcCtx.AgentHub != nil {
			svcCtx.AgentHub.Broadcast(&agenthub.Frame{
				Type:   "event",
				Method: "integrations_changed",
			})
		}

		httputil.OkJSON(w, types.TestMCPIntegrationResponse{
			Success:   true,
			Message:   fmt.Sprintf("Connected — %d tools available", len(mcpTools)),
			ToolCount: len(mcpTools),
		})
	}
}

func notifyIntegrationsChanged(svcCtx *svc.ServiceContext) {
	if svcCtx.AgentHub != nil {
		svcCtx.AgentHub.Broadcast(&agenthub.Frame{
			Type:   "event",
			Method: "integrations_changed",
		})
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
		ToolCount:        int(i.ToolCount.Int64),
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
