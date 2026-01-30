package middleware

import (
	"encoding/json"
	"io"
	"net/http"
	"strings"

	"nebo/internal/security"
)

// ValidationConfig holds configuration for request validation
type ValidationConfig struct {
	// MaxBodySize is the maximum request body size in bytes
	MaxBodySize int64
	// MaxURLLength is the maximum URL length
	MaxURLLength int
	// ValidateSQLInjection enables SQL injection detection in request parameters
	ValidateSQLInjection bool
	// ValidateXSS enables XSS detection in request parameters
	ValidateXSS bool
	// SanitizeInput enables automatic input sanitization
	SanitizeInput bool
}

// DefaultValidationConfig returns default validation settings
func DefaultValidationConfig() *ValidationConfig {
	return &ValidationConfig{
		MaxBodySize:          10 * 1024 * 1024, // 10MB
		MaxURLLength:         2048,
		ValidateSQLInjection: true,
		ValidateXSS:          true,
		SanitizeInput:        true,
	}
}

// RequestValidator validates incoming HTTP requests
type RequestValidator struct {
	config *ValidationConfig
}

// NewRequestValidator creates a new request validator
func NewRequestValidator(config *ValidationConfig) *RequestValidator {
	if config == nil {
		config = DefaultValidationConfig()
	}
	return &RequestValidator{config: config}
}

// Middleware returns HTTP middleware for request validation
func (rv *RequestValidator) Middleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Validate URL length
			if len(r.URL.String()) > rv.config.MaxURLLength {
				http.Error(w, "URL too long", http.StatusRequestURITooLong)
				return
			}

			// Validate request body size
			if r.ContentLength > rv.config.MaxBodySize {
				http.Error(w, "Request body too large", http.StatusRequestEntityTooLarge)
				return
			}

			// Limit body reader
			r.Body = http.MaxBytesReader(w, r.Body, rv.config.MaxBodySize)

			// Validate URL parameters
			if rv.config.ValidateSQLInjection || rv.config.ValidateXSS {
				// Check query parameters
				for key, values := range r.URL.Query() {
					for _, value := range values {
						if rv.config.ValidateSQLInjection && security.DetectSQLInjection(value) {
							rv.respondWithError(w, "Invalid input detected in query parameter: "+key)
							return
						}
						if rv.config.ValidateXSS && security.DetectXSS(value) {
							rv.respondWithError(w, "Invalid input detected in query parameter: "+key)
							return
						}
					}
				}

				// Check path parameters (from URL path)
				path := r.URL.Path
				if rv.config.ValidateSQLInjection && security.DetectSQLInjection(path) {
					rv.respondWithError(w, "Invalid input detected in URL path")
					return
				}
				if rv.config.ValidateXSS && security.DetectXSS(path) {
					rv.respondWithError(w, "Invalid input detected in URL path")
					return
				}
			}

			next.ServeHTTP(w, r)
		})
	}
}

// respondWithError sends a JSON error response
func (rv *RequestValidator) respondWithError(w http.ResponseWriter, message string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusBadRequest)
	json.NewEncoder(w).Encode(map[string]string{
		"error":   "validation_error",
		"message": message,
	})
}

// ValidateJSONBody validates and sanitizes a JSON request body
func ValidateJSONBody(r *http.Request, maxSize int64) ([]byte, error) {
	// Limit body size
	r.Body = http.MaxBytesReader(nil, r.Body, maxSize)

	// Read body
	body, err := io.ReadAll(r.Body)
	if err != nil {
		return nil, err
	}

	// Validate UTF-8
	if !security.ValidateUTF8(string(body)) {
		body = []byte(security.SanitizeUTF8(string(body)))
	}

	return body, nil
}

// SanitizeRequestMap sanitizes a map of request parameters
func SanitizeRequestMap(params map[string]interface{}) map[string]interface{} {
	result := make(map[string]interface{})
	for key, value := range params {
		sanitizedKey := security.SanitizeName(key)
		switch v := value.(type) {
		case string:
			result[sanitizedKey] = security.SanitizeString(v, security.DefaultSanitizeOptions())
		case map[string]interface{}:
			result[sanitizedKey] = SanitizeRequestMap(v)
		case []interface{}:
			result[sanitizedKey] = sanitizeSlice(v)
		default:
			result[sanitizedKey] = v
		}
	}
	return result
}

// sanitizeSlice sanitizes a slice of values
func sanitizeSlice(slice []interface{}) []interface{} {
	result := make([]interface{}, len(slice))
	for i, value := range slice {
		switch v := value.(type) {
		case string:
			result[i] = security.SanitizeString(v, security.DefaultSanitizeOptions())
		case map[string]interface{}:
			result[i] = SanitizeRequestMap(v)
		case []interface{}:
			result[i] = sanitizeSlice(v)
		default:
			result[i] = v
		}
	}
	return result
}

// ContentTypeValidator validates Content-Type headers
type ContentTypeValidator struct {
	allowedTypes []string
}

// NewContentTypeValidator creates a new content type validator
func NewContentTypeValidator(allowedTypes []string) *ContentTypeValidator {
	return &ContentTypeValidator{allowedTypes: allowedTypes}
}

// Middleware returns HTTP middleware for content type validation
func (cv *ContentTypeValidator) Middleware() func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			// Skip validation for GET, HEAD, OPTIONS
			if r.Method == http.MethodGet || r.Method == http.MethodHead || r.Method == http.MethodOptions {
				next.ServeHTTP(w, r)
				return
			}

			// Check Content-Type
			contentType := r.Header.Get("Content-Type")
			if contentType == "" {
				http.Error(w, "Content-Type header required", http.StatusUnsupportedMediaType)
				return
			}

			// Parse content type (handle parameters like charset)
			mediaType := strings.Split(contentType, ";")[0]
			mediaType = strings.TrimSpace(mediaType)

			allowed := false
			for _, allowedType := range cv.allowedTypes {
				if strings.EqualFold(mediaType, allowedType) {
					allowed = true
					break
				}
			}

			if !allowed {
				http.Error(w, "Unsupported Content-Type", http.StatusUnsupportedMediaType)
				return
			}

			next.ServeHTTP(w, r)
		})
	}
}

// JSONContentTypeValidator returns middleware that only allows application/json
func JSONContentTypeValidator() func(http.Handler) http.Handler {
	validator := NewContentTypeValidator([]string{"application/json"})
	return validator.Middleware()
}
