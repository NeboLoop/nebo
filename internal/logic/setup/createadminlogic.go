package setup

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"time"

	"gobot/internal/db"
	"gobot/internal/svc"
	"gobot/internal/types"

	"github.com/golang-jwt/jwt/v5"
	"github.com/zeromicro/go-zero/core/logx"
	"golang.org/x/crypto/bcrypt"
)

type CreateAdminLogic struct {
	logx.Logger
	ctx    context.Context
	svcCtx *svc.ServiceContext
}

// Create the first admin user (only works when no admin exists)
func NewCreateAdminLogic(ctx context.Context, svcCtx *svc.ServiceContext) *CreateAdminLogic {
	return &CreateAdminLogic{
		Logger: logx.WithContext(ctx),
		ctx:    ctx,
		svcCtx: svcCtx,
	}
}

func (l *CreateAdminLogic) CreateAdmin(req *types.CreateAdminRequest) (resp *types.CreateAdminResponse, err error) {
	// Check if any admin already exists
	hasAdmin, err := l.svcCtx.DB.HasAdminUser(l.ctx)
	if err != nil {
		l.Errorf("Failed to check for admin user: %v", err)
		return nil, err
	}

	if hasAdmin == 1 {
		return nil, fmt.Errorf("admin user already exists")
	}

	// Check if email already exists
	exists, err := l.svcCtx.DB.CheckEmailExists(l.ctx, req.Email)
	if err != nil {
		return nil, fmt.Errorf("failed to check email: %w", err)
	}
	if exists == 1 {
		return nil, fmt.Errorf("email already exists")
	}

	// Hash password
	hash, err := bcrypt.GenerateFromPassword([]byte(req.Password), bcrypt.DefaultCost)
	if err != nil {
		return nil, fmt.Errorf("failed to hash password: %w", err)
	}

	// Create admin user
	userID := generateID()
	user, err := l.svcCtx.DB.CreateUserWithRole(l.ctx, db.CreateUserWithRoleParams{
		ID:           userID,
		Email:        req.Email,
		PasswordHash: string(hash),
		Name:         req.Name,
		Role:         "admin",
	})
	if err != nil {
		return nil, fmt.Errorf("failed to create admin user: %w", err)
	}

	// Create default preferences
	_, err = l.svcCtx.DB.CreateUserPreferences(l.ctx, userID)
	if err != nil {
		return nil, fmt.Errorf("failed to create preferences: %w", err)
	}

	// Generate tokens
	now := time.Now()
	accessExpiry := now.Add(time.Duration(l.svcCtx.Config.Auth.AccessExpire) * time.Second)
	refreshExpiry := now.Add(time.Duration(l.svcCtx.Config.Auth.RefreshTokenExpire) * time.Second)

	// Create access token
	claims := jwt.MapClaims{
		"userId": userID,
		"email":  req.Email,
		"iat":    now.Unix(),
		"exp":    accessExpiry.Unix(),
	}
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, claims)
	accessToken, err := token.SignedString([]byte(l.svcCtx.Config.Auth.AccessSecret))
	if err != nil {
		return nil, fmt.Errorf("failed to sign token: %w", err)
	}

	// Create refresh token
	refreshToken := generateToken()
	tokenHash := hashToken(refreshToken)

	_, err = l.svcCtx.DB.CreateRefreshToken(l.ctx, db.CreateRefreshTokenParams{
		ID:        generateID(),
		UserID:    userID,
		TokenHash: tokenHash,
		ExpiresAt: refreshExpiry.Unix(),
	})
	if err != nil {
		return nil, fmt.Errorf("failed to store refresh token: %w", err)
	}

	l.Infof("Admin user created: %s", req.Email)

	return &types.CreateAdminResponse{
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
	}, nil
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
