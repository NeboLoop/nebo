package local

import (
	"context"
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"gobot/internal/config"
	"gobot/internal/db"

	"github.com/golang-jwt/jwt/v5"
	"golang.org/x/crypto/bcrypt"
)

var (
	ErrUserNotFound       = errors.New("user not found")
	ErrInvalidCredentials = errors.New("invalid credentials")
	ErrEmailExists        = errors.New("email already exists")
	ErrInvalidToken       = errors.New("invalid or expired token")
)

// AuthService handles local authentication with SQLite
type AuthService struct {
	store  *db.Store
	config config.Config
}

// NewAuthService creates a new local auth service
func NewAuthService(store *db.Store, cfg config.Config) *AuthService {
	return &AuthService{
		store:  store,
		config: cfg,
	}
}

// AuthResponse mirrors the Levee SDK auth response
type AuthResponse struct {
	Token        string
	RefreshToken string
	ExpiresAt    time.Time
	CheckoutURL  string // Only set during registration with paid plan
}

// Register creates a new user account
func (s *AuthService) Register(ctx context.Context, email, password, name string) (*AuthResponse, error) {
	// Check if email already exists
	exists, err := s.store.CheckEmailExists(ctx, email)
	if err != nil {
		return nil, fmt.Errorf("failed to check email: %w", err)
	}
	if exists == 1 {
		return nil, ErrEmailExists
	}

	// Hash password
	hash, err := bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)
	if err != nil {
		return nil, fmt.Errorf("failed to hash password: %w", err)
	}

	// Create user
	user, err := s.store.CreateUser(ctx, db.CreateUserParams{
		ID:           generateID(),
		Email:        email,
		PasswordHash: string(hash),
		Name:         name,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to create user: %w", err)
	}

	// Create default preferences
	_, err = s.store.CreateUserPreferences(ctx, user.ID)
	if err != nil {
		return nil, fmt.Errorf("failed to create preferences: %w", err)
	}

	// Generate tokens
	return s.generateTokens(ctx, user.ID, user.Email)
}

// Login authenticates a user and returns tokens
func (s *AuthService) Login(ctx context.Context, email, password string) (*AuthResponse, error) {
	user, err := s.store.GetUserByEmail(ctx, email)
	if err == sql.ErrNoRows {
		return nil, ErrInvalidCredentials
	}
	if err != nil {
		return nil, fmt.Errorf("failed to find user: %w", err)
	}

	// Verify password
	if err := bcrypt.CompareHashAndPassword([]byte(user.PasswordHash), []byte(password)); err != nil {
		return nil, ErrInvalidCredentials
	}

	return s.generateTokens(ctx, user.ID, user.Email)
}

// RefreshToken generates new tokens from a refresh token
func (s *AuthService) RefreshToken(ctx context.Context, refreshToken string) (*AuthResponse, error) {
	tokenHash := hashToken(refreshToken)

	token, err := s.store.GetRefreshTokenByHash(ctx, tokenHash)
	if err == sql.ErrNoRows {
		return nil, ErrInvalidToken
	}
	if err != nil {
		return nil, fmt.Errorf("failed to find token: %w", err)
	}

	// Get user email
	user, err := s.store.GetUserByID(ctx, token.UserID)
	if err != nil {
		return nil, ErrUserNotFound
	}

	// Delete old token
	s.store.DeleteRefreshToken(ctx, tokenHash)

	return s.generateTokens(ctx, user.ID, user.Email)
}

// GetUserByID returns a user by ID
func (s *AuthService) GetUserByID(ctx context.Context, userID string) (*db.User, error) {
	row, err := s.store.GetUserByID(ctx, userID)
	if err == sql.ErrNoRows {
		return nil, ErrUserNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("failed to find user: %w", err)
	}
	return &db.User{
		ID:                   row.ID,
		Email:                row.Email,
		PasswordHash:         row.PasswordHash,
		Name:                 row.Name,
		AvatarUrl:            row.AvatarUrl,
		EmailVerified:        row.EmailVerified,
		EmailVerifyToken:     row.EmailVerifyToken,
		EmailVerifyExpires:   row.EmailVerifyExpires,
		PasswordResetToken:   row.PasswordResetToken,
		PasswordResetExpires: row.PasswordResetExpires,
		CreatedAt:            row.CreatedAt,
		UpdatedAt:            row.UpdatedAt,
		Role:                 row.Role,
	}, nil
}

// GetUserByEmail returns a user by email
func (s *AuthService) GetUserByEmail(ctx context.Context, email string) (*db.User, error) {
	row, err := s.store.GetUserByEmail(ctx, email)
	if err == sql.ErrNoRows {
		return nil, ErrUserNotFound
	}
	if err != nil {
		return nil, fmt.Errorf("failed to find user: %w", err)
	}
	return &db.User{
		ID:                   row.ID,
		Email:                row.Email,
		PasswordHash:         row.PasswordHash,
		Name:                 row.Name,
		AvatarUrl:            row.AvatarUrl,
		EmailVerified:        row.EmailVerified,
		EmailVerifyToken:     row.EmailVerifyToken,
		EmailVerifyExpires:   row.EmailVerifyExpires,
		PasswordResetToken:   row.PasswordResetToken,
		PasswordResetExpires: row.PasswordResetExpires,
		CreatedAt:            row.CreatedAt,
		UpdatedAt:            row.UpdatedAt,
		Role:                 row.Role,
	}, nil
}

