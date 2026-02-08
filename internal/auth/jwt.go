package auth

import (
	"errors"
	"time"

	"github.com/golang-jwt/jwt/v4"
	"github.com/google/uuid"
)

var (
	ErrInvalidToken      = errors.New("invalid token")
	ErrExpiredToken      = errors.New("token has expired")
	ErrInvalidClaims     = errors.New("invalid token claims")
	ErrMissingUserID     = errors.New("missing user ID in token")
	ErrInvalidSignMethod = errors.New("invalid signing method")
)

// TokenPair contains access and refresh tokens
type TokenPair struct {
	AccessToken  string
	RefreshToken string
	ExpiresAt    int64 // Unix timestamp in milliseconds
}

// GenerateTokens creates a new access token and refresh token for a user
func GenerateTokens(userID, email, name, accessSecret string, accessExpireSecs, refreshExpireSecs int64) (*TokenPair, error) {
	now := time.Now()
	accessExp := now.Add(time.Duration(accessExpireSecs) * time.Second)
	refreshExp := now.Add(time.Duration(refreshExpireSecs) * time.Second)

	// Create access token
	accessClaims := jwt.MapClaims{
		"userId": userID,
		"email":  email,
		"name":   name,
		"iat":    now.Unix(),
		"exp":    accessExp.Unix(),
	}
	accessToken := jwt.NewWithClaims(jwt.SigningMethodHS256, accessClaims)
	accessTokenStr, err := accessToken.SignedString([]byte(accessSecret))
	if err != nil {
		return nil, err
	}

	// Create refresh token (longer lived, includes email for user lookup)
	refreshClaims := jwt.MapClaims{
		"userId": userID,
		"email":  email,
		"type":   "refresh",
		"iat":    now.Unix(),
		"exp":    refreshExp.Unix(),
	}
	refreshToken := jwt.NewWithClaims(jwt.SigningMethodHS256, refreshClaims)
	refreshTokenStr, err := refreshToken.SignedString([]byte(accessSecret))
	if err != nil {
		return nil, err
	}

	return &TokenPair{
		AccessToken:  accessTokenStr,
		RefreshToken: refreshTokenStr,
		ExpiresAt:    accessExp.UnixMilli(),
	}, nil
}

// GetUserIDFromContext extracts the user ID from JWT claims set in context by JWT middleware
// The JWT middleware adds JWT claims to context with the claim name as key
func GetUserIDFromContext(ctx interface{ Value(any) any }) (uuid.UUID, error) {
	// JWT middleware sets individual claims in context by their key name
	userIDValue := ctx.Value("userId")
	if userIDValue == nil {
		// Try "sub" as fallback (standard JWT claim)
		userIDValue = ctx.Value("sub")
	}

	if userIDValue == nil {
		return uuid.Nil, ErrMissingUserID
	}

	userIDStr, ok := userIDValue.(string)
	if !ok {
		return uuid.Nil, ErrInvalidClaims
	}

	userID, err := uuid.Parse(userIDStr)
	if err != nil {
		return uuid.Nil, ErrInvalidClaims
	}

	return userID, nil
}

// GetEmailFromContext extracts the email from JWT claims set in context by JWT middleware
func GetEmailFromContext(ctx interface{ Value(any) any }) (string, error) {
	// JWT middleware sets individual claims in context by their key name
	emailValue := ctx.Value("email")
	if emailValue == nil {
		return "", errors.New("missing email in token")
	}

	email, ok := emailValue.(string)
	if !ok {
		return "", ErrInvalidClaims
	}

	return email, nil
}

// GetCustomerIDFromContext extracts the customer ID from JWT claims set in context
func GetCustomerIDFromContext(ctx interface{ Value(any) any }) (string, error) {
	// Try customer_id first
	customerIDValue := ctx.Value("customer_id")
	if customerIDValue == nil {
		// Fallback to sub (standard JWT claim)
		customerIDValue = ctx.Value("sub")
	}
	if customerIDValue == nil {
		// Fallback to userId for backwards compatibility
		customerIDValue = ctx.Value("userId")
	}

	if customerIDValue == nil {
		return "", errors.New("missing customer ID in token")
	}

	customerID, ok := customerIDValue.(string)
	if !ok {
		return "", ErrInvalidClaims
	}

	return customerID, nil
}
