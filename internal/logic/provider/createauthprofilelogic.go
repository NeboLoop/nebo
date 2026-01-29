package provider

import (
	"context"
	"database/sql"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/google/uuid"
	"github.com/zeromicro/go-zero/core/logx"
)

type CreateAuthProfileLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Create a new auth profile
func NewCreateAuthProfileLogic(ctx context.Context, svcCtx *svc.ServiceContext) *CreateAuthProfileLogic {
	return &CreateAuthProfileLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *CreateAuthProfileLogic) CreateAuthProfile(req *types.CreateAuthProfileRequest) (resp *types.CreateAuthProfileResponse, err error) {
	id := uuid.New().String()

	priority := int64(req.Priority)
	if priority == 0 {
		priority = 10
	}

	profile, err := l.svcCtx.DB.CreateAuthProfile(l.ctx, db.CreateAuthProfileParams{
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
		return nil, err
	}

	return &types.CreateAuthProfileResponse{
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
