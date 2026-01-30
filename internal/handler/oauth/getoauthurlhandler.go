package oauth

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net/http"
	"net/url"

	"gobot/internal/httputil"
	"gobot/internal/logging"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// Get OAuth authorization URL
func GetOAuthUrlHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.GetOAuthUrlRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if !svcCtx.Config.IsOAuthEnabled() {
			httputil.Error(w, fmt.Errorf("OAuth feature is not enabled"))
			return
		}

		if !svcCtx.UseLocal() {
			httputil.Error(w, fmt.Errorf("OAuth not available in this mode"))
			return
		}

		// Generate state for CSRF protection
		state, err := generateState()
		if err != nil {
			logging.Errorf("Failed to generate state: %v", err)
			httputil.Error(w, err)
			return
		}

		// Determine callback URL base
		callbackBase := svcCtx.Config.OAuth.CallbackBaseURL
		if callbackBase == "" {
			callbackBase = svcCtx.Config.App.BaseURL
		}

		var authURL string

		switch req.Provider {
		case "google":
			if !svcCtx.Config.IsGoogleOAuthEnabled() {
				httputil.Error(w, fmt.Errorf("Google OAuth is not enabled"))
				return
			}
			authURL = buildGoogleAuthURL(
				svcCtx.Config.OAuth.GoogleClientID,
				callbackBase+"/oauth/google/callback",
				state,
			)
		case "github":
			if !svcCtx.Config.IsGitHubOAuthEnabled() {
				httputil.Error(w, fmt.Errorf("GitHub OAuth is not enabled"))
				return
			}
			authURL = buildGitHubAuthURL(
				svcCtx.Config.OAuth.GitHubClientID,
				callbackBase+"/oauth/github/callback",
				state,
			)
		default:
			httputil.Error(w, fmt.Errorf("unsupported OAuth provider: %s", req.Provider))
			return
		}

		httputil.OkJSON(w, &types.GetOAuthUrlResponse{
			Url:   authURL,
			State: state,
		})
	}
}

func generateState() (string, error) {
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {
		return "", err
	}
	return hex.EncodeToString(b), nil
}

func buildGoogleAuthURL(clientID, redirectURI, state string) string {
	params := url.Values{
		"client_id":     {clientID},
		"redirect_uri":  {redirectURI},
		"response_type": {"code"},
		"scope":         {"openid email profile"},
		"state":         {state},
		"access_type":   {"offline"},
		"prompt":        {"consent"},
	}
	return "https://accounts.google.com/o/oauth2/v2/auth?" + params.Encode()
}

func buildGitHubAuthURL(clientID, redirectURI, state string) string {
	params := url.Values{
		"client_id":    {clientID},
		"redirect_uri": {redirectURI},
		"scope":        {"user:email"},
		"state":        {state},
	}
	return "https://github.com/login/oauth/authorize?" + params.Encode()
}
