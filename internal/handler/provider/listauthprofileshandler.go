package provider

import (
	"net/http"
	"time"

	"gobot/internal/httputil"
	"gobot/internal/svc"
	"gobot/internal/types"
)

// List all auth profiles (API keys)
func ListAuthProfilesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		profiles, err := svcCtx.DB.ListAuthProfiles(ctx)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		result := make([]types.AuthProfile, len(profiles))
		for i, p := range profiles {
			result[i] = types.AuthProfile{
				Id:        p.ID,
				Name:      p.Name,
				Provider:  p.Provider,
				Model:     p.Model.String,
				BaseUrl:   p.BaseUrl.String,
				Priority:  int(p.Priority.Int64),
				IsActive:  p.IsActive.Int64 == 1,
				CreatedAt: time.Unix(p.CreatedAt, 0).Format(time.RFC3339),
				UpdatedAt: time.Unix(p.UpdatedAt, 0).Format(time.RFC3339),
			}
		}

		httputil.OkJSON(w, &types.ListAuthProfilesResponse{
			Profiles: result,
		})
	}
}
