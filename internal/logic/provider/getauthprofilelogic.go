package provider

import (
	"context"
	"time"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type GetAuthProfileLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Get auth profile by ID
func NewGetAuthProfileLogic(ctx context.Context, svcCtx *svc.ServiceContext) *GetAuthProfileLogic {
	return &GetAuthProfileLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *GetAuthProfileLogic) GetAuthProfile(req *types.GetAuthProfileRequest) (resp *types.GetAuthProfileResponse, err error) {
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
