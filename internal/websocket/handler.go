package websocket

import (
	"net/http"

	"github.com/nebolabs/nebo/internal/middleware"
	"github.com/nebolabs/nebo/internal/realtime"

	"github.com/google/uuid"
	"github.com/gorilla/websocket"
	"github.com/nebolabs/nebo/internal/logging"
)

var upgrader = websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 1024,
	CheckOrigin: func(r *http.Request) bool {
		// Allow all origins in development
		// TODO: Tighten this in production to check specific origins
		return true
	},
}

// Handler returns an HTTP handler function for WebSocket upgrades
func Handler(hub *realtime.Hub) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		// Extract user_id from JWT token (required for authenticated connections)
		userID := extractUserIDFromJWT(r)

		// Generate client ID (unique per connection)
		clientID := r.URL.Query().Get("clientId")
		if clientID == "" {
			clientID = "client-" + uuid.New().String()[:8]
		}

		// If no JWT token found, fall back to query param (for backwards compatibility during transition)
		// TODO: Remove this fallback once all clients send JWT cookies
		if userID == "" {
			userID = r.URL.Query().Get("userId")
			if userID != "" {
				logging.Infof("WebSocket using legacy userId query param (deprecated): %s", userID)
			}
		}

		// Require authentication for WebSocket connections
		if userID == "" {
			logging.Infof("WebSocket connection rejected: no authentication")
			http.Error(w, "authentication required", http.StatusUnauthorized)
			return
		}

		logging.Infof("Serving WebSocket for clientID: %s, userID: %s", clientID, userID)

		// Upgrade HTTP connection to WebSocket
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			logging.Errorf("WebSocket upgrade error: %v", err)
			return
		}

		// Delegate to the realtime hub
		realtime.ServeWS(hub, conn, clientID, userID)
	}
}

// extractUserIDFromJWT extracts user_id from JWT token in cookie or Authorization header
func extractUserIDFromJWT(r *http.Request) string {
	// Try Authorization header first (Bearer token)
	authHeader := r.Header.Get("Authorization")
	if authHeader != "" && len(authHeader) > 7 && authHeader[:7] == "Bearer " {
		token := authHeader[7:]
		if claims, err := middleware.ParseJWTClaimsFromToken(token); err == nil {
			return claims.Sub
		}
	}

	// Try cookie (nebo_token)
	cookie, err := r.Cookie("nebo_token")
	if err == nil && cookie.Value != "" {
		if claims, err := middleware.ParseJWTClaimsFromToken(cookie.Value); err == nil {
			return claims.Sub
		}
	}

	// Try levee_access_token cookie (Levee SDK)
	cookie, err = r.Cookie("levee_access_token")
	if err == nil && cookie.Value != "" {
		if claims, err := middleware.ParseJWTClaimsFromToken(cookie.Value); err == nil {
			return claims.Sub
		}
	}

	return ""
}
