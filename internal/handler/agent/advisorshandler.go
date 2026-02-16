package agent

import (
	"net/http"
	"regexp"

	"github.com/neboloop/nebo/internal/db"
	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

var slugRe = regexp.MustCompile(`^[a-z][a-z0-9_-]{0,63}$`)

func dbAdvisorToItem(a db.Advisor) types.AdvisorItem {
	return types.AdvisorItem{
		ID:             a.ID,
		Name:           a.Name,
		Role:           a.Role,
		Description:    a.Description,
		Priority:       int(a.Priority),
		Enabled:        a.Enabled == 1,
		MemoryAccess:   a.MemoryAccess == 1,
		Persona:        a.Persona,
		TimeoutSeconds: int(a.TimeoutSeconds),
	}
}

func ListAdvisorsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		advisors, err := svcCtx.DB.ListAdvisors(r.Context())
		if err != nil {
			httputil.InternalError(w, "failed to list advisors")
			return
		}
		items := make([]types.AdvisorItem, len(advisors))
		for i, a := range advisors {
			items[i] = dbAdvisorToItem(a)
		}
		httputil.OkJSON(w, &types.ListAdvisorsResponse{Advisors: items})
	}
}

func GetAdvisorHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		name := httputil.PathVar(r, "name")
		advisor, err := svcCtx.DB.GetAdvisor(r.Context(), name)
		if err != nil {
			httputil.NotFound(w, "advisor not found")
			return
		}
		httputil.OkJSON(w, &types.GetAdvisorResponse{Advisor: dbAdvisorToItem(advisor)})
	}
}

func CreateAdvisorHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.CreateAdvisorRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}
		if !slugRe.MatchString(req.Name) {
			httputil.BadRequest(w, "name must be lowercase alphanumeric with hyphens/underscores (e.g. 'my-advisor')")
			return
		}
		timeout := req.TimeoutSeconds
		if timeout <= 0 {
			timeout = 30
		}
		var memAccess int64
		if req.MemoryAccess {
			memAccess = 1
		}
		advisor, err := svcCtx.DB.CreateAdvisor(r.Context(), db.CreateAdvisorParams{
			Name:           req.Name,
			Role:           req.Role,
			Description:    req.Description,
			Priority:       int64(req.Priority),
			Enabled:        1,
			MemoryAccess:   memAccess,
			Persona:        req.Persona,
			TimeoutSeconds: int64(timeout),
		})
		if err != nil {
			httputil.BadRequest(w, "advisor with this name already exists")
			return
		}
		httputil.OkJSON(w, &types.GetAdvisorResponse{Advisor: dbAdvisorToItem(advisor)})
	}
}

func UpdateAdvisorHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		name := httputil.PathVar(r, "name")
		var req types.UpdateAdvisorRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		existing, err := svcCtx.DB.GetAdvisor(r.Context(), name)
		if err != nil {
			httputil.NotFound(w, "advisor not found")
			return
		}

		// Merge partial update
		params := db.UpdateAdvisorParams{
			Name:           name,
			Role:           existing.Role,
			Description:    existing.Description,
			Priority:       existing.Priority,
			Enabled:        existing.Enabled,
			MemoryAccess:   existing.MemoryAccess,
			Persona:        existing.Persona,
			TimeoutSeconds: existing.TimeoutSeconds,
		}
		if req.Role != nil {
			params.Role = *req.Role
		}
		if req.Description != nil {
			params.Description = *req.Description
		}
		if req.Priority != nil {
			params.Priority = int64(*req.Priority)
		}
		if req.Enabled != nil {
			if *req.Enabled {
				params.Enabled = 1
			} else {
				params.Enabled = 0
			}
		}
		if req.MemoryAccess != nil {
			if *req.MemoryAccess {
				params.MemoryAccess = 1
			} else {
				params.MemoryAccess = 0
			}
		}
		if req.Persona != nil {
			params.Persona = *req.Persona
		}
		if req.TimeoutSeconds != nil {
			params.TimeoutSeconds = int64(*req.TimeoutSeconds)
		}

		if err := svcCtx.DB.UpdateAdvisor(r.Context(), params); err != nil {
			httputil.InternalError(w, "failed to update advisor")
			return
		}

		updated, err := svcCtx.DB.GetAdvisor(r.Context(), name)
		if err != nil {
			httputil.InternalError(w, "failed to fetch updated advisor")
			return
		}
		httputil.OkJSON(w, &types.GetAdvisorResponse{Advisor: dbAdvisorToItem(updated)})
	}
}

func DeleteAdvisorHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		name := httputil.PathVar(r, "name")
		if err := svcCtx.DB.DeleteAdvisor(r.Context(), name); err != nil {
			httputil.InternalError(w, "failed to delete advisor")
			return
		}
		httputil.OkJSON(w, &types.DeleteAdvisorResponse{Success: true})
	}
}
