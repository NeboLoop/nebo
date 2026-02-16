package user

import (
	"fmt"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/auth"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/logging"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

func GetCurrentUserHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		email, err := auth.GetEmailFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get email from context: %v", err)
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		user, err := svcCtx.Auth.GetUserByEmail(ctx, email)
		if err != nil {
			logging.Errorf("Failed to get user %s: %v", email, err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.GetUserResponse{
			User: types.User{
				Id:            user.ID,
				Email:         user.Email,
				Name:          user.Name,
				EmailVerified: user.EmailVerified == 1,
				CreatedAt:     time.Unix(user.CreatedAt, 0).Format("2006-01-02T15:04:05Z"),
				UpdatedAt:     time.Unix(user.UpdatedAt, 0).Format("2006-01-02T15:04:05Z"),
			},
		})
	}
}
