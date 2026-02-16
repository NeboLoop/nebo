package mcpauth

import (
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"errors"
	"net/http"
	"strings"

	"github.com/neboloop/nebo/internal/svc"

	"github.com/golang-jwt/jwt/v4"
	"github.com/modelcontextprotocol/go-sdk/auth"
)

// HashToken creates a SHA256 hash of a token for secure storage.
func HashToken(token string) string {
	h := sha256.Sum256([]byte(token))
	return hex.EncodeToString(h[:])
}

// UserInfo contains authenticated user information.
type UserInfo struct {
	UserID string
	Email  string
	Name   string
}

// userInfoKey is used to store UserInfo in context.
type userInfoKey struct{}

// tokenInfoKey is used to store TokenInfo in context.
type tokenInfoKey struct{}

// WithUserInfo adds UserInfo to a context.
func WithUserInfo(ctx context.Context, info *UserInfo) context.Context {
	return context.WithValue(ctx, userInfoKey{}, info)
}

// UserInfoFromContext retrieves UserInfo from a context.
func UserInfoFromContext(ctx context.Context) *UserInfo {
	if info, ok := ctx.Value(userInfoKey{}).(*UserInfo); ok {
		return info
	}
	return nil
}

// ContextWithTokenInfo adds TokenInfo to a context.
func ContextWithTokenInfo(ctx context.Context, info *auth.TokenInfo) context.Context {
	return context.WithValue(ctx, tokenInfoKey{}, info)
}

// TokenInfoFromContext retrieves TokenInfo from a context.
func TokenInfoFromContext(ctx context.Context) *auth.TokenInfo {
	if info, ok := ctx.Value(tokenInfoKey{}).(*auth.TokenInfo); ok {
		return info
	}
	return nil
}

// Authenticator handles MCP authentication using JWT tokens.
type Authenticator struct {
	svc *svc.ServiceContext
}

// NewAuthenticator creates a new MCP authenticator.
func NewAuthenticator(svc *svc.ServiceContext) *Authenticator {
	return &Authenticator{
		svc: svc,
	}
}

// TokenVerifier returns a token verifier function for use with auth.RequireBearerToken.
// It verifies JWT tokens using the same secret as the main API.
func (a *Authenticator) TokenVerifier() func(ctx context.Context, token string, req *http.Request) (*auth.TokenInfo, error) {
	return func(ctx context.Context, token string, req *http.Request) (*auth.TokenInfo, error) {
		return a.verifyJWT(ctx, token)
	}
}

// verifyJWT validates a JWT token and returns token info.
func (a *Authenticator) verifyJWT(ctx context.Context, tokenString string) (*auth.TokenInfo, error) {
	// Remove "Bearer " prefix if present
	tokenString = strings.TrimPrefix(tokenString, "Bearer ")

	// Parse and validate the JWT
	token, err := jwt.Parse(tokenString, func(token *jwt.Token) (interface{}, error) {
		// Validate signing method
		if _, ok := token.Method.(*jwt.SigningMethodHMAC); !ok {
			return nil, errors.New("unexpected signing method")
		}
		return []byte(a.svc.Config.Auth.AccessSecret), nil
	})

	if err != nil || !token.Valid {
		return nil, auth.ErrInvalidToken
	}

	claims, ok := token.Claims.(jwt.MapClaims)
	if !ok {
		return nil, auth.ErrInvalidToken
	}

	// Extract user ID from claims
	userID, ok := claims["userId"].(string)
	if !ok {
		// Try alternate claim name
		if sub, ok := claims["sub"].(string); ok {
			userID = sub
		} else {
			return nil, auth.ErrInvalidToken
		}
	}

	// Look up user in database
	user, err := a.svc.DB.GetUserByID(ctx, userID)
	if err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, auth.ErrInvalidToken
		}
		return nil, err
	}

	// Build user info
	userInfo := &UserInfo{
		UserID: user.ID,
		Email:  user.Email,
		Name:   user.Name,
	}

	return &auth.TokenInfo{
		Extra: map[string]any{
			"user_id":   user.ID,
			"user_info": userInfo,
			"user":      &user,
		},
	}, nil
}

// Middleware returns an HTTP middleware that authenticates requests.
func (a *Authenticator) Middleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Get Authorization header
		authHeader := r.Header.Get("Authorization")
		if authHeader == "" {
			http.Error(w, "Authorization required", http.StatusUnauthorized)
			return
		}

		// Verify token
		tokenInfo, err := a.verifyJWT(r.Context(), authHeader)
		if err != nil {
			http.Error(w, "Invalid token", http.StatusUnauthorized)
			return
		}

		// Add token info to context
		ctx := ContextWithTokenInfo(r.Context(), tokenInfo)

		// Add user info to context
		if userInfo, ok := tokenInfo.Extra["user_info"].(*UserInfo); ok {
			ctx = WithUserInfo(ctx, userInfo)
		}

		next.ServeHTTP(w, r.WithContext(ctx))
	})
}
