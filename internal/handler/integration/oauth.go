package integration

import (
	"net/http"

	"github.com/go-chi/chi/v5"

	"github.com/nebolabs/nebo/internal/httputil"
	mcpclient "github.com/nebolabs/nebo/internal/mcp/client"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// GetMCPOAuthURLHandler returns the OAuth authorization URL for an integration
func GetMCPOAuthURLHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		if svcCtx.MCPClient == nil {
			httputil.InternalError(w, "MCP client not initialized")
			return
		}

		authURL, err := svcCtx.MCPClient.StartOAuthFlow(r.Context(), id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, types.GetMCPOAuthURLResponse{AuthURL: authURL})
	}
}

// DisconnectMCPIntegrationHandler revokes tokens and disconnects an OAuth integration
func DisconnectMCPIntegrationHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		id := chi.URLParam(r, "id")
		if id == "" {
			httputil.BadRequest(w, "id is required")
			return
		}

		if svcCtx.MCPClient == nil {
			httputil.InternalError(w, "MCP client not initialized")
			return
		}

		if err := svcCtx.MCPClient.Disconnect(r.Context(), id); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, types.DisconnectMCPIntegrationResponse{
			Success: true,
			Message: "Integration disconnected",
		})
	}
}

// ListMCPToolsHandler lists tools from all connected OAuth integrations
func ListMCPToolsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if svcCtx.MCPClient == nil {
			httputil.OkJSON(w, types.ListMCPToolsResponse{Tools: []types.MCPToolInfo{}})
			return
		}

		// Get all enabled integrations
		integrations, err := svcCtx.DB.ListEnabledMCPIntegrations(r.Context())
		if err != nil {
			httputil.Error(w, err)
			return
		}

		var allTools []types.MCPToolInfo
		for _, integration := range integrations {
			// Only query OAuth integrations that are connected
			if integration.AuthType != "oauth" || !integration.ConnectionStatus.Valid || integration.ConnectionStatus.String != "connected" {
				continue
			}

			tools, err := svcCtx.MCPClient.ListTools(r.Context(), integration.ID)
			if err != nil {
				// Log error but continue with other integrations
				continue
			}

			for _, tool := range tools {
				allTools = append(allTools, types.MCPToolInfo{
					Name:        tool.Name,
					Description: tool.Description,
					ServerType:  integration.ServerType,
				})
			}
		}

		httputil.OkJSON(w, types.ListMCPToolsResponse{Tools: allTools})
	}
}

// OAuthCallbackHandler handles OAuth redirects from external MCP servers
func OAuthCallbackHandler(svcCtx *svc.ServiceContext, frontendURL string) http.HandlerFunc {
	if svcCtx.MCPClient == nil {
		return func(w http.ResponseWriter, r *http.Request) {
			httputil.InternalError(w, "MCP client not initialized")
		}
	}
	return mcpclient.OAuthCallbackHandler(svcCtx.DB, svcCtx.MCPClient, frontendURL, func() {
		notifyIntegrationsChanged(svcCtx)
	})
}
