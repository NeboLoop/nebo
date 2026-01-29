package provider

import (
	"context"

	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type DeleteAuthProfileLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Delete auth profile
func NewDeleteAuthProfileLogic(ctx context.Context, svcCtx *svc.ServiceContext) *DeleteAuthProfileLogic {
	return &DeleteAuthProfileLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *DeleteAuthProfileLogic) DeleteAuthProfile(req *types.DeleteAuthProfileRequest) (resp *types.MessageResponse, err error) {
	err = l.svcCtx.DB.DeleteAuthProfile(l.ctx, req.Id)
	if err != nil {
		return nil, err
	}

	return &types.MessageResponse{
		Message: "Provider deleted successfully",
	}, nil
}
