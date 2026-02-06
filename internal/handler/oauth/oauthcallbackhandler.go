package oauth

import (
	"fmt"
	"net/http"

	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// OAuth callback - exchange code for tokens
// Deprecated: OAuth callbacks are handled directly at /oauth/{provider}/callback
// This endpoint exists for API compatibility but should not be called directly.
func OAuthCallbackHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req types.OAuthLoginRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		httputil.Error(w, fmt.Errorf("OAuth callbacks should use /oauth/%s/callback (browser redirect), not the API endpoint", req.Provider))
	}
}
