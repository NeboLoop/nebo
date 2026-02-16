package middleware

import (
	"net/http"
	"time"

	"github.com/neboloop/nebo/internal/config"
)

// SecurityMiddleware combines all security middleware into a single handler
type SecurityMiddleware struct {
	config          config.Config
	securityHeaders *SecurityHeaders
	csrf            *CSRFProtection
	rateLimiter     *RateLimiter
	authRateLimiter *RateLimiter
	cors            *CORS
}

// NewSecurityMiddleware creates a new security middleware with the given configuration
func NewSecurityMiddleware(cfg config.Config) *SecurityMiddleware {
	sm := &SecurityMiddleware{
		config: cfg,
	}

	// Initialize security headers
	if cfg.IsSecurityHeadersEnabled() {
		headers := APISecurityHeaders()
		if cfg.Security.ContentSecurityPolicy != "" {
			headers.ContentSecurityPolicy = cfg.Security.ContentSecurityPolicy
		}
		sm.securityHeaders = headers
	}

	// Initialize CSRF protection
	if cfg.IsCSRFEnabled() {
		secret := cfg.Security.CSRFSecret
		if secret == "" {
			secret = cfg.Auth.AccessSecret
		}
		csrfConfig := DefaultCSRFConfig([]byte(secret))
		csrfConfig.TokenExpiry = time.Duration(cfg.Security.CSRFTokenExpiry) * time.Second
		csrfConfig.SecureCookie = cfg.IsCSRFSecureCookie()
		// Skip CSRF for API endpoints that use JWT authentication
		csrfConfig.SkipPaths = []string{"/api/v1/"}
		sm.csrf = NewCSRFProtection(csrfConfig)
	}

	// Initialize rate limiting
	if cfg.IsRateLimitEnabled() {
		// General rate limiter
		rateLimitConfig := &RateLimitConfig{
			Rate:            cfg.Security.RateLimitRequests,
			Interval:        time.Duration(cfg.Security.RateLimitInterval) * time.Second,
			Burst:           cfg.Security.RateLimitBurst,
			KeyFunc:         DefaultKeyFunc,
			ExceededHandler: DefaultExceededHandler,
		}
		sm.rateLimiter = NewRateLimiter(rateLimitConfig)

		// Auth endpoints rate limiter (stricter)
		authRateLimitConfig := &RateLimitConfig{
			Rate:            cfg.Security.AuthRateLimitRequests,
			Interval:        time.Duration(cfg.Security.AuthRateLimitInterval) * time.Second,
			Burst:           cfg.Security.AuthRateLimitRequests,
			KeyFunc:         DefaultKeyFunc,
			ExceededHandler: DefaultExceededHandler,
		}
		sm.authRateLimiter = NewRateLimiter(authRateLimitConfig)
	}

	// Initialize CORS
	if cfg.Security.AllowedOrigins != "" {
		origins := ParseAllowedOrigins(cfg.Security.AllowedOrigins)
		sm.cors = NewCORS(ProductionCORSConfig(origins))
	} else {
		// Default CORS configuration for development
		sm.cors = NewCORS(DefaultCORSConfig())
	}

	return sm
}

// Handler wraps an HTTP handler with all security middleware
func (sm *SecurityMiddleware) Handler(handler http.Handler) http.Handler {
	// Apply middleware in reverse order (last applied runs first)
	wrapped := handler

	// Security headers (always last to ensure headers are set)
	if sm.securityHeaders != nil {
		wrapped = SecurityHeadersMiddleware(sm.securityHeaders)(wrapped)
	}

	// CORS
	if sm.cors != nil {
		wrapped = sm.cors.Middleware()(wrapped)
	}

	// Rate limiting
	if sm.rateLimiter != nil {
		wrapped = sm.rateLimiter.Middleware()(wrapped)
	}

	return wrapped
}

// AuthHandler wraps an authentication handler with stricter security
func (sm *SecurityMiddleware) AuthHandler(handler http.Handler) http.Handler {
	wrapped := handler

	// Security headers
	if sm.securityHeaders != nil {
		wrapped = SecurityHeadersMiddleware(sm.securityHeaders)(wrapped)
	}

	// CORS
	if sm.cors != nil {
		wrapped = sm.cors.Middleware()(wrapped)
	}

	// Stricter rate limiting for auth endpoints
	if sm.authRateLimiter != nil {
		wrapped = sm.authRateLimiter.Middleware()(wrapped)
	}

	// CSRF protection for non-API auth endpoints
	if sm.csrf != nil {
		wrapped = sm.csrf.Middleware()(wrapped)
	}

	return wrapped
}

// GetCSRFTokenHandler returns a handler that provides CSRF tokens
func (sm *SecurityMiddleware) GetCSRFTokenHandler() http.HandlerFunc {
	if sm.csrf != nil {
		return sm.csrf.GetTokenHandler()
	}
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(`{"message":"CSRF protection disabled"}`))
	}
}

// HTTPSRedirectMiddleware redirects HTTP to HTTPS
func HTTPSRedirectMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Check if behind a proxy that terminates SSL
		if r.Header.Get("X-Forwarded-Proto") == "http" || (r.TLS == nil && r.Header.Get("X-Forwarded-Proto") == "") {
			target := "https://" + r.Host + r.URL.RequestURI()
			http.Redirect(w, r, target, http.StatusMovedPermanently)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// RequestSizeMiddleware limits the request body size
func RequestSizeMiddleware(maxBytes int64) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.ContentLength > maxBytes {
				http.Error(w, "Request body too large", http.StatusRequestEntityTooLarge)
				return
			}
			r.Body = http.MaxBytesReader(w, r.Body, maxBytes)
			next.ServeHTTP(w, r)
		})
	}
}

// ChainMiddleware chains multiple middleware functions
func ChainMiddleware(middlewares ...func(http.Handler) http.Handler) func(http.Handler) http.Handler {
	return func(handler http.Handler) http.Handler {
		for i := len(middlewares) - 1; i >= 0; i-- {
			handler = middlewares[i](handler)
		}
		return handler
	}
}

// RequestIDMiddleware adds a unique request ID to each request
func RequestIDMiddleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestID := r.Header.Get("X-Request-ID")
		if requestID == "" {
			requestID = generateRequestID()
		}
		w.Header().Set("X-Request-ID", requestID)
		next.ServeHTTP(w, r)
	})
}

// generateRequestID generates a unique request ID
func generateRequestID() string {
	// Simple implementation using timestamp and random suffix
	return time.Now().Format("20060102150405") + "-" + randomString(8)
}

// randomString generates a random alphanumeric string
func randomString(length int) string {
	const charset = "abcdefghijklmnopqrstuvwxyz0123456789"
	result := make([]byte, length)
	for i := range result {
		result[i] = charset[time.Now().UnixNano()%int64(len(charset))]
		time.Sleep(time.Nanosecond) // Ensure different values
	}
	return string(result)
}
