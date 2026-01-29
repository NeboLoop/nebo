package provider

import (
	"context"
	"time"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ListAuthProfilesLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// List all auth profiles (API keys)
func NewListAuthProfilesLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ListAuthProfilesLogic {
	return &ListAuthProfilesLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ListAuthProfilesLogic) ListAuthProfiles() (resp *types.ListAuthProfilesResponse, err error) {
	profiles, err := l.svcCtx.DB.ListAuthProfiles(l.ctx)
	if err != nil {
		return nil, err
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

	return &types.ListAuthProfilesResponse{
		Profiles: result,
	}, nil
}
