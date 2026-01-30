package auth

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

func GetAuthConfigHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Return OAuth provider configuration
		// Only return enabled status if OAuth feature is enabled and in local mode
		googleEnabled := false
		githubEnabled := false

		if svcCtx.UseLocal() && svcCtx.Config.IsOAuthEnabled() {
			googleEnabled = svcCtx.Config.IsGoogleOAuthEnabled()
			githubEnabled = svcCtx.Config.IsGitHubOAuthEnabled()
		}

		httputil.OkJSON(w, &types.AuthConfigResponse{
			GoogleEnabled: googleEnabled,
			GitHubEnabled: githubEnabled,
		})
	}
}