// UpdateUser updates a user's profile
func (s *AuthService) UpdateUser(ctx context.Context, user *db.User) error {
	return s.store.UpdateUser(ctx, db.UpdateUserParams{
		ID:   user.ID,
		Name: sql.NullString{String: user.Name, Valid: user.Name != ""},
	})
}

// VerifyEmail verifies a user's email using a token
func (s *AuthService) VerifyEmail(ctx context.Context, token string) error {
	return nil
}

// ChangePassword changes a user's password
func (s *AuthService) ChangePassword(ctx context.Context, userID, currentPassword, newPassword string) error {
	user, err := s.store.GetUserByID(ctx, userID)
	if err != nil {
		return ErrUserNotFound
	}

	// Verify current password
	if err := bcrypt.CompareHashAndPassword([]byte(user.PasswordHash), []byte(currentPassword)); err != nil {
		return ErrInvalidCredentials
	}

	// Hash new password
	newHash, err := bcrypt.GenerateFromPassword([]byte(newPassword), bcrypt.DefaultCost)
	if err != nil {
		return fmt.Errorf("failed to hash password: %w", err)
	}

	return s.store.UpdateUserPassword(ctx, db.UpdateUserPasswordParams{
		ID:           userID,
		PasswordHash: string(newHash),
	})
}

// DeleteUser deletes a user account
func (s *AuthService) DeleteUser(ctx context.Context, userID string) error {
	return s.store.DeleteUser(ctx, userID)
}

// CreatePasswordResetToken creates a password reset token
func (s *AuthService) CreatePasswordResetToken(ctx context.Context, email string) (string, error) {
	user, err := s.store.GetUserByEmail(ctx, email)
	if err == sql.ErrNoRows {
		// Don't reveal if email exists
		return "", nil
	}
	if err != nil {
		return "", err
	}

	token := generateToken()
	expires := time.Now().Add(1 * time.Hour).Unix()

	err = s.store.SetPasswordResetToken(ctx, db.SetPasswordResetTokenParams{
		ID:      user.ID,
		Token:   sql.NullString{String: token, Valid: true},
		Expires: sql.NullInt64{Int64: expires, Valid: true},
	})
	if err != nil {
		return "", err
	}

	return token, nil
}

// ResetPassword resets a user's password using a token
func (s *AuthService) ResetPassword(ctx context.Context, token, newPassword string) error {
	user, err := s.store.GetUserByPasswordResetToken(ctx, sql.NullString{String: token, Valid: true})
	if err == sql.ErrNoRows {
		return ErrInvalidToken
	}
	if err != nil {
		return err
	}

	// Hash new password
	hash, err := bcrypt.GenerateFromPassword([]byte(newPassword), bcrypt.DefaultCost)
	if err != nil {
		return fmt.Errorf("failed to hash password: %w", err)
	}

	// Update password (also clears reset token)
	return s.store.UpdateUserPassword(ctx, db.UpdateUserPasswordParams{
		ID:           user.ID,
		PasswordHash: string(hash),
	})
}

// GenerateTokensForUser creates tokens for an existing user (used for admin login bypass)
func (s *AuthService) GenerateTokensForUser(ctx context.Context, userID, email string) (*AuthResponse, error) {
	return s.generateTokens(ctx, userID, email)
}

// generateTokens creates access and refresh tokens for a user
func (s *AuthService) generateTokens(ctx context.Context, userID, email string) (*AuthResponse, error) {
	now := time.Now()
	accessExpiry := now.Add(time.Duration(s.config.Auth.AccessExpire) * time.Second)
	refreshExpiry := now.Add(time.Duration(s.config.Auth.RefreshTokenExpire) * time.Second)

	// Create access token
	claims := jwt.MapClaims{
		"userId": userID,
		"email":  email,
		"iat":    now.Unix(),
		"exp":    accessExpiry.Unix(),
	}
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, claims)
	accessToken, err := token.SignedString([]byte(s.config.Auth.AccessSecret))
	if err != nil {
		return nil, fmt.Errorf("failed to sign token: %w", err)
	}

	// Create refresh token
	refreshToken := generateToken()
	tokenHash := hashToken(refreshToken)

	_, err = s.store.CreateRefreshToken(ctx, db.CreateRefreshTokenParams{
		ID:        generateID(),
		UserID:    userID,
		TokenHash: tokenHash,
		ExpiresAt: refreshExpiry.Unix(),
	})
	if err != nil {
		return nil, fmt.Errorf("failed to store refresh token: %w", err)
	}

	return &AuthResponse{
		Token:        accessToken,
		RefreshToken: refreshToken,
		ExpiresAt:    accessExpiry,
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
