package user

import (
	"fmt"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

func UpdateCurrentUserHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.UpdateUserRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.Auth == nil {
			httputil.Error(w, fmt.Errorf("auth service not configured"))
			return
		}

		email, err := auth.GetEmailFromContext(ctx)
		if err != nil {
			logging.Errorf("Failed to get email from context: %v", err)
			httputil.Error(w, err)
			return
		}

		user, err := svcCtx.Auth.GetUserByEmail(ctx, email)
		if err != nil {
			logging.Errorf("Failed to get user %s: %v", email, err)
			httputil.Error(w, err)
			return
		}

		if req.Name != "" {
			user.Name = req.Name
		}

		err = svcCtx.Auth.UpdateUser(ctx, user)
		if err != nil {
			logging.Errorf("Failed to update user %s: %v", email, err)
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.GetUserResponse{
			User: types.User{
				Id:        user.ID,
				Email:     user.Email,
				Name:      user.Name,
				CreatedAt: time.Unix(user.CreatedAt, 0).Format("2006-01-02T15:04:05Z07:00"),
			},
		})
	}
}
