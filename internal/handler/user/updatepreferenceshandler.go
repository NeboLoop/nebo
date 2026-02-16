package user

import (
	"net/http"

	"github.com/neboloop/nebo/internal/httputil"
	"github.com/neboloop/nebo/internal/svc"
	"github.com/neboloop/nebo/internal/types"
)

func UpdatePreferencesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.UpdatePreferencesRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Return the updated values
		httputil.OkJSON(w, &types.GetPreferencesResponse{
			Preferences: types.UserPreferences{
				EmailNotifications: req.EmailNotifications,
				MarketingEmails:    req.MarketingEmails,
				Timezone:           req.Timezone,
				Language:           req.Language,
				Theme:              req.Theme,
			},
		})
	}
}
