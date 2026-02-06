package oauth

import (
	"fmt"
	"net/http"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Disconnect OAuth provider
func DisconnectOAuthHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.DisconnectOAuthRequest
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

		// Get user ID from context
		userID, err := auth.GetUserIDFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get user ID: %v", err)
			httputil.Error(w, err)
			return
		}

		// Verify user has this OAuth connection
		_, err = svcCtx.DB.Queries.GetOAuthConnectionByUserAndProvider(ctx, db.GetOAuthConnectionByUserAndProviderParams{
			UserID:   userID.String(),
			Provider: req.Provider,
		})
		if err != nil {
			httputil.Error(w, fmt.Errorf("OAuth provider %s is not connected", req.Provider))
			return
		}

		// Get user to check if they have a password set
		user, err := svcCtx.Auth.GetUserByEmail(ctx, "")
		if err == nil && user != nil {
			// Check if user has other login methods
			connections, _ := svcCtx.DB.Queries.ListUserOAuthConnections(ctx, userID.String())
			hasPassword := user.PasswordHash != ""

			if !hasPassword && len(connections) <= 1 {
				httputil.Error(w, fmt.Errorf("cannot disconnect your only login method; please set a password first"))
				return
			}
		}

		// Delete the OAuth connection
		err = svcCtx.DB.Queries.DeleteOAuthConnectionByProvider(ctx, db.DeleteOAuthConnectionByProviderParams{
			UserID:   userID.String(),
			Provider: req.Provider,
		})
		if err != nil {
			logging.Errorf("Failed to disconnect OAuth: %v", err)
			httputil.Error(w, err)
			return
		}

		logging.Infof("User %s disconnected OAuth provider: %s", userID.String(), req.Provider)

		httputil.OkJSON(w, &types.MessageResponse{
			Message: fmt.Sprintf("Successfully disconnected %s", req.Provider),
		})
	}
}
