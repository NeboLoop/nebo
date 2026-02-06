package provider

import (
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Get auth profile by ID
func GetAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.GetAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		profile, err := svcCtx.DB.GetAuthProfile(ctx, req.Id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.GetAuthProfileResponse{
			Profile: types.AuthProfile{
				Id:        profile.ID,
				Name:      profile.Name,
				Provider:  profile.Provider,
				Model:     profile.Model.String,
				BaseUrl:   profile.BaseUrl.String,
				Priority:  int(profile.Priority.Int64),
				IsActive:  profile.IsActive.Int64 == 1,
				CreatedAt: time.Unix(profile.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt: time.Unix(profile.UpdatedAt, 0).Format(time.RFC3339),
			},
		})
	}
}
