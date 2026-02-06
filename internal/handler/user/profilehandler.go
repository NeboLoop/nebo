package user

import (
	"context"
	"database/sql"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/auth"
	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"
)

// Default user ID for single-user personal assistant mode
const defaultUserID = "default-user"
const defaultUserEmail = "user@local.nebo.bot"
const defaultUserName = "User"

// ensureDefaultUserExists creates the default user if it doesn't exist (for single-user mode)
func ensureDefaultUserExists(ctx context.Context, svcCtx *svc.ServiceContext) error {
	// Check if user exists
	_, err := svcCtx.DB.GetUserByID(ctx, defaultUserID)
	if err == nil {
		return nil // User exists
	}
	if err != sql.ErrNoRows {
		return err // Unexpected error
	}

	// Create default user with placeholder password (not used in single-user mode)
	_, err = svcCtx.DB.CreateUser(ctx, db.CreateUserParams{
		ID:           defaultUserID,
		Email:        defaultUserEmail,
		PasswordHash: "not-used-single-user-mode",
		Name:         defaultUserName,
	})
	if err != nil {
		// Ignore duplicate key error (race condition)
		if !isDuplicateKeyError(err) {
			return err
		}
	}
	return nil
}

func isDuplicateKeyError(err error) bool {
	if err == nil {
		return false
	}
	errStr := err.Error()
	return strings.Contains(errStr, "UNIQUE constraint failed")
}

// GetUserProfileHandler returns the current user's profile
func GetUserProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Try to get user ID from JWT, fall back to default for single-user mode
		userIDStr := defaultUserID
		if userID, err := auth.GetUserIDFromContext(ctx); err == nil {
			userIDStr = userID.String()
		}

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		profile, err := svcCtx.DB.GetUserProfile(ctx, userIDStr)
		if err != nil {
			if err == sql.ErrNoRows {
				// Return empty profile if not exists
				httputil.OkJSON(w, &types.GetUserProfileResponse{
					Profile: types.UserProfile{
						UserId:              userIDStr,
						OnboardingCompleted: false,
						CreatedAt:           time.Now().Format(time.RFC3339),
						UpdatedAt:           time.Now().Format(time.RFC3339),
					},
				})
				return
			}
			logging.Errorf("Failed to get user profile: %v", err)
			httputil.InternalError(w, "failed to get profile")
			return
		}

		httputil.OkJSON(w, &types.GetUserProfileResponse{
			Profile: dbProfileToType(profile),
		})
	}
}

// UpdateUserProfileHandler updates the current user's profile
func UpdateUserProfileHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		// Try to get user ID from JWT, fall back to default for single-user mode
		userIDStr := defaultUserID
		if userID, err := auth.GetUserIDFromContext(ctx); err == nil {
			userIDStr = userID.String()
		}

		var req types.UpdateUserProfileRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		if svcCtx.DB == nil {
			httputil.InternalError(w, "database not configured")
			return
		}

		// Ensure default user exists for single-user mode
		if userIDStr == defaultUserID {
			if err := ensureDefaultUserExists(ctx, svcCtx); err != nil {
				logging.Errorf("Failed to ensure default user exists: %v", err)
				httputil.InternalError(w, "failed to initialize user")
				return
			}
		}

		// Convert interests array to JSON string
		var interestsJSON sql.NullString
		if len(req.Interests) > 0 {
			data, _ := json.Marshal(req.Interests)
			interestsJSON = sql.NullString{String: string(data), Valid: true}
		}

		// Convert onboardingCompleted to sql.NullInt64
		var onboardingCompleted sql.NullInt64
		if req.OnboardingCompleted != nil {
			if *req.OnboardingCompleted {
				onboardingCompleted = sql.NullInt64{Int64: 1, Valid: true}
			} else {
				onboardingCompleted = sql.NullInt64{Int64: 0, Valid: true}
			}
		}

		// Upsert the profile
		profile, err := svcCtx.DB.UpsertUserProfile(ctx, db.UpsertUserProfileParams{
			UserID:              userIDStr,
			DisplayName:         toNullString(req.DisplayName),
			Bio:                 toNullString(req.Bio),
			Location:            toNullString(req.Location),
			Timezone:            toNullString(req.Timezone),
			Occupation:          toNullString(req.Occupation),
			Interests:           interestsJSON,
			CommunicationStyle:  toNullString(req.CommunicationStyle),
			Goals:               toNullString(req.Goals),
			Context:             toNullString(req.Context),
			OnboardingCompleted: onboardingCompleted,
		})
		if err != nil {
			logging.Errorf("Failed to update user profile: %v", err)
			httputil.InternalError(w, "failed to update profile")
			return
		}

		httputil.OkJSON(w, &types.UpdateUserProfileResponse{
			Profile: dbProfileToType(profile),
		})
	}
}

// Helper functions

func toNullString(s string) sql.NullString {
	if s == "" {
		return sql.NullString{}
	}
	return sql.NullString{String: s, Valid: true}
}

func fromNullString(ns sql.NullString) string {
	if ns.Valid {
		return ns.String
	}
	return ""
}

func dbProfileToType(profile db.UserProfile) types.UserProfile {
	var interests []string
	if profile.Interests.Valid && profile.Interests.String != "" {
		json.Unmarshal([]byte(profile.Interests.String), &interests)
	}

	return types.UserProfile{
		UserId:              profile.UserID,
		DisplayName:         fromNullString(profile.DisplayName),
		Bio:                 fromNullString(profile.Bio),
		Location:            fromNullString(profile.Location),
		Timezone:            fromNullString(profile.Timezone),
		Occupation:          fromNullString(profile.Occupation),
		Interests:           interests,
		CommunicationStyle:  fromNullString(profile.CommunicationStyle),
		Goals:               fromNullString(profile.Goals),
		Context:             fromNullString(profile.Context),
		OnboardingCompleted: profile.OnboardingCompleted.Valid && profile.OnboardingCompleted.Int64 == 1,
		OnboardingStep:      fromNullString(profile.OnboardingStep),
		CreatedAt:           time.Unix(profile.CreatedAt, 0).Format(time.RFC3339),
		UpdatedAt:           time.Unix(profile.UpdatedAt, 0).Format(time.RFC3339),
	}
}
