package provider

import (
	"context"

	"gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ToggleModelLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Toggle model active status
func NewToggleModelLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ToggleModelLogic {
	return &ToggleModelLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ToggleModelLogic) ToggleModel(req *types.ToggleModelRequest) (resp *types.MessageResponse, err error) {
	if err := provider.SetModelActive(req.Provider, req.ModelId, req.Active); err != nil {
		return nil, err
	}

	status := "enabled"
	if !req.Active {
		status = "disabled"
	}

	return &types.MessageResponse{
		Message: "Model " + req.ModelId + " " + status,
	}, nil
}
