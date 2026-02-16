package middleware

import (
	"context"
	"net/http"
	"strings"

	"github.com/golang-jwt/jwt/v4"
	"github.com/neboloop/nebo/internal/httputil"
)

// JWTMiddleware creates a chi middleware that validates JWT tokens
func JWTMiddleware(secret string) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Get token from Authorization header
			authHeader := r.Header.Get("Authorization")
			if authHeader == "" {
				httputil.Unauthorized(w, "missing authorization header")
				return
			}

			// Extract Bearer token
			parts := strings.SplitN(authHeader, " ", 2)
			if len(parts) != 2 || !strings.EqualFold(parts[0], "bearer") {
				httputil.Unauthorized(w, "invalid authorization header format")
				return
			}
			tokenString := parts[1]

			// Parse and validate token
			token, err := jwt.Parse(tokenString, func(token *jwt.Token) (interface{}, error) {
				if _, ok := token.Method.(*jwt.SigningMethodHMAC); !ok {
					return nil, jwt.ErrSignatureInvalid
				}
				return []byte(secret), nil
			})

			if err != nil || !token.Valid {
				httputil.Unauthorized(w, "invalid token")
				return
			}

			// Extract claims and add to context
			claims, ok := token.Claims.(jwt.MapClaims)
			if !ok {
				httputil.Unauthorized(w, "invalid token claims")
				return
			}

			// Add claims to context (compatible with existing code)
			ctx := r.Context()
			if userID, ok := claims["userId"].(string); ok {
				ctx = context.WithValue(ctx, "userId", userID)
			}
			if email, ok := claims["email"].(string); ok {
				ctx = context.WithValue(ctx, "email", email)
			}

			next.ServeHTTP(w, r.WithContext(ctx))
		})
	}
}
