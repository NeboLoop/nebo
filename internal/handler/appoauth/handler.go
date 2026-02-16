package appoauth

import (
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/oauth/broker"
)

// ConnectHandler starts the OAuth flow for an app+provider.
// GET /apps/{appId}/oauth/{provider}/connect
func ConnectHandler(b *broker.Broker) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := httputil.PathVar(r, "appId")
		providerName := httputil.PathVar(r, "provider")
		scopes := httputil.QueryString(r, "scopes", "")

		authURL, err := b.StartFlow(r.Context(), appID, providerName, scopes)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		http.Redirect(w, r, authURL, http.StatusFound)
	}
}

// CallbackHandler handles the OAuth callback from the provider.
// GET /apps/oauth/callback
func CallbackHandler(b *broker.Broker) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		state := r.URL.Query().Get("state")
		code := r.URL.Query().Get("code")

		if state == "" || code == "" {
			errMsg := r.URL.Query().Get("error")
			if errMsg == "" {
				errMsg = "missing state or code"
			}
			httputil.BadRequest(w, "OAuth error: "+errMsg)
			return
		}

		if err := b.HandleCallback(r.Context(), state, code); err != nil {
			httputil.ErrorWithCode(w, http.StatusBadGateway, "OAuth token exchange failed: "+err.Error())
			return
		}

		// Redirect to the app settings page after successful connection
		w.Header().Set("Content-Type", "text/html")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`<!DOCTYPE html><html><body><script>window.close();</script><p>Connected successfully. You may close this window.</p></body></html>`))
	}
}

// GrantsHandler returns the OAuth grant status for all providers for an app.
// GET /apps/{appId}/oauth/grants
func GrantsHandler(b *broker.Broker) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := httputil.PathVar(r, "appId")

		grants, err := b.GetGrants(r.Context(), appID)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, map[string]any{"grants": grants})
	}
}

// DisconnectHandler removes an OAuth grant for an app+provider.
// DELETE /apps/{appId}/oauth/{provider}
func DisconnectHandler(b *broker.Broker) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		appID := httputil.PathVar(r, "appId")
		providerName := httputil.PathVar(r, "provider")

		if err := b.Disconnect(r.Context(), appID, providerName); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, map[string]any{"ok": true})
	}
}
