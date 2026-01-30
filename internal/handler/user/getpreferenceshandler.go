package user

import (
	"net/http"

	"nebo/internal/httputil"
	"nebo/internal/svc"
	"nebo/internal/types"
)

func GetPreferencesHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Preferences are stored locally - return defaults
		httputil.OkJSON(w, &types.GetPreferencesResponse{
			Preferences: types.UserPreferences{
				EmailNotifications: true,
				MarketingEmails:    true,
				Timezone:           "UTC",
				Language:           "en",
				Theme:              "system",
			},
		})
	}
}
