package provider

import (
	"context"

	"gobot/agent/ai"
	"gobot/internal/provider"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/zeromicro/go-zero/core/logx"
)

type ListModelsLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// List all available models from YAML cache
func NewListModelsLogic(ctx context.Context, svcCtx *svc.ServiceContext) *ListModelsLogic {
	return &ListModelsLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *ListModelsLogic) ListModels() (resp *types.ListModelsResponse, err error) {
	config := provider.GetModelsConfig()

	result := make(map[string][]types.ModelInfo)
	for providerType, models := range config.Providers {
		modelList := make([]types.ModelInfo, len(models))
		for i, m := range models {
			info := types.ModelInfo{
				Id:            m.ID,
				DisplayName:   m.DisplayName,
				ContextWindow: m.ContextWindow,
				Capabilities:  m.Capabilities,
				IsActive:      m.IsActive(),
			}
			if m.Pricing != nil {
				info.Pricing = &types.ModelPricing{
					Input:       m.Pricing.Input,
					Output:      m.Pricing.Output,
					CachedInput: m.Pricing.CachedInput,
				}
			}
			modelList[i] = info
		}
		result[providerType] = modelList
	}

	// Include task routing if configured
	var taskRouting *types.TaskRouting
	if config.TaskRouting != nil {
		taskRouting = &types.TaskRouting{
			Vision:    config.TaskRouting.Vision,
			Reasoning: config.TaskRouting.Reasoning,
			Code:      config.TaskRouting.Code,
			General:   config.TaskRouting.General,
			Fallbacks: config.TaskRouting.Fallbacks,
		}
	}

	// Detect available CLI tools
	cliAvailability := &types.CLIAvailability{
		Claude: ai.CheckCLIAvailable("claude"),
		Codex:  ai.CheckCLIAvailable("codex"),
		Gemini: ai.CheckCLIAvailable("gemini"),
	}

	return &types.ListModelsResponse{
		Models:        result,
		TaskRouting:   taskRouting,
		AvailableCLIs: cliAvailability,
	}, nil
}
