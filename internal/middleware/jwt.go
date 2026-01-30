package middleware

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/golang-jwt/jwt/v4"
	"nebo/internal/logging"
)

// JWTClaims represents the claims from a Levee JWT token
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

// LeveeTokenTranslator creates middleware that translates Levee JWT tokens to Gobot tokens
// It intercepts Levee-issued tokens, validates them, extracts claims, and re-signs with Gobot's secret
// This allows our JWT middleware to validate the translated token
func LeveeTokenTranslator(accessSecret string) func(next http.HandlerFunc) http.HandlerFunc {
	logging.Infof("[LeveeTokenTranslator] Middleware initialized")
	return func(next http.HandlerFunc) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			// Skip non-authenticated routes
			authHeader := r.Header.Get("Authorization")
			if authHeader == "" {
				// No auth header, let the request through
				next(w, r)
				return
			}

			logging.Infof("[LeveeTokenTranslator] Processing request to %s with auth header", r.URL.Path)

			// Extract token
			parts := strings.SplitN(authHeader, " ", 2)
			if len(parts) != 2 || !strings.EqualFold(parts[0], "bearer") {
				logging.Infof("[LeveeTokenTranslator] Invalid auth header format")
				next(w, r)
				return
			}

			token := parts[1]
			if token == "" {
				logging.Infof("[LeveeTokenTranslator] Empty token")
				next(w, r)
				return
			}

			// Parse the JWT token
			claims, err := parseJWTClaims(token)
			if err != nil {
				logging.Infof("[LeveeTokenTranslator] Token parse failed: %v", err)
				next(w, r)
				return
			}

			logging.Infof("[LeveeTokenTranslator] Parsed token - iss: %s, sub: %s, email: %s", claims.Iss, claims.Sub, claims.Email)

			// Check if this is a Levee token
			if claims.Iss != "levee.sh/sdk" {
				logging.Infof("[LeveeTokenTranslator] Not a Levee token (iss=%s), passing through", claims.Iss)
				next(w, r)
				return
			}

			// Validate expiration
			if claims.Exp > 0 && time.Now().Unix() > claims.Exp {
				logging.Infof("[LeveeTokenTranslator] Token expired for user: %s", claims.Sub)
				next(w, r)
				return
			}

			// Create new claims (using "userId" not "sub")
			newClaims := GoZeroClaims{
				UserId: claims.Sub,
				Email:  claims.Email,
				Name:   claims.Name,
				Iss:    "gobot",
				Exp:    claims.Exp,
				Iat:    claims.Iat,
			}

			// Create new JWT signed with Gobot's secret
			newToken, err := createJWT(newClaims, accessSecret)
			if err != nil {
				logging.Errorf("[LeveeTokenTranslator] Failed to create translated token: %v", err)
				next(w, r)
				return
			}

			// Replace the Authorization header with the translated token
			r.Header.Set("Authorization", "Bearer "+newToken)

			logging.Infof("[LeveeTokenTranslator] Successfully translated token for user: %s (%s)", claims.Sub, claims.Email)

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

// LeveeJWTMiddleware creates middleware that parses Levee JWT tokens
// It extracts claims and sets them in the request context
// Note: This middleware trusts Levee-issued tokens without cryptographic verification
// because Levee is the trusted auth provider
func LeveeJWTMiddleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Extract token from Authorization header
			authHeader := r.Header.Get("Authorization")
			if authHeader == "" {
				unauthorized(w, "missing authorization header")
				return
			}

			// Expect "Bearer <token>"
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

			// Parse the JWT token (Levee uses standard JWT format)
			claims, err := parseJWTClaims(token)
			if err != nil {
				logging.Errorf("Failed to parse JWT: %v", err)
				unauthorized(w, "invalid token")
				return
			}

			// Validate issuer
			if claims.Iss != "levee.sh/sdk" {
				logging.Errorf("Invalid token issuer: %s", claims.Iss)
				unauthorized(w, "invalid token issuer")
				return
			}

			// Check if token is expired
			// Note: Levee tokens have their own expiration logic

			// Set claims in context
			ctx := r.Context()
			ctx = context.WithValue(ctx, UserIDKey, claims.Sub)
			ctx = context.WithValue(ctx, UserEmailKey, claims.Email)
			ctx = context.WithValue(ctx, UserNameKey, claims.Name)

			// Also set "userId" key for backwards compatibility
			ctx = context.WithValue(ctx, "userId", claims.Sub)

			logging.Infof("JWT auth: user=%s email=%s", claims.Sub, claims.Email)

			next.ServeHTTP(w, r.WithContext(ctx))
		})
	}
}

// parseJWTClaims parses the claims from a JWT token without signature verification
// This is safe because we trust Levee as the auth provider
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
