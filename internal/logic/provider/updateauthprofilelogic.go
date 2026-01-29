package provider

import (
	"context"
	"database/sql"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type UpdateAuthProfileLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Update auth profile
func NewUpdateAuthProfileLogic(ctx context.Context, svcCtx *svc.ServiceContext) *UpdateAuthProfileLogic {
	return &UpdateAuthProfileLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *UpdateAuthProfileLogic) UpdateAuthProfile(req *types.UpdateAuthProfileRequest) (resp *types.GetAuthProfileResponse, err error) {
	// Get existing profile first
	existing, err := l.svcCtx.DB.GetAuthProfile(l.ctx, req.Id)
	if err != nil {
		return nil, err
	}

	// Use existing values if not provided
	name := req.Name
	if name == "" {
		name = existing.Name
	}

	apiKey := req.ApiKey
	if apiKey == "" {
		apiKey = existing.ApiKey
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

	// Update profile
	err = l.svcCtx.DB.UpdateAuthProfile(l.ctx, db.UpdateAuthProfileParams{
		ID:       req.Id,
		Name:     name,
		ApiKey:   apiKey,
		Model:    sql.NullString{String: model, Valid: model != ""},
		BaseUrl:  sql.NullString{String: baseUrl, Valid: baseUrl != ""},
		Priority: sql.NullInt64{Int64: priority, Valid: true},
	})
	if err != nil {
		return nil, err
	}

	// Handle isActive toggle if needed
	if req.IsActive != (existing.IsActive.Int64 == 1) {
		isActiveVal := int64(0)
		if req.IsActive {
			isActiveVal = 1
		}
		err = l.svcCtx.DB.ToggleAuthProfile(l.ctx, db.ToggleAuthProfileParams{
			ID:       req.Id,
			IsActive: sql.NullInt64{Int64: isActiveVal, Valid: true},
		})
		if err != nil {
			return nil, err
		}
	}

	// Return updated profile
	profile, err := l.svcCtx.DB.GetAuthProfile(l.ctx, req.Id)
	if err != nil {
		return nil, err
	}

	return &types.GetAuthProfileResponse{
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
	}, nil
}
