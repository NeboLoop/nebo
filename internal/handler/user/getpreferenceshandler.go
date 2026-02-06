package user

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
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
