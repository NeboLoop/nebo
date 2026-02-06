package setup

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"net/http"
	"time"

	"github.com/nebolabs/nebo/internal/db"
	"github.com/nebolabs/nebo/internal/httputil"
	"github.com/nebolabs/nebo/internal/logging"
	"github.com/nebolabs/nebo/internal/svc"
	"github.com/nebolabs/nebo/internal/types"

	"github.com/golang-jwt/jwt/v5"
	"golang.org/x/crypto/bcrypt"
)

func CreateAdminHandler(svcCtx *svc.ServiceContext) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		var req types.CreateAdminRequest
		if err := httputil.Parse(r, &req); err != nil {
			httputil.Error(w, err)
			return
		}

		// Check if any admin already exists
		hasAdmin, err := svcCtx.DB.HasAdminUser(ctx)
		if err != nil {
			logging.Errorf("Failed to check for admin user: %v", err)
			httputil.Error(w, err)
			return
		}

		if hasAdmin == 1 {
			httputil.Error(w, fmt.Errorf("admin user already exists"))
			return
		}

		// Check if email already exists
		exists, err := svcCtx.DB.CheckEmailExists(ctx, req.Email)
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to check email: %w", err))
			return
		}
		if exists == 1 {
			httputil.Error(w, fmt.Errorf("email already exists"))
			return
		}

		// Hash password
		hash, err := bcrypt.GenerateFromPassword([]byte(req.Password), bcrypt.DefaultCost)
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to hash password: %w", err))
			return
		}

		// Create admin user
		userID := generateID()
		user, err := svcCtx.DB.CreateUserWithRole(ctx, db.CreateUserWithRoleParams{
			ID:           userID,
			Email:        req.Email,
			PasswordHash: string(hash),
			Name:         req.Name,
			Role:         "admin",
		})
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to create admin user: %w", err))
			return
		}

		// Create default preferences
		_, err = svcCtx.DB.CreateUserPreferences(ctx, userID)
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to create preferences: %w", err))
			return
		}

		// Generate tokens
		now := time.Now()
		accessExpiry := now.Add(time.Duration(svcCtx.Config.Auth.AccessExpire) * time.Second)
		refreshExpiry := now.Add(time.Duration(svcCtx.Config.Auth.RefreshTokenExpire) * time.Second)

		// Create access token
		claims := jwt.MapClaims{
			"userId": userID,
			"email":  req.Email,
			"iat":    now.Unix(),
			"exp":    accessExpiry.Unix(),
		}
		token := jwt.NewWithClaims(jwt.SigningMethodHS256, claims)
		accessToken, err := token.SignedString([]byte(svcCtx.Config.Auth.AccessSecret))
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to sign token: %w", err))
			return
		}

		// Create refresh token
		refreshToken := generateToken()
		tokenHash := hashToken(refreshToken)

		_, err = svcCtx.DB.CreateRefreshToken(ctx, db.CreateRefreshTokenParams{
			ID:        generateID(),
			UserID:    userID,
			TokenHash: tokenHash,
			ExpiresAt: refreshExpiry.Unix(),
		})
		if err != nil {
			httputil.Error(w, fmt.Errorf("failed to store refresh token: %w", err))
			return
		}

		logging.Infof("Admin user created: %s", req.Email)

		httputil.OkJSON(w, &types.CreateAdminResponse{
			Token:        accessToken,
			RefreshToken: refreshToken,
			ExpiresAt:    accessExpiry.UnixMilli(),
			User: types.User{
				Id:            userID,
				Email:         user.Email,
				Name:          user.Name,
				EmailVerified: false,
				CreatedAt:     time.Unix(user.CreatedAt, 0).Format("2006-01-02T15:04:05Z"),
				UpdatedAt:     time.Unix(user.UpdatedAt, 0).Format("2006-01-02T15:04:05Z"),
			},
		})
	}
}

// generateID creates a random ID
func generateID() string {
	b := make([]byte, 16)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// generateToken creates a random token
func generateToken() string {
	b := make([]byte, 32)
	rand.Read(b)
	return hex.EncodeToString(b)
}

// hashToken hashes a token for storage
func hashToken(token string) string {
	b := make([]byte, 32)
	copy(b, []byte(token))
	return hex.EncodeToString(b)
}
