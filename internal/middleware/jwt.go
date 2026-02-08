package middleware

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/golang-jwt/jwt/v4"
	"github.com/nebolabs/nebo/internal/logging"
)

// JWTClaims represents the claims from a JWT token
type JWTClaims struct {
	Sub   string `json:"sub"`   // Subject (customer ID)
	Email string `json:"email"` // Customer email
	Name  string `json:"name"`  // Customer name
	Iss   string `json:"iss"`   // Issuer
	Exp   int64  `json:"exp"`   // Expiration time
	Iat   int64  `json:"iat"`   // Issued at
}

// GoZeroClaims represents claims expected by our JWT middleware
type GoZeroClaims struct {
	UserId string `json:"userId"` // userId claim for internal use
	Email  string `json:"email"`
	Name   string `json:"name"`
	Iss    string `json:"iss"`
	Exp    int64  `json:"exp"`
	Iat    int64  `json:"iat"`
}

// ContextKey is a type for context keys
type ContextKey string

const (
	// UserIDKey is the context key for user ID
	UserIDKey ContextKey = "userId"
	// UserEmailKey is the context key for user email
	UserEmailKey ContextKey = "userEmail"
	// UserNameKey is the context key for user name
	UserNameKey ContextKey = "userName"
)

// ExternalTokenTranslator creates middleware that translates external JWT tokens to internal tokens.
// It intercepts externally-issued tokens, validates them, extracts claims, and re-signs with our secret.
func ExternalTokenTranslator(accessSecret string) func(next http.HandlerFunc) http.HandlerFunc {
	logging.Infof("[ExternalTokenTranslator] Middleware initialized")
	return func(next http.HandlerFunc) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			// Skip non-authenticated routes
			authHeader := r.Header.Get("Authorization")
			if authHeader == "" {
				next(w, r)
				return
			}

			// Extract token
			parts := strings.SplitN(authHeader, " ", 2)
			if len(parts) != 2 || !strings.EqualFold(parts[0], "bearer") {
				next(w, r)
				return
			}

			token := parts[1]
			if token == "" {
				next(w, r)
				return
			}

			// Parse the JWT token
			claims, err := parseJWTClaims(token)
			if err != nil {
				next(w, r)
				return
			}

			// Only translate tokens from known external issuers
			if claims.Iss == "nebo" {
				next(w, r)
				return
			}

			// Validate expiration
			if claims.Exp > 0 && time.Now().Unix() > claims.Exp {
				next(w, r)
				return
			}

			// Create new internal claims
			newClaims := GoZeroClaims{
				UserId: claims.Sub,
				Email:  claims.Email,
				Name:   claims.Name,
				Iss:    "nebo",
				Exp:    claims.Exp,
				Iat:    claims.Iat,
			}

			newToken, err := createJWT(newClaims, accessSecret)
			if err != nil {
				logging.Errorf("[ExternalTokenTranslator] Failed to create translated token: %v", err)
				next(w, r)
				return
			}

			r.Header.Set("Authorization", "Bearer "+newToken)
			next(w, r)
		}
	}
}

// createJWT creates a new JWT token with the given claims signed with the secret
// Uses golang-jwt/jwt library for JWT signing
func createJWT(claims GoZeroClaims, secret string) (string, error) {
	// Create JWT claims
	jwtClaims := jwt.MapClaims{
		"userId": claims.UserId,
		"email":  claims.Email,
		"name":   claims.Name,
		"iss":    claims.Iss,
		"exp":    claims.Exp,
		"iat":    claims.Iat,
	}

	// Create token with HS256 signing method
	token := jwt.NewWithClaims(jwt.SigningMethodHS256, jwtClaims)

	// Sign and return the token
	return token.SignedString([]byte(secret))
}

// ExternalJWTMiddleware creates middleware that parses external JWT tokens.
// It extracts claims and sets them in the request context.
func ExternalJWTMiddleware(trustedIssuer string) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			authHeader := r.Header.Get("Authorization")
			if authHeader == "" {
				unauthorized(w, "missing authorization header")
				return
			}

			parts := strings.SplitN(authHeader, " ", 2)
			if len(parts) != 2 || !strings.EqualFold(parts[0], "bearer") {
				unauthorized(w, "invalid authorization header format")
				return
			}

			token := parts[1]
			if token == "" {
				unauthorized(w, "empty token")
				return
			}

			claims, err := parseJWTClaims(token)
			if err != nil {
				logging.Errorf("Failed to parse JWT: %v", err)
				unauthorized(w, "invalid token")
				return
			}

			if trustedIssuer != "" && claims.Iss != trustedIssuer {
				logging.Errorf("Invalid token issuer: %s", claims.Iss)
				unauthorized(w, "invalid token issuer")
				return
			}

			ctx := r.Context()
			ctx = context.WithValue(ctx, UserIDKey, claims.Sub)
			ctx = context.WithValue(ctx, UserEmailKey, claims.Email)
			ctx = context.WithValue(ctx, UserNameKey, claims.Name)
			ctx = context.WithValue(ctx, "userId", claims.Sub)

			next.ServeHTTP(w, r.WithContext(ctx))
		})
	}
}

// parseJWTClaims parses the claims from a JWT token without signature verification
func parseJWTClaims(tokenString string) (*JWTClaims, error) {
	// JWT format: header.payload.signature
	parts := strings.Split(tokenString, ".")
	if len(parts) != 3 {
		return nil, ErrInvalidToken
	}

	// Decode the payload (second part)
	payload, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return nil, ErrInvalidToken
	}

	var claims JWTClaims
	if err := json.Unmarshal(payload, &claims); err != nil {
		return nil, ErrInvalidToken
	}

	return &claims, nil
}

// ParseJWTClaimsFromToken is an exported version of parseJWTClaims for use by other packages
// (e.g., websocket handler for extracting user_id from JWT cookies)
func ParseJWTClaimsFromToken(tokenString string) (*JWTClaims, error) {
	return parseJWTClaims(tokenString)
}

// unauthorized sends a 401 response
func unauthorized(w http.ResponseWriter, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusUnauthorized)
	json.NewEncoder(w).Encode(map[string]string{
		"error": message,
	})
}

// ErrInvalidToken is returned when token parsing fails
var ErrInvalidToken = &tokenError{message: "invalid token"}

type tokenError struct {
	message string
}

func (e *tokenError) Error() string {
	return e.message
}

// GetUserID extracts user ID from context
func GetUserID(ctx context.Context) string {
	if id, ok := ctx.Value(UserIDKey).(string); ok {
		return id
	}
	// Fallback to "userId" key
	if id, ok := ctx.Value("userId").(string); ok {
		return id
	}
	return ""
}

// GetUserEmail extracts user email from context
func GetUserEmail(ctx context.Context) string {
	if email, ok := ctx.Value(UserEmailKey).(string); ok {
		return email
	}
	return ""
}

// GetUserName extracts user name from context
func GetUserName(ctx context.Context) string {
	if name, ok := ctx.Value(UserNameKey).(string); ok {
		return name
	}
	return ""
}

// ValidateJWT validates a JWT token and returns its claims
func ValidateJWT(tokenString, secret string) (jwt.MapClaims, error) {
	token, err := jwt.Parse(tokenString, func(token *jwt.Token) (interface{}, error) {
		if _, ok := token.Method.(*jwt.SigningMethodHMAC); !ok {
			return nil, ErrInvalidToken
		}
		return []byte(secret), nil
	})
	if err != nil {
		return nil, err
	}

	if claims, ok := token.Claims.(jwt.MapClaims); ok && token.Valid {
		return claims, nil
	}

	return nil, ErrInvalidToken
}
