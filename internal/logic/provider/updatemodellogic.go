package provider

import (
	"context"

	"gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type UpdateModelLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Update model settings (active, kind, preferred)
func NewUpdateModelLogic(ctx context.Context, svcCtx *svc.ServiceContext) *UpdateModelLogic {
	return &UpdateModelLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *UpdateModelLogic) UpdateModel(req *types.UpdateModelRequest) (resp *types.MessageResponse, err error) {
	update := provider.ModelUpdate{}

	// Only set fields that were provided (nil means not sent)
	if req.Active != nil {
		update.Active = req.Active
	}
	if req.Kind != nil {
		update.Kind = req.Kind
	}
	if req.Preferred != nil {
		update.Preferred = req.Preferred
	}

	if err := provider.UpdateModel(req.Provider, req.ModelId, update); err != nil {
		return nil, err
	}

	return &types.MessageResponse{
		Message: "Model " + req.ModelId + " updated",
	}, nil
}
