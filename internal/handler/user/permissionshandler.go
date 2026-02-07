package user

import (
	"database/sql"
	"encoding/json"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// GetToolPermissionsHandler returns the current user's tool permissions
func GetToolPermissionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		userIDStr := defaultUserID
		if userID, err := auth.GetUserIDFromContext(ctx); err == nil {
			userIDStr = userID.String()
		}

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		permJSON, err := svcCtx.DB.GetToolPermissions(ctx, userIDStr)
		if err != nil {
			if err == sql.ErrNoRows {
				// No profile yet — return default permissions (only chat enabled)
				httputil.OkJSON(w, &types.GetToolPermissionsResponse{
					Permissions: defaultPermissions(),
				})
				return
			}
			logging.Errorf("Failed to get tool permissions: %v", err)
			httputil.InternalError(w, "failed to get permissions")
			return
		}

		perms := make(map[string]bool)
		if err := json.Unmarshal([]byte(permJSON), &perms); err != nil || len(perms) == 0 {
			perms = defaultPermissions()
		}

		httputil.OkJSON(w, &types.GetToolPermissionsResponse{
			Permissions: perms,
		})
	}
}

// UpdateToolPermissionsHandler updates tool permissions
func UpdateToolPermissionsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		userIDStr := defaultUserID
		if userID, err := auth.GetUserIDFromContext(ctx); err == nil {
			userIDStr = userID.String()
		}

		var req types.UpdateToolPermissionsRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		// Ensure default user and profile exist
		if userIDStr == defaultUserID {
			if err := ensureDefaultUserExists(ctx, svcCtx); err != nil {
				logging.Errorf("Failed to ensure default user exists: %v", err)
				httputil.InternalError(w, "failed to initialize user")
				return
			}
			// Ensure profile exists via upsert with minimal data
			_, err := svcCtx.DB.UpsertUserProfile(ctx, db.UpsertUserProfileParams{
				UserID: userIDStr,
			})
			if err != nil {
				logging.Errorf("Failed to ensure user profile exists: %v", err)
				httputil.InternalError(w, "failed to initialize profile")
				return
			}
		}

		// Serialize permissions to JSON
		data, err := json.Marshal(req.Permissions)
		if err != nil {
			httputil.InternalError(w, "failed to serialize permissions")
			return
		}

		err = svcCtx.DB.UpdateToolPermissions(ctx, db.UpdateToolPermissionsParams{
			ToolPermissions: sql.NullString{String: string(data), Valid: true},
			UserID:          userIDStr,
		})
		if err != nil {
			logging.Errorf("Failed to update tool permissions: %v", err)
			httputil.InternalError(w, "failed to update permissions")
			return
		}

		httputil.OkJSON(w, &types.UpdateToolPermissionsResponse{
			Permissions: req.Permissions,
		})
	}
}

// AcceptTermsHandler records that the user accepted the terms
func AcceptTermsHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		userIDStr := defaultUserID
		if userID, err := auth.GetUserIDFromContext(ctx); err == nil {
			userIDStr = userID.String()
		}

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		// Ensure default user and profile exist
		if userIDStr == defaultUserID {
			if err := ensureDefaultUserExists(ctx, svcCtx); err != nil {
				logging.Errorf("Failed to ensure default user exists: %v", err)
				httputil.InternalError(w, "failed to initialize user")
				return
			}
			_, err := svcCtx.DB.UpsertUserProfile(ctx, db.UpsertUserProfileParams{
				UserID: userIDStr,
			})
			if err != nil {
				logging.Errorf("Failed to ensure user profile exists: %v", err)
				httputil.InternalError(w, "failed to initialize profile")
				return
			}
		}

		err := svcCtx.DB.AcceptTerms(ctx, userIDStr)
		if err != nil {
			logging.Errorf("Failed to accept terms: %v", err)
			httputil.InternalError(w, "failed to accept terms")
			return
		}

		httputil.OkJSON(w, &types.AcceptTermsResponse{
			AcceptedAt: time.Now().Format(time.RFC3339),
		})
	}
}

// defaultPermissions returns the default tool permissions (only chat/agent enabled)
func defaultPermissions() map[string]bool {
	return map[string]bool{
		"chat":     true,
		"file":     false,
		"shell":    false,
		"web":      false,
		"contacts": false,
		"desktop":  false,
		"media":    false,
		"system":   false,
	}
}
