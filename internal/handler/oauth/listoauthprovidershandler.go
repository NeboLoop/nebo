package oauth

import (
	"fmt"
	"net/http"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// List connected OAuth providers
func ListOAuthProvidersHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		if !svcCtx.Config.IsOAuthEnabled() {
			httputil.Error(w, fmt.Errorf("OAuth feature is not enabled"))
			return
		}

		if !svcCtx.UseLocal() {
			httputil.Error(w, fmt.Errorf("OAuth not available in this mode"))
			return
		}

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Get user's OAuth connections
		connections, err := svcCtx.DB.Queries.ListUserOAuthConnections(ctx, userID.String())
		if err != nil {
			logging.Errorf("Failed to list OAuth connections: %v", err)
			httputil.Error(w, err)
			return
		}

		// Build map of connected providers
		connectedProviders := make(map[string]string)
		for _, conn := range connections {
			connectedProviders[conn.Provider] = conn.Email.String
		}

		// Build provider list
		var providers []types.OAuthProvider

		// Google
		if svcCtx.Config.IsGoogleOAuthEnabled() {
			email := connectedProviders["google"]
			providers = append(providers, types.OAuthProvider{
				Name:      "google",
				Connected: email != "",
				Email:     email,
			})
		}

		// GitHub
		if svcCtx.Config.IsGitHubOAuthEnabled() {
			email := connectedProviders["github"]
			providers = append(providers, types.OAuthProvider{
				Name:      "github",
				Connected: email != "",
				Email:     email,
			})
		}

		httputil.OkJSON(w, &types.ListOAuthProvidersResponse{
			Providers: providers,
		})
	}
}
