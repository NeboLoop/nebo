package provider

import (
	"database/sql"
	"net/http"
	"time"

	"nebo/internal/db"
	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"

	"github.com/google/uuid"
)

// Create a new auth profile
func CreateAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.CreateAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		id := uuid.New().String()

		priority := int64(req.Priority)
		if priority == 0 {
			priority = 10
		}

		profile, err := svcCtx.DB.CreateAuthProfile(ctx, db.CreateAuthProfileParams{
			ID:       id,
			Name:     req.Name,
			Provider: req.Provider,
			ApiKey:   req.ApiKey,
			Model:    sql.NullString{String: req.Model, Valid: req.Model != ""},
			BaseUrl:  sql.NullString{String: req.BaseUrl, Valid: req.BaseUrl != ""},
			Priority: sql.NullInt64{Int64: priority, Valid: true},
			IsActive: sql.NullInt64{Int64: 1, Valid: true},
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.CreateAuthProfileResponse{
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
