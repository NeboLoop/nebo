package provider

import (
	"context"

	"gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type UpdateTaskRoutingLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Update task routing configuration
func NewUpdateTaskRoutingLogic(ctx context.Context, svcCtx *svc.ServiceContext) *UpdateTaskRoutingLogic {
	return &UpdateTaskRoutingLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *UpdateTaskRoutingLogic) UpdateTaskRouting(req *types.UpdateTaskRoutingRequest) (resp *types.MessageResponse, err error) {
	config := provider.GetModelsConfig()

	// Initialize task routing if nil
	if config.TaskRouting == nil {
		config.TaskRouting = &provider.TaskRouting{}
	}

	// Update routing configuration
	if req.Vision != "" {
		config.TaskRouting.Vision = req.Vision
	}
	if req.Audio != "" {
		config.TaskRouting.Audio = req.Audio
	}
	if req.Reasoning != "" {
		config.TaskRouting.Reasoning = req.Reasoning
	}
	if req.Code != "" {
		config.TaskRouting.Code = req.Code
	}
	if req.General != "" {
		config.TaskRouting.General = req.General
	}
	if req.Fallbacks != nil {
		config.TaskRouting.Fallbacks = req.Fallbacks
	}

	// Update aliases if provided
	if req.Aliases != nil {
		config.Aliases = make([]provider.ModelAlias, len(req.Aliases))
		for i, a := range req.Aliases {
			config.Aliases[i] = provider.ModelAlias{
				Alias:   a.Alias,
				ModelId: a.ModelId,
			}
		}
	}

	// Save to YAML
	if err := provider.SaveModels(config); err != nil {
		return nil, err
	}

	return &types.MessageResponse{
		Message: "Task routing updated successfully",
	}, nil
}
