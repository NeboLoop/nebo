package provider

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

// Delete auth profile
func DeleteAuthProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.DeleteAuthProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		err := svcCtx.DB.DeleteAuthProfile(ctx, req.Id)
		if err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.OkJSON(w, &types.MessageResponse{
			Message: "Provider deleted successfully",
		})
	}
}
