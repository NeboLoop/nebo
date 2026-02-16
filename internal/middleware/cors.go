package middleware

import (
	"net/http"
	"strconv"
	"strings"
)

// CORSConfig holds CORS configuration
type CORSConfig struct {
	// AllowedOrigins is a list of origins that are allowed to access the resource
	// Use "*" to allow all origins (not recommended for production with credentials)
	AllowedOrigins []string
	// AllowedMethods is a list of methods that are allowed
	AllowedMethods []string
	// AllowedHeaders is a list of headers that are allowed in requests
	AllowedHeaders []string
	// ExposedHeaders is a list of headers that can be exposed to the client
	ExposedHeaders []string
	// AllowCredentials indicates whether credentials (cookies, authorization headers) are allowed
	AllowCredentials bool
	// MaxAge indicates how long the results of a preflight request can be cached (in seconds)
	MaxAge int
	// Debug enables debug logging
	Debug bool
}

// DefaultCORSConfig returns a default CORS configuration
func DefaultCORSConfig() *CORSConfig {
	return &CORSConfig{
		AllowedOrigins:   []string{},
		AllowedMethods:   []string{"GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"},
		AllowedHeaders:   []string{"Accept", "Authorization", "Content-Type", "X-CSRF-Token", "X-Requested-With"},
		ExposedHeaders:   []string{"X-RateLimit-Limit", "X-RateLimit-Remaining", "X-RateLimit-Reset"},
		AllowCredentials: true,
		MaxAge:           86400, // 24 hours
	}
}

// ProductionCORSConfig returns a strict CORS configuration for production
func ProductionCORSConfig(allowedOrigins []string) *CORSConfig {
	return &CORSConfig{
		AllowedOrigins:   allowedOrigins,
		AllowedMethods:   []string{"GET", "POST", "PUT", "DELETE", "OPTIONS"},
		AllowedHeaders:   []string{"Accept", "Authorization", "Content-Type", "X-CSRF-Token"},
		ExposedHeaders:   []string{"X-RateLimit-Limit", "X-RateLimit-Remaining", "X-RateLimit-Reset"},
		AllowCredentials: true,
		MaxAge:           86400,
	}
}

// CORS represents a CORS middleware handler
type CORS struct {
	config *CORSConfig
}

// NewCORS creates a new CORS middleware
func NewCORS(config *CORSConfig) *CORS {
	if config == nil {
		config = DefaultCORSConfig()
	}
	return &CORS{config: config}
}

// Middleware returns an HTTP middleware that handles CORS
func (c *CORS) Middleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			origin := r.Header.Get("Origin")

			// Check if origin is allowed
			if origin != "" && c.isOriginAllowed(origin) {
				w.Header().Set("Access-Control-Allow-Origin", origin)
				w.Header().Set("Vary", "Origin")

				if c.config.AllowCredentials {
					w.Header().Set("Access-Control-Allow-Credentials", "true")
				}

				// Expose headers
				if len(c.config.ExposedHeaders) > 0 {
					w.Header().Set("Access-Control-Expose-Headers", strings.Join(c.config.ExposedHeaders, ", "))
				}
			}

			// Handle preflight requests
			if r.Method == http.MethodOptions {
				c.handlePreflight(w, r)
				return
			}

			next.ServeHTTP(w, r)
		})
	}
}

// handlePreflight handles CORS preflight requests
func (c *CORS) handlePreflight(w http.ResponseWriter, r *http.Request) {
	origin := r.Header.Get("Origin")

	if origin == "" || !c.isOriginAllowed(origin) {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	// Check requested method
	requestedMethod := r.Header.Get("Access-Control-Request-Method")
	if requestedMethod == "" || !c.isMethodAllowed(requestedMethod) {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	// Check requested headers
	requestedHeaders := r.Header.Get("Access-Control-Request-Headers")
	if requestedHeaders != "" && !c.areHeadersAllowed(requestedHeaders) {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	// Set preflight response headers
	w.Header().Set("Access-Control-Allow-Methods", strings.Join(c.config.AllowedMethods, ", "))
	w.Header().Set("Access-Control-Allow-Headers", strings.Join(c.config.AllowedHeaders, ", "))

	if c.config.MaxAge > 0 {
		w.Header().Set("Access-Control-Max-Age", strconv.Itoa(c.config.MaxAge))
	}

	w.WriteHeader(http.StatusNoContent)
}

// isOriginAllowed checks if an origin is allowed
func (c *CORS) isOriginAllowed(origin string) bool {
	if len(c.config.AllowedOrigins) == 0 {
		return false
	}

	for _, allowed := range c.config.AllowedOrigins {
		if allowed == "*" {
			return true
		}
		if allowed == origin {
			return true
		}
		// Support wildcard subdomains (e.g., *.example.com)
		if strings.HasPrefix(allowed, "*.") {
			suffix := allowed[1:] // Remove the *
			if strings.HasSuffix(origin, suffix) {
				return true
			}
		}
	}

	return false
}

// isMethodAllowed checks if a method is allowed
func (c *CORS) isMethodAllowed(method string) bool {
	method = strings.ToUpper(method)
	for _, allowed := range c.config.AllowedMethods {
		if strings.ToUpper(allowed) == method {
			return true
		}
	}
	return false
}

// areHeadersAllowed checks if all requested headers are allowed
func (c *CORS) areHeadersAllowed(requestedHeaders string) bool {
	headers := strings.Split(requestedHeaders, ",")
	for _, header := range headers {
		header = strings.TrimSpace(header)
		if !c.isHeaderAllowed(header) {
			return false
		}
	}
	return true
}

// isHeaderAllowed checks if a single header is allowed
func (c *CORS) isHeaderAllowed(header string) bool {
	header = strings.ToLower(header)
	for _, allowed := range c.config.AllowedHeaders {
		if strings.ToLower(allowed) == header {
			return true
		}
	}
	// Always allow simple headers
	simpleHeaders := []string{"accept", "accept-language", "content-language", "content-type"}
	for _, simple := range simpleHeaders {
		if header == simple {
			return true
		}
	}
	return false
}

// CORSMiddleware is a convenience function that creates CORS middleware
func CORSMiddleware(config *CORSConfig) func(http.Handler) http.Handler {
	cors := NewCORS(config)
	return cors.Middleware()
}

// ParseAllowedOrigins parses a comma-separated list of origins
// IsLocalhostOrigin checks if an origin is from localhost (any port).
// Used by CORS middleware and WebSocket upgraders to restrict to local connections.
func IsLocalhostOrigin(origin string) bool {
	for _, prefix := range []string{
		"http://localhost",
		"https://localhost",
		"http://127.0.0.1",
		"https://127.0.0.1",
	} {
		if origin == prefix || (len(origin) > len(prefix) && origin[:len(prefix)+1] == prefix+":") {
			return true
		}
	}
	return false
}

func ParseAllowedOrigins(origins string) []string {
	if origins == "" {
		return []string{}
	}

	result := make([]string, 0)
	for _, origin := range strings.Split(origins, ",") {
		origin = strings.TrimSpace(origin)
		if origin != "" {
			result = append(result, origin)
		}
	}
	return result
}
