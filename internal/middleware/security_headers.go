package middleware

import (
	"net/http"
)

// SecurityHeaders contains configuration for HTTP security headers
type SecurityHeaders struct {
	// ContentSecurityPolicy defines the Content-Security-Policy header value
	ContentSecurityPolicy string
	// PermissionsPolicy defines the Permissions-Policy header value
	PermissionsPolicy string
	// ReferrerPolicy defines the Referrer-Policy header value
	ReferrerPolicy string
	// StrictTransportSecurity defines the Strict-Transport-Security header value
	StrictTransportSecurity string
	// XContentTypeOptions defines the X-Content-Type-Options header value
	XContentTypeOptions string
	// XFrameOptions defines the X-Frame-Options header value
	XFrameOptions string
	// XXSSProtection defines the X-XSS-Protection header value
	XXSSProtection string
	// CacheControl defines the Cache-Control header value for sensitive endpoints
	CacheControl string
	// Pragma defines the Pragma header value for sensitive endpoints
	Pragma string
}

// DefaultSecurityHeaders returns the recommended security headers configuration
func DefaultSecurityHeaders() *SecurityHeaders {
	return &SecurityHeaders{
		// Content Security Policy - restrictive default, customize as needed
		ContentSecurityPolicy: "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
		// Permissions Policy - disable potentially dangerous features
		PermissionsPolicy: "accelerometer=(), camera=(self), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()",
		// Referrer Policy - send referrer only for same-origin requests
		ReferrerPolicy: "strict-origin-when-cross-origin",
		// Strict Transport Security - enforce HTTPS for 1 year, include subdomains
		StrictTransportSecurity: "max-age=31536000; includeSubDomains; preload",
		// Prevent MIME type sniffing
		XContentTypeOptions: "nosniff",
		// Prevent clickjacking
		XFrameOptions: "DENY",
		// Enable XSS filter (legacy browsers)
		XXSSProtection: "1; mode=block",
		// Prevent caching of sensitive responses
		CacheControl: "no-store, no-cache, must-revalidate, private",
		Pragma:       "no-cache",
	}
}

// APISecurityHeaders returns security headers optimized for API endpoints
func APISecurityHeaders() *SecurityHeaders {
	return &SecurityHeaders{
		// APIs typically don't need CSP, but we set a restrictive one anyway
		ContentSecurityPolicy:   "default-src 'none'; frame-ancestors 'none'",
		PermissionsPolicy:       "accelerometer=(), camera=(), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()",
		ReferrerPolicy:          "strict-origin-when-cross-origin",
		StrictTransportSecurity: "max-age=31536000; includeSubDomains; preload",
		XContentTypeOptions:     "nosniff",
		XFrameOptions:           "DENY",
		XXSSProtection:          "1; mode=block",
		CacheControl:            "no-store, no-cache, must-revalidate, private",
		Pragma:                  "no-cache",
	}
}

// SecurityHeadersMiddleware adds security headers to all responses
func SecurityHeadersMiddleware(config *SecurityHeaders) func(http.Handler) http.Handler {
	if config == nil {
		config = DefaultSecurityHeaders()
	}

	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Set security headers
			if config.ContentSecurityPolicy != "" {
				w.Header().Set("Content-Security-Policy", config.ContentSecurityPolicy)
			}
			if config.PermissionsPolicy != "" {
				w.Header().Set("Permissions-Policy", config.PermissionsPolicy)
			}
			if config.ReferrerPolicy != "" {
				w.Header().Set("Referrer-Policy", config.ReferrerPolicy)
			}
			if config.StrictTransportSecurity != "" {
				w.Header().Set("Strict-Transport-Security", config.StrictTransportSecurity)
			}
			if config.XContentTypeOptions != "" {
				w.Header().Set("X-Content-Type-Options", config.XContentTypeOptions)
			}
			if config.XFrameOptions != "" {
				w.Header().Set("X-Frame-Options", config.XFrameOptions)
			}
			if config.XXSSProtection != "" {
				w.Header().Set("X-XSS-Protection", config.XXSSProtection)
			}
			if config.CacheControl != "" {
				w.Header().Set("Cache-Control", config.CacheControl)
			}
			if config.Pragma != "" {
				w.Header().Set("Pragma", config.Pragma)
			}

			next.ServeHTTP(w, r)
		})
	}
}

// SecureHandler wraps an http.Handler with security headers
func SecureHandler(handler http.Handler, config *SecurityHeaders) http.Handler {
	return SecurityHeadersMiddleware(config)(handler)
}

// SecureHandlerFunc wraps an http.HandlerFunc with security headers
func SecureHandlerFunc(handler http.HandlerFunc, config *SecurityHeaders) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		SecurityHeadersMiddleware(config)(handler).ServeHTTP(w, r)
	}
}
