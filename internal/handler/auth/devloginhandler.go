package auth

import (
	"errors"
	"net/http"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

func DevLoginHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Try to get a user - first try test@example.com, then try admin email pattern
		emails := []string{"test@example.com", "admin@localhost", "alma.tuck@gmail.com"}

		var userID, userEmail string
		for _, email := range emails {
			user, err := svcCtx.DB.GetUserByEmail(ctx, email)
			if err == nil {
				userID = user.ID
				userEmail = user.Email
				break
			}
		}

		if userID == "" {
			httputil.Error(w, errors.New("no users found - run setup first"))
			return
		}

		// Generate tokens for the user
		authResp, err := svcCtx.Auth.GenerateTokensForUser(ctx, userID, userEmail)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		logging.Infof("Dev login: auto-logged in as %s", userEmail)

		httputil.OkJSON(w, &types.LoginResponse{
			Token:        authResp.Token,
			RefreshToken: authResp.RefreshToken,
			ExpiresAt:    authResp.ExpiresAt.Unix() * 1000, // Convert to milliseconds for JS
		})
	}
}
