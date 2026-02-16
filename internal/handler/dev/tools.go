package dev

import (
	"net/http"

	"github.com/neboloop/nebo/internal/agent/ai"
	"github.com/neboloop/nebo/internal/agent/tools"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

// getToolRegistry extracts the *tools.Registry from the ServiceContext.
func getToolRegistry(svcCtx *svc.ServiceContext) *tools.Registry {
	r := svcCtx.ToolRegistry()
	if r == nil {
		return nil
	}
	reg, ok := r.(*tools.Registry)
	if !ok {
		return nil
	}
	return reg
}

// ListToolsHandler returns all registered tools with their schemas.
func ListToolsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		reg := getToolRegistry(svcCtx)
		if reg == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "tool registry not available (agent not connected)")
			return
		}

		defs := reg.List()
		items := make([]types.ToolDefinitionItem, 0, len(defs))
		for _, d := range defs {
			items = append(items, types.ToolDefinitionItem{
				Name:        d.Name,
				Description: d.Description,
				Schema:      d.InputSchema,
			})
		}

		httputil.OkJSON(w, &types.ListToolsResponse{Tools: items})
	}
}

// ToolExecuteHandler executes a tool directly (bypasses the agent).
func ToolExecuteHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.ToolExecuteRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if req.Tool == "" {
			httputil.ErrorWithCode(w, http.StatusBadRequest, "tool name is required")
			return
		}

		reg := getToolRegistry(svcCtx)
		if reg == nil {
			httputil.ErrorWithCode(w, http.StatusServiceUnavailable, "tool registry not available (agent not connected)")
			return
		}

		result := reg.Execute(r.Context(), &ai.ToolCall{
			ID:    "dev-tool-test",
			Name:  req.Tool,
			Input: req.Input,
		})

		httputil.OkJSON(w, &types.ToolExecuteResponse{
			Content: result.Content,
			IsError: result.IsError,
		})
	}
}
