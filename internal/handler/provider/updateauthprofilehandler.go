package provider

import (
	"database/sql"
	"encoding/json"
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/credential"
	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// Update auth profile
func UpdateAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.UpdateAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Get existing profile first
		existing, err := svcCtx.DB.GetAuthProfile(ctx, req.Id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Use existing values if not provided
		name := req.Name
		if name == "" {
			name = existing.Name
		}

		apiKey := existing.ApiKey // Already encrypted in DB
		if req.ApiKey != "" {
			// New key provided — encrypt it
			var encErr error
			apiKey, encErr = credential.Encrypt(req.ApiKey)
			if encErr != nil {
				httputil.InternalError(w, "failed to encrypt API key")
				return
			}
		}

		model := req.Model
		if model == "" {
			model = existing.Model.String
		}

		baseUrl := req.BaseUrl
		if baseUrl == "" {
			baseUrl = existing.BaseUrl.String
		}

		priority := int64(req.Priority)
		if priority == 0 {
			priority = existing.Priority.Int64
		}

		// Merge metadata: overlay new keys onto existing
		metadataStr := existing.Metadata
		if req.Metadata != nil {
			var merged map[string]string
			if existing.Metadata.Valid {
				json.Unmarshal([]byte(existing.Metadata.String), &merged)
			}
			if merged == nil {
				merged = make(map[string]string)
			}
			for k, v := range req.Metadata {
				merged[k] = v
			}
			raw, _ := json.Marshal(merged)
			metadataStr = sql.NullString{String: string(raw), Valid: true}
		}

		// Update profile
		err = svcCtx.DB.UpdateAuthProfile(ctx, db.UpdateAuthProfileParams{
			ID:       req.Id,
			Name:     name,
			ApiKey:   apiKey,
			Model:    sql.NullString{String: model, Valid: model != ""},
			BaseUrl:  sql.NullString{String: baseUrl, Valid: baseUrl != ""},
			Priority: sql.NullInt64{Int64: priority, Valid: true},
			AuthType: existing.AuthType,
			Metadata: metadataStr,
		})
		if err != nil {
			httputil.Error(w, err)
			return
		}

		// Handle isActive toggle only when explicitly provided
		if req.IsActive != nil && *req.IsActive != (existing.IsActive.Int64 == 1) {
			isActiveVal := int64(0)
			if *req.IsActive {
				isActiveVal = 1
			}
			err = svcCtx.DB.ToggleAuthProfile(ctx, db.ToggleAuthProfileParams{
				ID:       req.Id,
				IsActive: sql.NullInt64{Int64: isActiveVal, Valid: true},
			})
			if err != nil {
				httputil.Error(w, err)
				return
			}
		}

		// Return updated profile
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
