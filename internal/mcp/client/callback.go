package client

import (
	"database/sql"
	"fmt"
	"net/http"
	"net/url"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
)

// OAuthCallbackHandler handles OAuth redirects from external MCP servers.
// onConnect is called after a successful token exchange so callers can trigger a bridge re-sync.
func OAuthCallbackHandler(database *db.Store, mcpClient *Client, frontendURL string, onConnect func()) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		code := r.URL.Query().Get("code")
		state := r.URL.Query().Get("state")
		errParam := r.URL.Query().Get("error")
		errDesc := r.URL.Query().Get("error_description")

		// Handle OAuth error
		if errParam != "" {
			logging.Warnf("OAuth error: %s - %s", errParam, errDesc)
			redirectWithError(w, r, frontendURL, errParam, errDesc)
			return
		}

		// Validate required parameters
		if code == "" {
			redirectWithError(w, r, frontendURL, "missing_code", "Authorization code not provided")
			return
		}
		if state == "" {
			redirectWithError(w, r, frontendURL, "missing_state", "State parameter not provided")
			return
		}

		// Lookup integration by state
		integration, err := database.GetMCPIntegrationByOAuthState(ctx, sql.NullString{String: state, Valid: true})
		if err != nil {
			logging.Warnf("OAuth callback: invalid state %s: %v", state, err)
			redirectWithError(w, r, frontendURL, "invalid_state", "Invalid or expired state parameter")
			return
		}

		// Exchange code for tokens
		if err := mcpClient.ExchangeCode(ctx, integration.ID, code); err != nil {
			logging.Errorf("OAuth callback: token exchange failed for %s: %v", integration.ID, err)

			// Update integration with error
			database.UpdateMCPIntegrationConnectionStatus(ctx, db.UpdateMCPIntegrationConnectionStatusParams{
				ConnectionStatus: sql.NullString{String: "error", Valid: true},
				Column2:          "error",
				LastError:        sql.NullString{String: err.Error(), Valid: true},
				ID:               integration.ID,
			})

			redirectWithError(w, r, frontendURL, "token_exchange_failed", err.Error())
			return
		}

		// Clear OAuth state
		if err := database.ClearMCPIntegrationOAuthState(ctx, integration.ID); err != nil {
			logging.Warnf("OAuth callback: failed to clear state for %s: %v", integration.ID, err)
		}

		// Update connection status to connected
		if err := database.UpdateMCPIntegrationConnectionStatus(ctx, db.UpdateMCPIntegrationConnectionStatusParams{
			ConnectionStatus: sql.NullString{String: "connected", Valid: true},
			Column2:          "connected",
			LastError:        sql.NullString{},
			ID:               integration.ID,
		}); err != nil {
			logging.Warnf("OAuth callback: failed to update status for %s: %v", integration.ID, err)
		}

		logging.Infof("OAuth callback: successfully connected integration %s (%s)", integration.Name, integration.ID)

		// Notify bridge to re-sync (picks up new credentials)
		if onConnect != nil {
			onConnect()
		}

		// Redirect to frontend with success
		redirectURL := fmt.Sprintf("%s/settings/mcp?connected=%s", frontendURL, url.QueryEscape(integration.ID))
		http.Redirect(w, r, redirectURL, http.StatusFound)
	}
}

// OAuthCallbackJSONHandler handles OAuth callbacks and returns JSON (for API-based flows)
func OAuthCallbackJSONHandler(database *db.Store, mcpClient *Client) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		code := r.URL.Query().Get("code")
		state := r.URL.Query().Get("state")
		errParam := r.URL.Query().Get("error")
		errDesc := r.URL.Query().Get("error_description")

		// Handle OAuth error
		if errParam != "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, fmt.Sprintf("%s: %s", errParam, errDesc))
			return
		}

		// Validate required parameters
		if code == "" {
			httputil.BadRequest(w, "Authorization code not provided")
			return
		}
		if state == "" {
			httputil.BadRequest(w, "State parameter not provided")
			return
		}

		// Lookup integration by state
		integration, err := database.GetMCPIntegrationByOAuthState(ctx, sql.NullString{String: state, Valid: true})
		if err != nil {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "Invalid or expired state parameter")
			return
		}

		// Exchange code for tokens
		if err := mcpClient.ExchangeCode(ctx, integration.ID, code); err != nil {
			// Update integration with error
			database.UpdateMCPIntegrationConnectionStatus(ctx, db.UpdateMCPIntegrationConnectionStatusParams{
				ConnectionStatus: sql.NullString{String: "error", Valid: true},
				Column2:          "error",
				LastError:        sql.NullString{String: err.Error(), Valid: true},
				ID:               integration.ID,
			})

			httputil.ErrorWithCode(w, http.StatusInternalServerError, "Token exchange failed: "+err.Error())
			return
		}

		// Clear OAuth state
		database.ClearMCPIntegrationOAuthState(ctx, integration.ID)

		// Update connection status
		database.UpdateMCPIntegrationConnectionStatus(ctx, db.UpdateMCPIntegrationConnectionStatusParams{
			ConnectionStatus: sql.NullString{String: "connected", Valid: true},
			Column2:          "connected",
			LastError:        sql.NullString{},
			ID:               integration.ID,
		})

		httputil.OkJSON(w, map[string]interface{}{
			"success":       true,
			"integrationId": integration.ID,
			"message":       "Successfully connected",
		})
	}
}

// redirectWithError redirects to the frontend with an error message
func redirectWithError(w http.ResponseWriter, r *http.Request, frontendURL, errCode, errDesc string) {
	redirectURL := fmt.Sprintf("%s/settings/mcp?error=%s&error_description=%s",
		frontendURL,
		url.QueryEscape(errCode),
		url.QueryEscape(errDesc))
	http.Redirect(w, r, redirectURL, http.StatusFound)
}
